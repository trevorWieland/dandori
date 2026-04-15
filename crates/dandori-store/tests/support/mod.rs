use chrono::Utc;
use dandori_domain::{
    AuthContext, CommandId, CreateIssueCommandV1, IdempotencyKey, IssueCreatedEventV1, IssueId,
    IssuePriority,
};
use dandori_store::{PgStore, migrate_database};
use sqlx::PgPool;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use uuid::Uuid;

#[derive(Debug)]
pub(crate) struct TestDatabase {
    pub _container: testcontainers::ContainerAsync<Postgres>,
    pub admin_pool: PgPool,
    pub app_store: PgStore,
    pub workspace_a: Uuid,
    pub _workspace_b: Uuid,
    pub _workflow_a: Uuid,
    pub project_a: Uuid,
    pub _project_b: Uuid,
}

pub(crate) async fn setup_db() -> TestDatabase {
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
        _workspace_b: workspace_b,
        _workflow_a: workflow_a,
        project_a,
        _project_b: project_b,
    }
}

pub(crate) fn make_command(
    workspace_id: Uuid,
    project_id: Uuid,
    issue_id: Uuid,
    command_id: Uuid,
    idempotency_key: &str,
) -> CreateIssueCommandV1 {
    CreateIssueCommandV1 {
        command_id: CommandId(command_id),
        idempotency_key: IdempotencyKey(idempotency_key.to_owned()),
        request_fingerprint: format!("fingerprint:{workspace_id}:{project_id}:{idempotency_key}"),
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

pub(crate) fn make_event(command: &CreateIssueCommandV1) -> IssueCreatedEventV1 {
    IssueCreatedEventV1 {
        event_id: Uuid::now_v7(),
        issue_id: command.issue_id,
        workspace_id: command.workspace_id,
        project_id: command.project_id,
        milestone_id: command.milestone_id,
        actor_id: command.actor_id,
        occurred_at: Utc::now(),
        title: command.title.clone(),
        description: command.description.clone(),
        priority: command.priority,
    }
}

pub(crate) fn auth_context(workspace_id: Uuid, actor_id: Uuid) -> AuthContext {
    AuthContext {
        workspace_id: workspace_id.into(),
        actor_id,
    }
}
