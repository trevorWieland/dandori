use dandori_app_services::{
    IssueAppService, handle_mcp_create_issue, handle_mcp_get_issue, handle_rest_create_issue,
    handle_rest_get_issue,
};
use dandori_contract::{CreateIssueRequest, Envelope, IssuePriorityDto};
use dandori_domain::AuthContext;
use dandori_test_support::{TestDatabase, setup_database};
use serde_json::json;
use uuid::Uuid;

struct TestService {
    _db: TestDatabase,
    auth: AuthContext,
    project_id: Uuid,
    service: IssueAppService,
}

async fn setup() -> TestService {
    let db = setup_database().await;
    let service = IssueAppService::new(db.app_store.clone());
    let auth = AuthContext {
        workspace_id: db.workspace_a.into(),
        actor_id: Uuid::now_v7(),
    };
    let project_id = db.project_a;
    TestService {
        _db: db,
        auth,
        project_id,
        service,
    }
}

#[tokio::test]
async fn rest_and_mcp_create_get_issue_have_equivalent_success_outcomes()
-> Result<(), Box<dyn std::error::Error>> {
    let test = setup().await;

    let rest_request = CreateIssueRequest {
        idempotency_key: "rest-success-1".to_owned(),
        project_id: test.project_id,
        milestone_id: None,
        title: "Parity success".to_owned(),
        description: Some("created via rest".to_owned()),
        priority: IssuePriorityDto::Medium,
    };

    let (rest_status, rest_create) =
        handle_rest_create_issue(&test.service, &test.auth, rest_request).await;

    assert_eq!(rest_status, 201);

    let created_issue_id = match rest_create {
        Envelope::Ok { data } => data.issue.id,
        Envelope::Err { error } => {
            return Err(format!("unexpected rest error: {error:?}").into());
        }
    };

    let rest_get = handle_rest_get_issue(&test.service, &test.auth, created_issue_id).await;
    let mcp_get = handle_mcp_get_issue(
        &test.service,
        &test.auth,
        json!({ "issue_id": created_issue_id }),
    )
    .await;

    let rest_title = match rest_get.1 {
        Envelope::Ok { data } => data.issue.title,
        Envelope::Err { error } => {
            return Err(format!("unexpected rest get error: {error:?}").into());
        }
    };

    let mcp_title = match mcp_get {
        Envelope::Ok { data } => data
            .get("issue")
            .and_then(|issue| issue.get("title"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        Envelope::Err { error } => {
            return Err(format!("unexpected mcp get error: {error:?}").into());
        }
    };

    assert_eq!(rest_title, mcp_title);
    Ok(())
}

#[tokio::test]
async fn rest_and_mcp_create_issue_have_equivalent_precondition_failures()
-> Result<(), Box<dyn std::error::Error>> {
    let test = setup().await;

    let bad_request = CreateIssueRequest {
        idempotency_key: "bad-project".to_owned(),
        project_id: Uuid::now_v7(),
        milestone_id: None,
        title: "Parity failure".to_owned(),
        description: None,
        priority: IssuePriorityDto::Low,
    };

    let (rest_status, rest_result) =
        handle_rest_create_issue(&test.service, &test.auth, bad_request.clone()).await;
    let mcp_result = handle_mcp_create_issue(&test.service, &test.auth, json!(bad_request)).await;

    assert_eq!(rest_status, 422);

    let rest_error_code = match rest_result {
        Envelope::Ok { data } => {
            return Err(format!("unexpected rest success: {data:?}").into());
        }
        Envelope::Err { error } => error.code,
    };

    let mcp_error_code = match mcp_result {
        Envelope::Ok { data } => {
            return Err(format!("unexpected mcp success: {data:?}").into());
        }
        Envelope::Err { error } => error.code,
    };

    assert_eq!(rest_error_code, "project_not_found");
    assert_eq!(mcp_error_code, "project_not_found");
    Ok(())
}

#[tokio::test]
async fn rest_and_mcp_get_issue_have_equivalent_not_found_failures()
-> Result<(), Box<dyn std::error::Error>> {
    let test = setup().await;

    let missing_id = Uuid::now_v7();

    let (rest_status, rest_result) =
        handle_rest_get_issue(&test.service, &test.auth, missing_id).await;
    let mcp_result =
        handle_mcp_get_issue(&test.service, &test.auth, json!({ "issue_id": missing_id })).await;

    assert_eq!(rest_status, 404);

    let rest_error_code = match rest_result {
        Envelope::Ok { data } => {
            return Err(format!("unexpected rest success: {data:?}").into());
        }
        Envelope::Err { error } => error.code,
    };

    let mcp_error_code = match mcp_result {
        Envelope::Ok { data } => {
            return Err(format!("unexpected mcp success: {data:?}").into());
        }
        Envelope::Err { error } => error.code,
    };

    assert_eq!(rest_error_code, "issue_not_found");
    assert_eq!(mcp_error_code, "issue_not_found");
    Ok(())
}

#[tokio::test]
async fn rest_and_mcp_create_issue_retry_replays_and_payload_drift_conflicts()
-> Result<(), Box<dyn std::error::Error>> {
    let test = setup().await;

    let request = CreateIssueRequest {
        idempotency_key: "retry-replay".to_owned(),
        project_id: test.project_id,
        milestone_id: None,
        title: "Replay me".to_owned(),
        description: Some("same payload".to_owned()),
        priority: IssuePriorityDto::Medium,
    };

    let (first_status, first) =
        handle_rest_create_issue(&test.service, &test.auth, request.clone()).await;
    assert_eq!(first_status, 201);
    let first_issue_id = match first {
        Envelope::Ok { data } => {
            assert!(!data.idempotent_replay);
            data.issue.id
        }
        Envelope::Err { error } => {
            return Err(format!("unexpected first rest error: {error:?}").into());
        }
    };

    let (retry_status, retry) =
        handle_rest_create_issue(&test.service, &test.auth, request.clone()).await;
    assert_eq!(retry_status, 201);
    match retry {
        Envelope::Ok { data } => {
            assert!(data.idempotent_replay);
            assert_eq!(data.issue.id, first_issue_id);
        }
        Envelope::Err { error } => {
            return Err(format!("unexpected rest retry error: {error:?}").into());
        }
    }

    let mcp_retry =
        handle_mcp_create_issue(&test.service, &test.auth, json!(request.clone())).await;
    match mcp_retry {
        Envelope::Ok { data } => {
            let replay = data
                .get("idempotent_replay")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let issue_id = data
                .get("issue")
                .and_then(|issue| issue.get("id"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            assert!(replay);
            assert_eq!(issue_id, first_issue_id.to_string());
        }
        Envelope::Err { error } => {
            return Err(format!("unexpected mcp retry error: {error:?}").into());
        }
    }

    let mut changed = request;
    changed.title = "Replay changed".to_owned();

    let (rest_conflict_status, rest_conflict) =
        handle_rest_create_issue(&test.service, &test.auth, changed.clone()).await;
    assert_eq!(rest_conflict_status, 409);
    let rest_conflict_code = match rest_conflict {
        Envelope::Ok { data } => {
            return Err(format!("unexpected rest conflict success: {data:?}").into());
        }
        Envelope::Err { error } => error.code,
    };
    assert_eq!(rest_conflict_code, "duplicate_issue_command");

    let mcp_conflict = handle_mcp_create_issue(&test.service, &test.auth, json!(changed)).await;
    let mcp_conflict_code = match mcp_conflict {
        Envelope::Ok { data } => {
            return Err(format!("unexpected mcp conflict success: {data:?}").into());
        }
        Envelope::Err { error } => error.code,
    };
    assert_eq!(mcp_conflict_code, "duplicate_issue_command");

    Ok(())
}
