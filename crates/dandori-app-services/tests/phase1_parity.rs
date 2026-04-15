use dandori_app_services::{
    IssueAppService, handle_mcp_create_issue, handle_mcp_get_issue, handle_rest_create_issue,
    handle_rest_get_issue,
};
use dandori_contract::{CreateIssueRequest, Envelope, IssuePriorityDto};
use dandori_domain::AuthContext;
use dandori_store::{PgStore, migrate_database};
use serde_json::json;
use sqlx::PgPool;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use uuid::Uuid;

struct TestService {
    _container: testcontainers::ContainerAsync<Postgres>,
    auth: AuthContext,
    project_id: Uuid,
    service: IssueAppService,
}

async fn setup() -> TestService {
    let container = Postgres::default().start().await.expect("start postgres");
    let host = container.get_host().await.expect("host");
    let port = container.get_host_port_ipv4(5432).await.expect("port");

    let admin_url = format!("postgres://postgres:postgres@{host}:{port}/postgres");
    migrate_database(&admin_url).await.expect("migrate");

    let admin_pool = PgPool::connect(&admin_url).await.expect("connect admin");

    sqlx::query(
        "DO $$
            BEGIN
                IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'dandori_app') THEN
                    CREATE ROLE dandori_app
                        LOGIN
                        PASSWORD 'dandori_app'
                        NOSUPERUSER
                        NOCREATEDB
                        NOCREATEROLE
                        NOBYPASSRLS;
                END IF;
            END
        $$;",
    )
    .execute(&admin_pool)
    .await
    .expect("create app role");

    sqlx::query("GRANT USAGE ON SCHEMA public TO dandori_app")
        .execute(&admin_pool)
        .await
        .expect("grant schema usage");

    sqlx::query(
        "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO dandori_app",
    )
    .execute(&admin_pool)
    .await
    .expect("grant table perms");

    let workspace_id = Uuid::now_v7();
    let workflow_id = Uuid::now_v7();
    let project_id = Uuid::now_v7();

    sqlx::query("INSERT INTO workspace (id, name) VALUES ($1, 'ws-a')")
        .bind(workspace_id)
        .execute(&admin_pool)
        .await
        .expect("seed workspace");

    sqlx::query(
        "INSERT INTO workflow_version (id, workspace_id, name, version, checksum, states, transitions)
         VALUES ($1, $2, 'default', 1, 'sha256:a', '[]'::jsonb, '[]'::jsonb)",
    )
    .bind(workflow_id)
    .bind(workspace_id)
    .execute(&admin_pool)
    .await
    .expect("seed workflow");

    sqlx::query(
        "INSERT INTO project (id, workspace_id, name, workflow_version_id)
         VALUES ($1, $2, 'project-a', $3)",
    )
    .bind(project_id)
    .bind(workspace_id)
    .bind(workflow_id)
    .execute(&admin_pool)
    .await
    .expect("seed project");

    let app_url = format!("postgres://dandori_app:dandori_app@{host}:{port}/postgres");
    let store = PgStore::connect(&app_url).await.expect("connect app store");
    let service = IssueAppService::new(store);

    TestService {
        _container: container,
        auth: AuthContext {
            workspace_id: workspace_id.into(),
            actor_id: Uuid::now_v7(),
        },
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
