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
                workflow_version_id: db.workflow_a,
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

    let auth_b = auth_context(db.workspace_b, Uuid::now_v7());
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
    let auth_b = auth_context(db.workspace_b, Uuid::now_v7());

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
async fn create_issue_is_idempotent_and_conflicts_on_changed_command_id() {
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

    let conflicting = make_command(
        db.workspace_a,
        db.project_a,
        Uuid::now_v7(),
        Uuid::now_v7(),
        "idem-write",
    );
    let conflicting_event = make_event(&conflicting);

    let error = db
        .app_store
        .create_issue_transactional(&auth, &conflicting, &conflicting_event)
        .await
        .expect_err("different command id with same key must conflict");

    assert!(matches!(error, StoreError::IdempotencyConflict));
}

#[tokio::test]
async fn failed_create_does_not_write_activity_or_outbox() {
    let db = setup_db().await;

    let bad_command = make_command(
        db.workspace_b,
        db.project_a,
        Uuid::now_v7(),
        Uuid::now_v7(),
        "idem-fail",
    );
    let event = make_event(&bad_command);
    let auth = auth_context(db.workspace_b, bad_command.actor_id);

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
async fn fault_injection_after_issue_insert_rolls_back_issue_activity_outbox_and_idempotency() {
    let db = setup_db().await;

    query(
        "CREATE OR REPLACE FUNCTION test_fail_outbox_insert() RETURNS trigger AS $$
            BEGIN
                RAISE EXCEPTION 'forced outbox failure';
            END;
        $$ LANGUAGE plpgsql",
    )
    .execute(&db.admin_pool)
    .await
    .expect("create test function");

    query(
        "CREATE TRIGGER test_fail_outbox_insert_trigger
         BEFORE INSERT ON outbox
         FOR EACH ROW
         EXECUTE FUNCTION test_fail_outbox_insert()",
    )
    .execute(&db.admin_pool)
    .await
    .expect("create test trigger");

    let command = make_command(
        db.workspace_a,
        db.project_a,
        Uuid::now_v7(),
        Uuid::now_v7(),
        "idem-fault-inject",
    );
    let event = make_event(&command);
    let auth = auth_context(db.workspace_a, command.actor_id);

    let _error = db
        .app_store
        .create_issue_transactional(&auth, &command, &event)
        .await
        .expect_err("fault injection should fail write path");

    let issue_count: i64 = query("SELECT COUNT(*) AS count FROM issue WHERE id = $1")
        .bind(command.issue_id.0)
        .fetch_one(&db.admin_pool)
        .await
        .expect("issue count")
        .get("count");

    let activity_count: i64 = query("SELECT COUNT(*) AS count FROM activity WHERE command_id = $1")
        .bind(command.command_id.0)
        .fetch_one(&db.admin_pool)
        .await
        .expect("activity count")
        .get("count");

    let outbox_count: i64 = query("SELECT COUNT(*) AS count FROM outbox WHERE correlation_id = $1")
        .bind(command.command_id.0)
        .fetch_one(&db.admin_pool)
        .await
        .expect("outbox count")
        .get("count");

    let idempotency_count: i64 = query(
        "SELECT COUNT(*) AS count
         FROM idempotency_record
         WHERE workspace_id = $1 AND command_name = $2 AND idempotency_key = $3",
    )
    .bind(command.workspace_id.0)
    .bind("issue.create.v1")
    .bind(command.idempotency_key.as_str())
    .fetch_one(&db.admin_pool)
    .await
    .expect("idempotency count")
    .get("count");

    assert_eq!(issue_count, 0);
    assert_eq!(activity_count, 0);
    assert_eq!(outbox_count, 0);
    assert_eq!(idempotency_count, 0);
}
