mod support;

use std::time::Instant;

use chrono::{Duration, Utc};
use sqlx::query_scalar;
use uuid::Uuid;

use support::{auth_context, make_command, make_event, setup_db};

#[tokio::test]
async fn create_issue_write_path_meets_baseline_throughput_budget() {
    let db = setup_db().await;
    let auth = auth_context(db.workspace_a, Uuid::now_v7());
    let iterations: u32 = 32;

    let started = Instant::now();
    for n in 0..iterations {
        let command = make_command(
            db.workspace_a,
            db.project_a,
            Uuid::now_v7(),
            Uuid::now_v7(),
            format!("perf-write-{n}").as_str(),
        );
        let event = make_event(&command);
        db.app_store
            .create_issue_transactional(&auth, &command, &event)
            .await
            .expect("create issue");
    }
    let elapsed = started.elapsed();

    let per_op_ms = elapsed.as_secs_f64() * 1_000.0 / f64::from(iterations);
    assert!(
        per_op_ms < 250.0,
        "create_issue throughput regression: {per_op_ms:.2}ms/op"
    );
}

#[tokio::test]
async fn outbox_lease_and_delivery_meet_baseline_throughput_budget() {
    let db = setup_db().await;
    let auth = auth_context(db.workspace_a, Uuid::now_v7());
    let iterations: u32 = 48;

    for n in 0..iterations {
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
    loop {
        let leased = db
            .app_store
            .lease_outbox_batch(&auth, Utc::now(), Duration::seconds(30), 8)
            .await
            .expect("lease outbox");
        if leased.is_empty() {
            break;
        }
        for message in leased {
            db.app_store
                .mark_outbox_delivered(&auth, message.id, Utc::now())
                .await
                .expect("mark delivered");
            delivered += 1;
        }
    }
    let elapsed = started.elapsed();

    let per_op_ms = elapsed.as_secs_f64() * 1_000.0 / f64::from(delivered.max(1));
    assert!(
        per_op_ms < 200.0,
        "outbox lease/delivery regression: {per_op_ms:.2}ms/op"
    );
}

#[tokio::test]
async fn critical_phase1_indexes_exist_for_query_shape_guard() {
    let db = setup_db().await;

    let expected = [
        "idx_issue_workspace_project_state",
        "idx_outbox_poll_pending",
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
