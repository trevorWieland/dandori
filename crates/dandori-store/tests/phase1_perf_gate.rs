use std::cmp::Ordering;
use std::time::Instant;

use chrono::{Duration, Utc};
use dandori_test_support::{
    auth_context, make_create_issue_command as make_command,
    make_issue_created_event as make_event, setup_database as setup_db,
};
use sqlx::query_scalar;
use uuid::Uuid;

#[tokio::test]
async fn create_issue_write_path_meets_slo_p50_p95_and_throughput() {
    let db = setup_db().await;
    let auth = auth_context(db.workspace_a, Uuid::now_v7());

    for n in 0..8_u32 {
        let command = make_command(
            db.workspace_a,
            db.project_a,
            Uuid::now_v7(),
            Uuid::now_v7(),
            format!("warmup-write-{n}").as_str(),
        );
        let event = make_event(&command);
        db.app_store
            .create_issue_transactional(&auth, &command, &event)
            .await
            .expect("warmup create issue");
    }

    let iterations: u32 = 40;
    let started = Instant::now();
    let mut latencies_ms = Vec::with_capacity(usize::try_from(iterations).expect("usize"));

    for n in 0..iterations {
        let command = make_command(
            db.workspace_a,
            db.project_a,
            Uuid::now_v7(),
            Uuid::now_v7(),
            format!("perf-write-{n}").as_str(),
        );
        let event = make_event(&command);
        let op_started = Instant::now();
        db.app_store
            .create_issue_transactional(&auth, &command, &event)
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
        "create_issue p50 latency regression: {p50:.2}ms"
    );
    assert!(
        p95 < 450.0,
        "create_issue p95 latency regression: {p95:.2}ms"
    );
    assert!(
        throughput > 2.5,
        "create_issue throughput regression: {throughput:.2} ops/s"
    );
}

#[tokio::test]
async fn outbox_lease_and_delivery_meet_slo_p50_p95_and_throughput() {
    let db = setup_db().await;
    let auth = auth_context(db.workspace_a, Uuid::now_v7());

    for n in 0..64_u32 {
        let command = make_command(
            db.workspace_a,
            db.project_a,
            Uuid::now_v7(),
            Uuid::now_v7(),
            format!("perf-outbox-{n}").as_str(),
        );
        let event = make_event(&command);
        db.app_store
            .create_issue_transactional(&auth, &command, &event)
            .await
            .expect("create issue");
    }

    let started = Instant::now();
    let mut delivered = 0_u32;
    let mut latencies_ms = Vec::new();
    loop {
        let leased = db
            .app_store
            .lease_outbox_batch(&auth, Utc::now(), Duration::seconds(30), 16)
            .await
            .expect("lease outbox");
        if leased.is_empty() {
            break;
        }
        for message in leased {
            let op_started = Instant::now();
            db.app_store
                .mark_outbox_delivered(
                    &auth,
                    message.id,
                    message.lease_token,
                    message.lease_owner,
                    Utc::now(),
                )
                .await
                .expect("mark delivered");
            latencies_ms.push(op_started.elapsed().as_secs_f64() * 1_000.0);
            delivered += 1;
        }
    }

    let elapsed = started.elapsed();
    let p50 = percentile(&latencies_ms, 0.50);
    let p95 = percentile(&latencies_ms, 0.95);
    let throughput = f64::from(delivered.max(1)) / elapsed.as_secs_f64().max(0.001);

    assert!(
        p50 < 200.0,
        "outbox delivery p50 latency regression: {p50:.2}ms"
    );
    assert!(
        p95 < 400.0,
        "outbox delivery p95 latency regression: {p95:.2}ms"
    );
    assert!(
        throughput > 3.0,
        "outbox delivery throughput regression: {throughput:.2} ops/s"
    );
}

#[tokio::test]
async fn critical_phase1_indexes_exist_for_query_shape_guard() {
    let db = setup_db().await;

    let expected = [
        "idx_issue_workspace_project_state",
        "idx_outbox_poll_pending",
        "idx_outbox_lease_expiry",
        "idx_idempotency_fingerprint",
    ];

    for index_name in expected {
        let exists: bool = query_scalar(
            "SELECT EXISTS(
                SELECT 1
                FROM pg_indexes
                WHERE schemaname = 'public' AND indexname = $1
            )",
        )
        .bind(index_name)
        .fetch_one(&db.admin_pool)
        .await
        .expect("check index");

        assert!(exists, "missing expected index: {index_name}");
    }
}

#[tokio::test]
async fn outbox_poll_queries_use_tenant_leading_indexes() {
    let db = setup_db().await;
    let auth = auth_context(db.workspace_a, Uuid::now_v7());

    for n in 0..12_u32 {
        let command = make_command(
            db.workspace_a,
            db.project_a,
            Uuid::now_v7(),
            Uuid::now_v7(),
            format!("perf-explain-{n}").as_str(),
        );
        let event = make_event(&command);
        db.app_store
            .create_issue_transactional(&auth, &command, &event)
            .await
            .expect("create issue");
    }

    let pending_plan: Vec<String> = query_scalar(
        "EXPLAIN (COSTS OFF)
         SELECT id
         FROM outbox
         WHERE workspace_id = $1
           AND status IN ('pending'::outbox_status, 'failed'::outbox_status)
           AND available_at <= $2
         ORDER BY available_at, id
         LIMIT 10",
    )
    .bind(db.workspace_a)
    .bind(Utc::now())
    .fetch_all(&db.admin_pool)
    .await
    .expect("explain pending poll");

    assert!(
        pending_plan
            .iter()
            .any(|line| line.contains("idx_outbox_poll_pending")),
        "pending poll plan did not reference tenant-leading pending index: {pending_plan:?}"
    );

    let lease_plan: Vec<String> = query_scalar(
        "EXPLAIN (COSTS OFF)
         SELECT id
         FROM outbox
         WHERE workspace_id = $1
           AND status = 'leased'::outbox_status
           AND leased_until <= $2
         ORDER BY leased_until, id
         LIMIT 10",
    )
    .bind(db.workspace_a)
    .bind(Utc::now())
    .fetch_all(&db.admin_pool)
    .await
    .expect("explain lease recovery poll");

    assert!(
        lease_plan
            .iter()
            .any(|line| line.contains("idx_outbox_lease_expiry")),
        "lease recovery poll plan did not reference tenant-leading lease index: {lease_plan:?}"
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
