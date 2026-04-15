use dandori_domain::{
    AuthContext, CommandId, CreateIssueCommandV1, IdempotencyKey, IssueCreatedEventV1, IssueId,
    IssuePriority,
};
use dandori_store::{PgStore, StoreError, migrate_database};
use sqlx::{PgPool, Row};
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use uuid::Uuid;

struct TestDatabase {
    _container: testcontainers::ContainerAsync<Postgres>,
    admin_pool: PgPool,
    app_store: PgStore,
    workspace_a: Uuid,
    workspace_b: Uuid,
    project_a: Uuid,
    project_b: Uuid,
}

async fn setup_db() -> TestDatabase {
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

    let workspace_a = Uuid::now_v7();
    let workspace_b = Uuid::now_v7();
    let workflow_a = Uuid::now_v7();
    let workflow_b = Uuid::now_v7();
    let project_a = Uuid::now_v7();
    let project_b = Uuid::now_v7();

    sqlx::query("INSERT INTO workspace (id, name) VALUES ($1, 'ws-a'), ($2, 'ws-b')")
        .bind(workspace_a)
        .bind(workspace_b)
        .execute(&admin_pool)
        .await
        .expect("seed workspace");

    sqlx::query(
        "INSERT INTO workflow_version (id, workspace_id, name, version, checksum, states, transitions)
         VALUES
            ($1, $2, 'default', 1, 'sha256:a', '[]'::jsonb, '[]'::jsonb),
            ($3, $4, 'default', 1, 'sha256:b', '[]'::jsonb, '[]'::jsonb)",
    )
    .bind(workflow_a)
    .bind(workspace_a)
    .bind(workflow_b)
    .bind(workspace_b)
    .execute(&admin_pool)
    .await
    .expect("seed workflow");

    sqlx::query(
        "INSERT INTO project (id, workspace_id, name, workflow_version_id)
         VALUES
            ($1, $2, 'project-a', $3),
            ($4, $5, 'project-b', $6)",
    )
    .bind(project_a)
    .bind(workspace_a)
    .bind(workflow_a)
    .bind(project_b)
    .bind(workspace_b)
    .bind(workflow_b)
    .execute(&admin_pool)
    .await
    .expect("seed project");

    let app_url = format!("postgres://dandori_app:dandori_app@{host}:{port}/postgres");
    let app_store = PgStore::connect(&app_url).await.expect("connect app");

    TestDatabase {
        _container: container,
        admin_pool,
        app_store,
        workspace_a,
        workspace_b,
        project_a,
        project_b,
    }
}

fn make_command(
    workspace_id: Uuid,
    project_id: Uuid,
    issue_id: Uuid,
    command_id: Uuid,
    idempotency_key: &str,
) -> CreateIssueCommandV1 {
    CreateIssueCommandV1 {
        command_id: CommandId(command_id),
        idempotency_key: IdempotencyKey(idempotency_key.to_owned()),
        issue_id: IssueId(issue_id),
        workspace_id: workspace_id.into(),
        project_id: project_id.into(),
        milestone_id: None,
        title: "Ship phase 1".to_owned(),
        description: Some("integration-test".to_owned()),
        priority: IssuePriority::High,
        actor_id: Uuid::now_v7(),
    }
}

fn make_event(command: &CreateIssueCommandV1) -> IssueCreatedEventV1 {
    IssueCreatedEventV1 {
        event_id: Uuid::now_v7(),
        issue_id: command.issue_id,
        workspace_id: command.workspace_id,
        project_id: command.project_id,
        milestone_id: command.milestone_id,
        actor_id: command.actor_id,
        occurred_at: chrono::Utc::now(),
        title: command.title.clone(),
        description: command.description.clone(),
        priority: command.priority,
    }
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

    let auth_a = AuthContext {
        workspace_id: db.workspace_a.into(),
        actor_id: command.actor_id,
    };
    let auth_b = AuthContext {
        workspace_id: db.workspace_b.into(),
        actor_id: Uuid::now_v7(),
    };

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

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workspace")
        .fetch_one(db.app_store.pool())
        .await
        .expect("count query without context");

    assert_eq!(count, 0);

    let insert_error = sqlx::query("INSERT INTO workspace (id, name) VALUES ($1, 'forbidden')")
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
    let auth = AuthContext {
        workspace_id: db.workspace_a.into(),
        actor_id: command.actor_id,
    };

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
    let auth = AuthContext {
        workspace_id: db.workspace_b.into(),
        actor_id: bad_command.actor_id,
    };

    let error = db
        .app_store
        .create_issue_transactional(&auth, &bad_command, &event)
        .await
        .expect_err("missing project precondition");

    assert!(matches!(error, StoreError::ProjectNotFound));

    let activity_count: i64 =
        sqlx::query("SELECT COUNT(*) AS count FROM activity WHERE command_id = $1")
            .bind(bad_command.command_id.0)
            .fetch_one(&db.admin_pool)
            .await
            .expect("activity count")
            .get("count");

    let outbox_count: i64 =
        sqlx::query("SELECT COUNT(*) AS count FROM outbox WHERE correlation_id = $1")
            .bind(bad_command.command_id.0)
            .fetch_one(&db.admin_pool)
            .await
            .expect("outbox count")
            .get("count");

    assert_eq!(activity_count, 0);
    assert_eq!(outbox_count, 0);

    let existing_project_b: i64 =
        sqlx::query("SELECT COUNT(*) AS count FROM project WHERE id = $1")
            .bind(db.project_b)
            .fetch_one(&db.admin_pool)
            .await
            .expect("project-b count")
            .get("count");

    assert_eq!(existing_project_b, 1);
}
