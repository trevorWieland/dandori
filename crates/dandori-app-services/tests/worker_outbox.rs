use std::sync::Arc;

use chrono::{Duration, Utc};
use dandori_app_services::{OutboxWorkerConfig, OutboxWorkerService};
use dandori_contract::{CreateIssueRequest, IssuePriorityDto};
use dandori_domain::AuthContext;
use dandori_test_support::{
    AlwaysOkPublisher, PermanentFailurePublisher, TestDatabase, TransientFailurePublisher,
    setup_database,
};
use sqlx::query_scalar;
use uuid::Uuid;

struct TestWorkerDb {
    db: TestDatabase,
    auth: AuthContext,
    workspace_b_auth: AuthContext,
    project_a: Uuid,
    project_b: Uuid,
}

async fn setup_worker_db() -> TestWorkerDb {
    let db = setup_database().await;
    let auth = AuthContext {
        workspace_id: db.workspace_a.into(),
        actor_id: Uuid::now_v7(),
    };
    let workspace_b_auth = AuthContext {
        workspace_id: db.workspace_b.into(),
        actor_id: Uuid::now_v7(),
    };
    let project_a = db.project_a;
    let project_b = db.project_b;
    TestWorkerDb {
        db,
        auth,
        workspace_b_auth,
        project_a,
        project_b,
    }
}

async fn create_issue_via_service(
    auth: &AuthContext,
    store: &dandori_store::PgStore,
    project_id: Uuid,
    idempotency_key: &str,
) {
    let service = dandori_app_services::IssueAppService::new(store.clone());
    service
        .create_issue(
            auth,
            CreateIssueRequest {
                idempotency_key: idempotency_key.to_owned(),
                project_id,
                milestone_id: None,
                title: "worker issue".to_owned(),
                description: Some("worker-path".to_owned()),
                priority: IssuePriorityDto::Medium,
            },
        )
        .await
        .expect("issue create");
}

fn base_config(instance_id: Uuid, workspace_ids: Vec<Uuid>) -> OutboxWorkerConfig {
    OutboxWorkerConfig {
        workspace_ids: Some(workspace_ids),
        worker_instance_id: instance_id,
        batch_size: 10,
        lease_seconds: 30,
        max_attempts: 3,
        retry_backoff_seconds: 0,
        delivered_retention_hours: 1_000,
        dead_letter_retention_hours: 1_000,
        idempotency_retention_hours: 1_000,
        publish_concurrency: 4,
        retry_jitter_ms: 0,
        circuit_failure_threshold: 0,
        ..OutboxWorkerConfig::default()
    }
}

#[tokio::test]
async fn worker_delivers_known_issue_created_outbox_message() {
    let ctx = setup_worker_db().await;
    create_issue_via_service(
        &ctx.auth,
        &ctx.db.app_store,
        ctx.project_a,
        "worker-success",
    )
    .await;

    let worker = OutboxWorkerService::new(
        ctx.db.app_store.clone(),
        base_config(ctx.auth.actor_id, vec![ctx.auth.workspace_id.0]),
        Arc::new(AlwaysOkPublisher),
    );

    let report = worker.run_once().await.expect("worker run");
    assert_eq!(report.leased, 1);
    assert_eq!(report.delivered, 1);
    assert_eq!(report.failed, 0);
    assert_eq!(report.dead_lettered, 0);
}

#[tokio::test]
async fn worker_dead_letters_unknown_event_type_immediately() {
    let ctx = setup_worker_db().await;

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
    .bind(ctx.auth.workspace_id.0)
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .bind(Utc::now())
    .bind(Uuid::now_v7())
    .execute(&ctx.db.admin_pool)
    .await
    .expect("insert unsupported outbox event");

    let mut config = base_config(ctx.auth.actor_id, vec![ctx.auth.workspace_id.0]);
    // A large max_attempts budget proves terminal classification dead-letters
    // before the budget runs out.
    config.max_attempts = 10;
    let worker = OutboxWorkerService::new(
        ctx.db.app_store.clone(),
        config,
        Arc::new(AlwaysOkPublisher),
    );

    let report = worker.run_once().await.expect("worker run");
    assert_eq!(report.leased, 1);
    assert_eq!(report.delivered, 0);
    assert_eq!(report.failed, 1);
    assert_eq!(report.dead_lettered, 1);

    let status: String = query_scalar("SELECT status::text FROM outbox LIMIT 1")
        .fetch_one(&ctx.db.admin_pool)
        .await
        .expect("read outbox status");
    assert_eq!(status, "dead_letter");

    let cleaned = ctx
        .db
        .app_store
        .cleanup_outbox(
            &ctx.auth,
            Utc::now() + Duration::hours(1),
            Utc::now() + Duration::hours(1),
        )
        .await
        .expect("cleanup outbox");
    assert_eq!(cleaned, 1);
}

#[tokio::test]
async fn worker_marks_transient_publish_failures_as_failed_for_retry() {
    let ctx = setup_worker_db().await;
    create_issue_via_service(
        &ctx.auth,
        &ctx.db.app_store,
        ctx.project_a,
        "worker-transient-failure",
    )
    .await;

    let worker = OutboxWorkerService::new(
        ctx.db.app_store.clone(),
        base_config(ctx.auth.actor_id, vec![ctx.auth.workspace_id.0]),
        Arc::new(TransientFailurePublisher),
    );

    let report = worker.run_once().await.expect("worker run");
    assert_eq!(report.leased, 1);
    assert_eq!(report.delivered, 0);
    assert_eq!(report.failed, 1);
    assert_eq!(report.dead_lettered, 0);

    let status: String = query_scalar("SELECT status::text FROM outbox LIMIT 1")
        .fetch_one(&ctx.db.admin_pool)
        .await
        .expect("read outbox status");
    assert_eq!(status, "failed");
}

#[tokio::test]
async fn worker_dead_letters_permanent_publish_failure_on_first_attempt() {
    let ctx = setup_worker_db().await;
    create_issue_via_service(
        &ctx.auth,
        &ctx.db.app_store,
        ctx.project_a,
        "worker-permanent-failure",
    )
    .await;

    let mut config = base_config(ctx.auth.actor_id, vec![ctx.auth.workspace_id.0]);
    // A large budget proves classification-based dead-letter is independent of
    // attempt-count exhaustion.
    config.max_attempts = 10;
    let worker = OutboxWorkerService::new(
        ctx.db.app_store.clone(),
        config,
        Arc::new(PermanentFailurePublisher),
    );

    let report = worker.run_once().await.expect("worker run");
    assert_eq!(report.leased, 1);
    assert_eq!(report.delivered, 0);
    assert_eq!(report.failed, 1);
    assert_eq!(report.dead_lettered, 1);

    let status: String = query_scalar("SELECT status::text FROM outbox LIMIT 1")
        .fetch_one(&ctx.db.admin_pool)
        .await
        .expect("read outbox status");
    assert_eq!(status, "dead_letter");
}

#[tokio::test]
async fn worker_processes_multiple_workspaces_in_one_run() {
    let ctx = setup_worker_db().await;
    create_issue_via_service(
        &ctx.auth,
        &ctx.db.app_store,
        ctx.project_a,
        "worker-multi-a",
    )
    .await;
    create_issue_via_service(
        &ctx.workspace_b_auth,
        &ctx.db.app_store,
        ctx.project_b,
        "worker-multi-b",
    )
    .await;

    let worker = OutboxWorkerService::new(
        ctx.db.app_store.clone(),
        base_config(
            ctx.auth.actor_id,
            vec![ctx.auth.workspace_id.0, ctx.workspace_b_auth.workspace_id.0],
        ),
        Arc::new(AlwaysOkPublisher),
    );

    let report = worker.run_once().await.expect("worker run");
    assert_eq!(report.leased, 2);
    assert_eq!(report.delivered, 2);
}
