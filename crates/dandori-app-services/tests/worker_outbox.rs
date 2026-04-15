use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Duration, Utc};
use dandori_app_services::{
    OutboxPublisher, OutboxWorkerConfig, OutboxWorkerService, PublishError, PublishErrorKind,
};
use dandori_domain::{
    AuthContext, CommandId, CreateIssueCommandV1, IdempotencyKey, IssueCreatedEventV1, IssueId,
    IssuePriority,
};
use dandori_store::{PgStore, migrate_database};
use sqlx::PgPool;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use uuid::Uuid;

struct TestWorkerDb {
    _container: testcontainers::ContainerAsync<Postgres>,
    admin_pool: PgPool,
    store: PgStore,
    auth: AuthContext,
    project_id: Uuid,
}

#[derive(Debug)]
struct AlwaysOkPublisher;

#[async_trait]
impl OutboxPublisher for AlwaysOkPublisher {
    async fn publish_issue_created(
        &self,
        _message: &dandori_store::OutboxMessage,
        _event: &IssueCreatedEventV1,
    ) -> Result<(), PublishError> {
        Ok(())
    }
}

#[derive(Debug)]
struct TransientFailurePublisher;

#[async_trait]
impl OutboxPublisher for TransientFailurePublisher {
    async fn publish_issue_created(
        &self,
        _message: &dandori_store::OutboxMessage,
        _event: &IssueCreatedEventV1,
    ) -> Result<(), PublishError> {
        Err(PublishError {
            kind: PublishErrorKind::Transient,
            message: "temporary downstream outage".to_owned(),
        })
    }
}

#[derive(Debug)]
struct PermanentFailurePublisher;

#[async_trait]
impl OutboxPublisher for PermanentFailurePublisher {
    async fn publish_issue_created(
        &self,
        _message: &dandori_store::OutboxMessage,
        _event: &IssueCreatedEventV1,
    ) -> Result<(), PublishError> {
        Err(PublishError {
            kind: PublishErrorKind::Permanent,
            message: "permanent downstream rejection".to_owned(),
        })
    }
}

async fn setup() -> TestWorkerDb {
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

    sqlx::query("INSERT INTO workspace (id, name) VALUES ($1, 'worker-ws')")
        .bind(workspace_id)
        .execute(&admin_pool)
        .await
        .expect("seed workspace");

    sqlx::query(
        "INSERT INTO workflow_version (id, workspace_id, name, version, checksum, states, transitions)
         VALUES ($1, $2, 'default', 1, 'sha256:worker', '[]'::jsonb, '[]'::jsonb)",
    )
    .bind(workflow_id)
    .bind(workspace_id)
    .execute(&admin_pool)
    .await
    .expect("seed workflow");

    sqlx::query(
        "INSERT INTO project (id, workspace_id, name, workflow_version_id)
         VALUES ($1, $2, 'worker-project', $3)",
    )
    .bind(project_id)
    .bind(workspace_id)
    .bind(workflow_id)
    .execute(&admin_pool)
    .await
    .expect("seed project");

    let app_url = format!("postgres://dandori_app:dandori_app@{host}:{port}/postgres");
    let store = PgStore::connect(&app_url).await.expect("connect store");

    TestWorkerDb {
        _container: container,
        admin_pool,
        store,
        auth: AuthContext {
            workspace_id: workspace_id.into(),
            actor_id: Uuid::now_v7(),
        },
        project_id,
    }
}

fn make_command(
    auth: &AuthContext,
    project_id: Uuid,
    idempotency_key: &str,
) -> CreateIssueCommandV1 {
    CreateIssueCommandV1 {
        command_id: CommandId(Uuid::now_v7()),
        idempotency_key: IdempotencyKey(idempotency_key.to_owned()),
        request_fingerprint: format!("worker-fingerprint:{project_id}:{idempotency_key}"),
        issue_id: IssueId(Uuid::now_v7()),
        workspace_id: auth.workspace_id,
        project_id: project_id.into(),
        milestone_id: None,
        title: "worker issue".to_owned(),
        description: Some("worker-path".to_owned()),
        priority: IssuePriority::Medium,
        actor_id: auth.actor_id,
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
        occurred_at: Utc::now(),
        title: command.title.clone(),
        description: command.description.clone(),
        priority: command.priority,
    }
}

#[tokio::test]
async fn worker_delivers_known_issue_created_outbox_message() {
    let db = setup().await;
    let command = make_command(&db.auth, db.project_id, "worker-success");
    let event = make_event(&command);

    db.store
        .create_issue_transactional(&db.auth, &command, &event)
        .await
        .expect("issue create");

    let worker = OutboxWorkerService::new(
        db.store.clone(),
        OutboxWorkerConfig {
            workspace_id: db.auth.workspace_id.0,
            actor_id: db.auth.actor_id,
            batch_size: 10,
            lease_seconds: 30,
            max_attempts: 3,
            retry_backoff_seconds: 0,
            delivered_retention_hours: 1_000,
            dead_letter_retention_hours: 1_000,
            idempotency_retention_hours: 1_000,
        },
        Arc::new(AlwaysOkPublisher),
    );

    let report = worker.run_once().await.expect("worker run");
    assert_eq!(report.leased, 1);
    assert_eq!(report.delivered, 1);
    assert_eq!(report.failed, 0);
    assert_eq!(report.dead_lettered, 0);
}

#[tokio::test]
async fn worker_retries_and_dead_letters_unknown_event_type() {
    let db = setup().await;

    sqlx::query(
        "INSERT INTO outbox (
            id, workspace_id, event_id, event_type, aggregate_type, aggregate_id,
            occurred_at, correlation_id, payload, attempts, available_at, status,
            leased_at, leased_until, published_at, last_error, created_at, updated_at
        ) VALUES (
            $1, $2, $3, 'unsupported.event.v1', 'issue', $4,
            $5, $6, '{}'::jsonb, 0, $5, 'pending'::outbox_status,
            NULL, NULL, NULL, NULL, $5, $5
        )",
    )
    .bind(Uuid::now_v7())
    .bind(db.auth.workspace_id.0)
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .bind(Utc::now())
    .bind(Uuid::now_v7())
    .execute(&db.admin_pool)
    .await
    .expect("insert unsupported outbox event");

    let worker = OutboxWorkerService::new(
        db.store.clone(),
        OutboxWorkerConfig {
            workspace_id: db.auth.workspace_id.0,
            actor_id: db.auth.actor_id,
            batch_size: 10,
            lease_seconds: 30,
            max_attempts: 1,
            retry_backoff_seconds: 0,
            delivered_retention_hours: 1_000,
            dead_letter_retention_hours: 1_000,
            idempotency_retention_hours: 1_000,
        },
        Arc::new(AlwaysOkPublisher),
    );

    let report = worker.run_once().await.expect("worker run");
    assert_eq!(report.leased, 1);
    assert_eq!(report.delivered, 0);
    assert_eq!(report.failed, 1);
    assert_eq!(report.dead_lettered, 1);

    let status: String = sqlx::query_scalar("SELECT status::text FROM outbox LIMIT 1")
        .fetch_one(&db.admin_pool)
        .await
        .expect("read outbox status");
    assert_eq!(status, "dead_letter");

    let cleaned = db
        .store
        .cleanup_outbox(
            &db.auth,
            Utc::now() + Duration::hours(1),
            Utc::now() + Duration::hours(1),
        )
        .await
        .expect("cleanup outbox");
    assert_eq!(cleaned, 1);
}

#[tokio::test]
async fn worker_marks_transient_publish_failures_as_failed_for_retry() {
    let db = setup().await;
    let command = make_command(&db.auth, db.project_id, "worker-transient-failure");
    let event = make_event(&command);

    db.store
        .create_issue_transactional(&db.auth, &command, &event)
        .await
        .expect("issue create");

    let worker = OutboxWorkerService::new(
        db.store.clone(),
        OutboxWorkerConfig {
            workspace_id: db.auth.workspace_id.0,
            actor_id: db.auth.actor_id,
            batch_size: 10,
            lease_seconds: 30,
            max_attempts: 3,
            retry_backoff_seconds: 0,
            delivered_retention_hours: 1_000,
            dead_letter_retention_hours: 1_000,
            idempotency_retention_hours: 1_000,
        },
        Arc::new(TransientFailurePublisher),
    );

    let report = worker.run_once().await.expect("worker run");
    assert_eq!(report.leased, 1);
    assert_eq!(report.delivered, 0);
    assert_eq!(report.failed, 1);
    assert_eq!(report.dead_lettered, 0);

    let status: String = sqlx::query_scalar("SELECT status::text FROM outbox LIMIT 1")
        .fetch_one(&db.admin_pool)
        .await
        .expect("read outbox status");
    assert_eq!(status, "failed");
}

#[tokio::test]
async fn worker_marks_permanent_publish_failures_dead_letter_when_attempt_budget_is_one() {
    let db = setup().await;
    let command = make_command(&db.auth, db.project_id, "worker-permanent-failure");
    let event = make_event(&command);

    db.store
        .create_issue_transactional(&db.auth, &command, &event)
        .await
        .expect("issue create");

    let worker = OutboxWorkerService::new(
        db.store.clone(),
        OutboxWorkerConfig {
            workspace_id: db.auth.workspace_id.0,
            actor_id: db.auth.actor_id,
            batch_size: 10,
            lease_seconds: 30,
            max_attempts: 1,
            retry_backoff_seconds: 0,
            delivered_retention_hours: 1_000,
            dead_letter_retention_hours: 1_000,
            idempotency_retention_hours: 1_000,
        },
        Arc::new(PermanentFailurePublisher),
    );

    let report = worker.run_once().await.expect("worker run");
    assert_eq!(report.leased, 1);
    assert_eq!(report.delivered, 0);
    assert_eq!(report.failed, 1);
    assert_eq!(report.dead_lettered, 1);

    let status: String = sqlx::query_scalar("SELECT status::text FROM outbox LIMIT 1")
        .fetch_one(&db.admin_pool)
        .await
        .expect("read outbox status");
    assert_eq!(status, "dead_letter");
}
