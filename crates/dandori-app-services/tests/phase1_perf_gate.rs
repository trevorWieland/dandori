use std::cmp::Ordering;
use std::sync::Arc;
use std::time::Instant;

use dandori_app_services::{IssueAppService, OutboxWorkerConfig, OutboxWorkerService};
use dandori_contract::{CreateIssueRequest, IssuePriorityDto};
use dandori_domain::AuthContext;
use dandori_store::PgStore;
use dandori_test_support::{AlwaysOkPublisher, TestDatabase, setup_database};
use uuid::Uuid;

struct TestService {
    _db: TestDatabase,
    auth: AuthContext,
    workspace_id: Uuid,
    project_id: Uuid,
    store: PgStore,
    service: IssueAppService,
}

async fn setup() -> TestService {
    let db = setup_database().await;
    let workspace_id = db.workspace_a;
    let project_id = db.project_a;
    let store = db.app_store.clone();
    let service = IssueAppService::new(store.clone());
    let auth = AuthContext {
        workspace_id: workspace_id.into(),
        actor_id: Uuid::now_v7(),
    };
    TestService {
        _db: db,
        auth,
        workspace_id,
        project_id,
        store,
        service,
    }
}

#[tokio::test]
async fn app_service_create_issue_meets_slo_p50_p95_and_throughput() {
    let test = setup().await;

    for n in 0..8_u32 {
        let request = CreateIssueRequest {
            idempotency_key: format!("warmup-app-create-{n}"),
            project_id: test.project_id,
            milestone_id: None,
            title: format!("warmup issue {n}"),
            description: Some("perf".to_owned()),
            priority: IssuePriorityDto::Medium,
        };
        test.service
            .create_issue(&test.auth, request)
            .await
            .expect("warmup create issue");
    }

    let iterations: u32 = 36;
    let started = Instant::now();
    let mut latencies_ms = Vec::with_capacity(usize::try_from(iterations).expect("usize"));

    for n in 0..iterations {
        let request = CreateIssueRequest {
            idempotency_key: format!("perf-app-create-{n}"),
            project_id: test.project_id,
            milestone_id: None,
            title: format!("perf issue {n}"),
            description: Some("perf".to_owned()),
            priority: IssuePriorityDto::Medium,
        };

        let op_started = Instant::now();
        test.service
            .create_issue(&test.auth, request)
            .await
            .expect("create issue");
        latencies_ms.push(op_started.elapsed().as_secs_f64() * 1_000.0);
    }

    let elapsed = started.elapsed();
    let p50 = percentile(&latencies_ms, 0.50);
    let p95 = percentile(&latencies_ms, 0.95);
    let throughput = f64::from(iterations) / elapsed.as_secs_f64().max(0.001);

    assert!(
        p50 < 250.0,
        "app-service create_issue p50 latency regression: {p50:.2}ms"
    );
    assert!(
        p95 < 450.0,
        "app-service create_issue p95 latency regression: {p95:.2}ms"
    );
    assert!(
        throughput > 2.0,
        "app-service create_issue throughput regression: {throughput:.2} ops/s"
    );
}

#[tokio::test]
async fn worker_run_once_meets_slo_p50_p95_and_throughput() {
    let test = setup().await;
    let iterations: u32 = 40;

    for n in 0..iterations {
        let request = CreateIssueRequest {
            idempotency_key: format!("perf-worker-{n}"),
            project_id: test.project_id,
            milestone_id: None,
            title: format!("perf worker issue {n}"),
            description: Some("perf".to_owned()),
            priority: IssuePriorityDto::Medium,
        };
        test.service
            .create_issue(&test.auth, request)
            .await
            .expect("create issue");
    }

    let mut latencies_ms = Vec::new();
    let mut total_delivered = 0_u32;
    let started = Instant::now();

    for _ in 0..6 {
        let config = OutboxWorkerConfig {
            workspace_ids: Some(vec![test.workspace_id]),
            worker_instance_id: test.auth.actor_id,
            batch_size: 16,
            lease_seconds: 30,
            max_attempts: 3,
            retry_backoff_seconds: 0,
            delivered_retention_hours: 1_000,
            dead_letter_retention_hours: 1_000,
            idempotency_retention_hours: 1_000,
            ..OutboxWorkerConfig::default()
        };
        let worker =
            OutboxWorkerService::new(test.store.clone(), config, Arc::new(AlwaysOkPublisher));

        let op_started = Instant::now();
        let report = worker.run_once().await.expect("worker run");
        latencies_ms.push(op_started.elapsed().as_secs_f64() * 1_000.0);
        total_delivered += u32::try_from(report.delivered).expect("u32");

        if report.leased == 0 {
            break;
        }
    }

    let elapsed = started.elapsed();
    let p50 = percentile(&latencies_ms, 0.50);
    let p95 = percentile(&latencies_ms, 0.95);
    let throughput = f64::from(total_delivered.max(1)) / elapsed.as_secs_f64().max(0.001);

    assert_eq!(total_delivered, iterations);
    assert!(
        p50 < 350.0,
        "worker run_once p50 latency regression: {p50:.2}ms"
    );
    assert!(
        p95 < 700.0,
        "worker run_once p95 latency regression: {p95:.2}ms"
    );
    assert!(
        throughput > 1.5,
        "worker throughput regression: {throughput:.2} ops/s"
    );
}

fn percentile(samples: &[f64], ratio: f64) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));

    if sorted.is_empty() {
        return 0.0;
    }

    let index = ((sorted.len() as f64 - 1.0) * ratio).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}
