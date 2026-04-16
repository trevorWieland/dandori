//! Fault-injection tests for the outbox worker.
//!
//! Cover classification, retry budget interaction, fail-closed publisher
//! policy, and the in-process circuit breaker.

use std::sync::Arc;

use dandori_app_services::{OutboxWorkerConfig, OutboxWorkerService};
use dandori_contract::{CreateIssueRequest, IssuePriorityDto};
use dandori_domain::AuthContext;
use dandori_test_support::{
    AlwaysOkPublisher, PermanentFailurePublisher, TestDatabase, TransientThenOkPublisher,
    setup_database,
};
use sqlx::query_scalar;
use uuid::Uuid;

fn base_config(instance_id: Uuid, workspace_id: Uuid) -> OutboxWorkerConfig {
    OutboxWorkerConfig {
        workspace_ids: Some(vec![workspace_id]),
        worker_instance_id: instance_id,
        batch_size: 10,
        lease_seconds: 30,
        max_attempts: 5,
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

async fn create_issue(db: &TestDatabase, auth: &AuthContext, idempotency_key: &str) {
    let service = dandori_app_services::IssueAppService::new(db.app_store.clone());
    service
        .create_issue(
            auth,
            CreateIssueRequest {
                idempotency_key: idempotency_key.to_owned(),
                project_id: db.project_a,
                milestone_id: None,
                title: "chaos issue".to_owned(),
                description: None,
                priority: IssuePriorityDto::Medium,
            },
        )
        .await
        .expect("create issue");
}

#[tokio::test]
async fn worker_retries_transient_failures_until_success() {
    let db = setup_database().await;
    let auth = AuthContext {
        workspace_id: db.workspace_a.into(),
        actor_id: Uuid::now_v7(),
    };
    create_issue(&db, &auth, "chaos-transient-then-ok").await;

    let publisher = TransientThenOkPublisher::new(2);
    let worker = OutboxWorkerService::new(
        db.app_store.clone(),
        base_config(auth.actor_id, auth.workspace_id.0),
        Arc::clone(&publisher) as Arc<dyn dandori_app_services::OutboxPublisher>,
    );

    // Run 1: transient failure, row goes back to failed.
    let report1 = worker.run_once().await.expect("run 1");
    assert_eq!(report1.delivered, 0);
    assert_eq!(report1.failed, 1);
    assert_eq!(report1.dead_lettered, 0);

    // Run 2: transient failure again.
    let report2 = worker.run_once().await.expect("run 2");
    assert_eq!(report2.delivered, 0);
    assert_eq!(report2.failed, 1);
    assert_eq!(report2.dead_lettered, 0);

    // Run 3: publisher succeeds.
    let report3 = worker.run_once().await.expect("run 3");
    assert_eq!(report3.delivered, 1);
    assert_eq!(report3.failed, 0);

    assert_eq!(publisher.total_calls(), 3);

    let status: String = query_scalar("SELECT status::text FROM outbox LIMIT 1")
        .fetch_one(&db.admin_pool)
        .await
        .expect("read outbox status");
    assert_eq!(status, "delivered");
}

#[tokio::test]
async fn worker_dead_letters_permanent_failure_on_first_attempt() {
    let db = setup_database().await;
    let auth = AuthContext {
        workspace_id: db.workspace_a.into(),
        actor_id: Uuid::now_v7(),
    };
    create_issue(&db, &auth, "chaos-permanent").await;

    let mut config = base_config(auth.actor_id, auth.workspace_id.0);
    config.max_attempts = 10; // prove classification, not attempt budget, triggers DLQ.
    let worker = OutboxWorkerService::new(
        db.app_store.clone(),
        config,
        Arc::new(PermanentFailurePublisher),
    );

    let report = worker.run_once().await.expect("run");
    assert_eq!(report.delivered, 0);
    assert_eq!(report.failed, 1);
    assert_eq!(report.dead_lettered, 1);

    let (status, attempts): (String, i32) =
        sqlx::query_as("SELECT status::text, attempts FROM outbox LIMIT 1")
            .fetch_one(&db.admin_pool)
            .await
            .expect("read outbox row");
    assert_eq!(status, "dead_letter");
    assert_eq!(attempts, 1);
}

#[tokio::test]
async fn worker_dead_letters_unsupported_event_type_on_first_attempt() {
    let db = setup_database().await;
    let auth = AuthContext {
        workspace_id: db.workspace_a.into(),
        actor_id: Uuid::now_v7(),
    };

    sqlx::query(
        "INSERT INTO outbox (
            id, workspace_id, event_id, event_type, aggregate_type, aggregate_id,
            occurred_at, correlation_id, payload, attempts, available_at, status,
            leased_at, leased_until, published_at, last_error, created_at, updated_at
        ) VALUES (
            $1, $2, $3, 'unsupported.chaos.v1', 'issue', $4,
            $5, $6, '{}'::jsonb, 0, $5, 'pending'::outbox_status,
            NULL, NULL, NULL, NULL, $5, $5
        )",
    )
    .bind(Uuid::now_v7())
    .bind(auth.workspace_id.0)
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .bind(chrono::Utc::now())
    .bind(Uuid::now_v7())
    .execute(&db.admin_pool)
    .await
    .expect("seed unsupported event");

    let mut config = base_config(auth.actor_id, auth.workspace_id.0);
    config.max_attempts = 10;
    let worker =
        OutboxWorkerService::new(db.app_store.clone(), config, Arc::new(AlwaysOkPublisher));

    let report = worker.run_once().await.expect("run");
    assert_eq!(report.dead_lettered, 1);
}

#[tokio::test]
async fn default_publisher_selection_fails_closed_without_publish_url() {
    let db = setup_database().await;
    let result = OutboxWorkerService::with_publisher_selection(
        db.app_store.clone(),
        base_config(Uuid::now_v7(), db.workspace_a),
        None,
        None,
    );
    assert!(
        result.is_err(),
        "missing publisher url must fail startup (fail-closed policy)"
    );
    let err = result.expect_err("publisher config must fail");
    assert_eq!(err.code, "publisher_not_configured");
}

#[tokio::test]
async fn default_publisher_selection_allows_noop_under_explicit_dev_override() {
    let db = setup_database().await;
    let worker = OutboxWorkerService::with_publisher_selection(
        db.app_store.clone(),
        base_config(Uuid::now_v7(), db.workspace_a),
        None,
        Some("1"),
    )
    .expect("should succeed under explicit noop override");
    let report = worker.run_once().await.expect("run");
    assert_eq!(report.leased, 0);
}

#[tokio::test]
async fn default_publisher_selection_uses_http_when_url_is_provided() {
    let db = setup_database().await;
    // An unreachable URL is fine for the wiring test; we never call .publish().
    let worker = OutboxWorkerService::with_publisher_selection(
        db.app_store.clone(),
        base_config(Uuid::now_v7(), db.workspace_a),
        Some("http://127.0.0.1:1/publish"),
        None,
    )
    .expect("should wire http publisher");
    let report = worker.run_once().await.expect("run");
    assert_eq!(report.leased, 0);
}
