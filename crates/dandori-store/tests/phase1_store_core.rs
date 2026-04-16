mod support;

use dandori_store::{ProjectWriteInput, StoreError, WorkspaceWriteInput};
use sqlx::{Row, query, query_scalar};
use uuid::Uuid;

use support::{auth_context, make_command, make_event, setup_db};

#[tokio::test]
async fn workspace_and_project_repository_read_write_respect_tenant_context() {
    let db = setup_db().await;

    let auth_a = auth_context(db.workspace_a, Uuid::now_v7());

    let workspace = db
        .app_store
        .get_workspace(&auth_a, db.workspace_a)
        .await
        .expect("get workspace")
        .expect("workspace visible");
    assert_eq!(workspace.id.0, db.workspace_a);

    let created_project_id = Uuid::now_v7();
    let project = db
        .app_store
        .create_project(
            &auth_a,
            ProjectWriteInput {
                project_id: created_project_id,
                workspace_id: db.workspace_a,
                name: "project-created".to_owned(),
                workflow_version_id: db._workflow_a,
            },
        )
        .await
        .expect("create project");

    assert_eq!(project.id.0, created_project_id);

    let fetched_project = db
        .app_store
        .get_project(&auth_a, created_project_id)
        .await
        .expect("get project")
        .expect("project visible");

    assert_eq!(fetched_project.name, "project-created");

    let auth_b = auth_context(db._workspace_b, Uuid::now_v7());
    let cross_tenant = db
        .app_store
        .get_project(&auth_b, created_project_id)
        .await
        .expect("cross-tenant project read should not fail");

    assert!(cross_tenant.is_none());
}

#[tokio::test]
async fn create_workspace_works_when_auth_workspace_matches_inserted_id() {
    let db = setup_db().await;

    let workspace_id = Uuid::now_v7();
    let auth = auth_context(workspace_id, Uuid::now_v7());

    let created = db
        .app_store
        .create_workspace(
            &auth,
            WorkspaceWriteInput {
                workspace_id,
                name: "created-ws".to_owned(),
            },
        )
        .await
        .expect("create workspace");

    assert_eq!(created.id.0, workspace_id);

    let fetched = db
        .app_store
        .get_workspace(&auth, workspace_id)
        .await
        .expect("get workspace")
        .expect("workspace visible");

    assert_eq!(fetched.name, "created-ws");
}

#[tokio::test]
async fn create_project_requires_workflow_version_in_same_workspace() {
    let db = setup_db().await;
    let auth = auth_context(db.workspace_a, Uuid::now_v7());

    let error = db
        .app_store
        .create_project(
            &auth,
            ProjectWriteInput {
                project_id: Uuid::now_v7(),
                workspace_id: db.workspace_a,
                name: "bad-project".to_owned(),
                workflow_version_id: Uuid::now_v7(),
            },
        )
        .await
        .expect_err("missing workflow version should fail");

    assert!(matches!(error, StoreError::WorkflowVersionNotFound));
}

#[tokio::test]
async fn rls_allows_same_tenant_and_denies_cross_tenant_reads() {
    let db = setup_db().await;

    let command = make_command(
        db.workspace_a,
        db.project_a,
        Uuid::now_v7(),
        Uuid::now_v7(),
        "idem-read",
    );
    let event = make_event(&command);

    let auth_a = auth_context(db.workspace_a, command.actor_id);
    let auth_b = auth_context(db._workspace_b, Uuid::now_v7());

    db.app_store
        .create_issue_transactional(&auth_a, &command, &event)
        .await
        .expect("create issue");

    let same_tenant = db
        .app_store
        .get_issue(&auth_a, command.issue_id.0)
        .await
        .expect("get issue")
        .expect("visible in same tenant");

    assert_eq!(same_tenant.id.0, command.issue_id.0);

    let cross_tenant = db
        .app_store
        .get_issue(&auth_b, command.issue_id.0)
        .await
        .expect("cross tenant read should not fail");

    assert!(cross_tenant.is_none());
}

#[tokio::test]
async fn rls_denies_when_tenant_context_missing() {
    let db = setup_db().await;

    let count: i64 = query_scalar("SELECT COUNT(*) FROM workspace")
        .fetch_one(db.app_store.pool())
        .await
        .expect("count query without context");

    assert_eq!(count, 0);

    let insert_error = query("INSERT INTO workspace (id, name) VALUES ($1, 'forbidden')")
        .bind(Uuid::now_v7())
        .execute(db.app_store.pool())
        .await
        .expect_err("insert should fail without context");

    let code = insert_error
        .as_database_error()
        .and_then(|db_err| db_err.code())
        .map(|value| value.to_string())
        .unwrap_or_default();

    assert_eq!(code, "42501");
}

#[tokio::test]
async fn create_issue_is_idempotent_and_conflicts_on_changed_request_fingerprint() {
    let db = setup_db().await;

    let issue_id = Uuid::now_v7();
    let command_id = Uuid::now_v7();

    let command = make_command(
        db.workspace_a,
        db.project_a,
        issue_id,
        command_id,
        "idem-write",
    );
    let event = make_event(&command);
    let auth = auth_context(db.workspace_a, command.actor_id);

    let first = db
        .app_store
        .create_issue_transactional(&auth, &command, &event)
        .await
        .expect("first create should succeed");

    assert!(!first.idempotent_replay);

    let replay = db
        .app_store
        .create_issue_transactional(&auth, &command, &event)
        .await
        .expect("replay create should succeed");

    assert!(replay.idempotent_replay);
    assert_eq!(replay.issue.id.0, issue_id);

    let retry_like_command = make_command(
        db.workspace_a,
        db.project_a,
        Uuid::now_v7(),
        Uuid::now_v7(),
        "idem-write",
    );
    let retry_like_event = make_event(&retry_like_command);
    let retry_replay = db
        .app_store
        .create_issue_transactional(&auth, &retry_like_command, &retry_like_event)
        .await
        .expect("retry with same key+fingerprint should replay");

    assert!(retry_replay.idempotent_replay);
    assert_eq!(retry_replay.issue.id.0, issue_id);

    let mut conflicting = make_command(
        db.workspace_a,
        db.project_a,
        Uuid::now_v7(),
        Uuid::now_v7(),
        "idem-write",
    );
    conflicting.request_fingerprint = "changed-fingerprint".to_owned();
    let conflicting_event = make_event(&conflicting);

    let error = db
        .app_store
        .create_issue_transactional(&auth, &conflicting, &conflicting_event)
        .await
        .expect_err("different request fingerprint with same key must conflict");

    assert!(matches!(error, StoreError::IdempotencyConflict));
}

#[tokio::test]
async fn failed_create_does_not_write_activity_or_outbox() {
    let db = setup_db().await;

    let bad_command = make_command(
        db._workspace_b,
        db.project_a,
        Uuid::now_v7(),
        Uuid::now_v7(),
        "idem-fail",
    );
    let event = make_event(&bad_command);
    let auth = auth_context(db._workspace_b, bad_command.actor_id);

    let error = db
        .app_store
        .create_issue_transactional(&auth, &bad_command, &event)
        .await
        .expect_err("missing project precondition");

    assert!(matches!(error, StoreError::ProjectNotFound));

    let activity_count: i64 = query("SELECT COUNT(*) AS count FROM activity WHERE command_id = $1")
        .bind(bad_command.command_id.0)
        .fetch_one(&db.admin_pool)
        .await
        .expect("activity count")
        .get("count");

    let outbox_count: i64 = query("SELECT COUNT(*) AS count FROM outbox WHERE correlation_id = $1")
        .bind(bad_command.command_id.0)
        .fetch_one(&db.admin_pool)
        .await
        .expect("outbox count")
        .get("count");

    assert_eq!(activity_count, 0);
    assert_eq!(outbox_count, 0);
}

#[tokio::test]
async fn create_issue_enforces_milestone_workspace_and_project_consistency() {
    let db = setup_db().await;
    let auth = auth_context(db.workspace_a, Uuid::now_v7());

    let missing_milestone = make_command(
        db.workspace_a,
        db.project_a,
        Uuid::now_v7(),
        Uuid::now_v7(),
        "idem-missing-milestone",
    );
    let mut missing_milestone = missing_milestone;
    missing_milestone.milestone_id = Some(Uuid::now_v7().into());
    let missing_event = make_event(&missing_milestone);
    let missing_error = db
        .app_store
        .create_issue_transactional(&auth, &missing_milestone, &missing_event)
        .await
        .expect_err("missing milestone must fail");
    assert!(matches!(missing_error, StoreError::MilestoneNotFound));

    let milestone_id = Uuid::now_v7();
    query(
        "INSERT INTO milestone (id, workspace_id, project_id, title)
         VALUES ($1, $2, $3, 'ms-a')",
    )
    .bind(milestone_id)
    .bind(db.workspace_a)
    .bind(db.project_a)
    .execute(&db.admin_pool)
    .await
    .expect("seed milestone");

    let other_project_id = Uuid::now_v7();
    query(
        "INSERT INTO project (id, workspace_id, name, workflow_version_id)
         VALUES ($1, $2, 'project-a-2', $3)",
    )
    .bind(other_project_id)
    .bind(db.workspace_a)
    .bind(db._workflow_a)
    .execute(&db.admin_pool)
    .await
    .expect("seed second project in same workspace");

    let mismatch_command = make_command(
        db.workspace_a,
        other_project_id,
        Uuid::now_v7(),
        Uuid::now_v7(),
        "idem-mismatch-milestone",
    );
    let mut mismatch_command = mismatch_command;
    mismatch_command.milestone_id = Some(milestone_id.into());
    let mismatch_event = make_event(&mismatch_command);

    let mismatch_error = db
        .app_store
        .create_issue_transactional(&auth, &mismatch_command, &mismatch_event)
        .await
        .expect_err("mismatched milestone/project must fail");
    assert!(matches!(
        mismatch_error,
        StoreError::MilestoneProjectMismatch
    ));
}

#[tokio::test]
async fn composite_foreign_keys_block_cross_tenant_relations() {
    let db = setup_db().await;

    let cross_tenant_project_insert = query(
        "INSERT INTO project (id, workspace_id, name, workflow_version_id)
         VALUES ($1, $2, 'cross-tenant-project', $3)",
    )
    .bind(Uuid::now_v7())
    .bind(db.workspace_a)
    .bind(Uuid::now_v7())
    .execute(&db.admin_pool)
    .await
    .expect_err("cross-tenant workflow linkage must fail");

    let project_fk_code = cross_tenant_project_insert
        .as_database_error()
        .and_then(|db_err| db_err.code())
        .map(|value| value.to_string())
        .unwrap_or_default();
    assert_eq!(project_fk_code, "23503");

    let cross_tenant_issue_insert = query(
        "INSERT INTO issue (
            id, workspace_id, project_id, milestone_id, title, description, state_category, priority
         ) VALUES ($1, $2, $3, NULL, 'bad', NULL, 'open'::issue_state_category, 'low'::issue_priority)",
    )
    .bind(Uuid::now_v7())
    .bind(db.workspace_a)
    .bind(db._project_b)
    .execute(&db.admin_pool)
    .await
    .expect_err("cross-tenant issue->project linkage must fail");

    let issue_fk_code = cross_tenant_issue_insert
        .as_database_error()
        .and_then(|db_err| db_err.code())
        .map(|value| value.to_string())
        .unwrap_or_default();
    assert_eq!(issue_fk_code, "23503");
}

#[tokio::test]
async fn issue_create_rejects_oversized_description_with_database_constraint() {
    let db = setup_db().await;
    let auth = auth_context(db.workspace_a, Uuid::now_v7());
    let mut command = make_command(
        db.workspace_a,
        db.project_a,
        Uuid::now_v7(),
        Uuid::now_v7(),
        "oversized-description",
    );
    command.description = Some("x".repeat(4001));
    let event = make_event(&command);

    let error = db
        .app_store
        .create_issue_transactional(&auth, &command, &event)
        .await
        .expect_err("oversized description must violate check constraint");

    assert!(matches!(error, StoreError::SeaOrm(_) | StoreError::Sqlx(_)));
}

#[tokio::test]
async fn issue_create_rejects_oversized_fingerprint_with_database_constraint() {
    let db = setup_db().await;
    let auth = auth_context(db.workspace_a, Uuid::now_v7());
    let mut command = make_command(
        db.workspace_a,
        db.project_a,
        Uuid::now_v7(),
        Uuid::now_v7(),
        "oversized-fingerprint",
    );
    command.request_fingerprint = "x".repeat(129);
    let event = make_event(&command);

    let error = db
        .app_store
        .create_issue_transactional(&auth, &command, &event)
        .await
        .expect_err("oversized fingerprint must violate check constraint");

    assert!(matches!(error, StoreError::SeaOrm(_) | StoreError::Sqlx(_)));
}
