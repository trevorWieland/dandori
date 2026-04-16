//! Concurrency proofs for the atomic idempotency contract.
//!
//! The write path uses `INSERT … ON CONFLICT … RETURNING (xmax = 0)` to
//! deterministically branch replay vs. conflict without a read-before-write
//! race. These tests spawn many concurrent writers to prove that:
//!
//! 1. Same key + same fingerprint → exactly one winner, all others replay.
//! 2. Same key + different fingerprint → exactly one winner, others return
//!    a typed `IdempotencyConflict` (no raw sqlx/sea_orm error leak).

use std::sync::Arc;

use dandori_domain::CommandName;
use dandori_store::{CreateIssueWriteResult, StoreError};
use dandori_test_support::{
    auth_context, make_create_issue_command, make_issue_created_event, setup_database,
};
use sqlx::query_scalar;
use uuid::Uuid;

const CONCURRENCY: usize = 16;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_same_fingerprint_writers_have_exactly_one_winner_and_others_replay() {
    let db = setup_database().await;
    let store = Arc::new(db.app_store.clone());
    let workspace = db.workspace_a;
    let project = db.project_a;
    let idempotency_key = "concurrent-race-match".to_owned();
    let actor_id = Uuid::now_v7();

    let mut handles = Vec::with_capacity(CONCURRENCY);
    for _ in 0..CONCURRENCY {
        let store = Arc::clone(&store);
        let key = idempotency_key.clone();
        handles.push(tokio::spawn(async move {
            let command = make_create_issue_command(
                workspace,
                project,
                Uuid::now_v7(),
                Uuid::now_v7(),
                key.as_str(),
            );
            let event = make_issue_created_event(&command);
            let auth = auth_context(workspace, actor_id);
            store
                .create_issue_transactional(&auth, &command, &event)
                .await
        }));
    }

    let mut results = Vec::with_capacity(CONCURRENCY);
    for handle in handles {
        results.push(handle.await.expect("join"));
    }

    let successes: Vec<CreateIssueWriteResult> = results
        .into_iter()
        .map(|outcome| outcome.expect("all matching-fingerprint writers must succeed"))
        .collect();

    let winners: Vec<_> = successes
        .iter()
        .filter(|result| !result.idempotent_replay)
        .collect();
    let replays: Vec<_> = successes
        .iter()
        .filter(|result| result.idempotent_replay)
        .collect();

    assert_eq!(
        winners.len(),
        1,
        "exactly one fresh write must win the race"
    );
    assert_eq!(replays.len(), CONCURRENCY - 1, "everyone else must replay");

    let winning_issue_id = winners[0].issue.id.0;
    for replay in &replays {
        assert_eq!(replay.issue.id.0, winning_issue_id);
    }

    let issue_count: i64 =
        query_scalar("SELECT COUNT(*) FROM issue WHERE workspace_id = $1 AND project_id = $2")
            .bind(workspace)
            .bind(project)
            .fetch_one(&db.admin_pool)
            .await
            .expect("count issues");
    assert_eq!(issue_count, 1, "only one issue row must exist");

    let outbox_count: i64 =
        query_scalar("SELECT COUNT(*) FROM outbox WHERE workspace_id = $1 AND event_type = $2")
            .bind(workspace)
            .bind("issue.created.v1")
            .fetch_one(&db.admin_pool)
            .await
            .expect("count outbox rows");
    assert_eq!(outbox_count, 1, "only one outbox row must exist");

    let idempotency_count: i64 = query_scalar(
        "SELECT COUNT(*) FROM idempotency_record
         WHERE workspace_id = $1 AND command_name = $2 AND idempotency_key = $3",
    )
    .bind(workspace)
    .bind(CommandName::IssueCreateV1.as_str())
    .bind(&idempotency_key)
    .fetch_one(&db.admin_pool)
    .await
    .expect("count idempotency rows");
    assert_eq!(
        idempotency_count, 1,
        "only one idempotency row must exist per key"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_different_fingerprint_writers_return_typed_conflict_deterministically() {
    let db = setup_database().await;
    let store = Arc::new(db.app_store.clone());
    let workspace = db.workspace_a;
    let project = db.project_a;
    let idempotency_key = "concurrent-race-mismatch".to_owned();
    let actor_id = Uuid::now_v7();

    let mut handles = Vec::with_capacity(CONCURRENCY);
    for i in 0..CONCURRENCY {
        let store = Arc::clone(&store);
        let key = idempotency_key.clone();
        handles.push(tokio::spawn(async move {
            let mut command = make_create_issue_command(
                workspace,
                project,
                Uuid::now_v7(),
                Uuid::now_v7(),
                key.as_str(),
            );
            // Give every writer a distinct fingerprint so at most one can
            // succeed; all others must deterministically see a conflict.
            command.request_fingerprint = format!("v2:writer-{i:02}");
            let event = make_issue_created_event(&command);
            let auth = auth_context(workspace, actor_id);
            store
                .create_issue_transactional(&auth, &command, &event)
                .await
        }));
    }

    let mut successes = 0;
    let mut conflicts = 0;
    let mut other_errors = Vec::new();
    for handle in handles {
        match handle.await.expect("join") {
            Ok(result) => {
                assert!(!result.idempotent_replay);
                successes += 1;
            }
            Err(StoreError::IdempotencyConflict) => {
                conflicts += 1;
            }
            Err(other) => other_errors.push(format!("{other:?}")),
        }
    }

    assert!(
        other_errors.is_empty(),
        "only IdempotencyConflict is allowed; saw: {other_errors:?}"
    );
    assert_eq!(successes, 1, "exactly one writer must succeed");
    assert_eq!(
        conflicts,
        CONCURRENCY - 1,
        "all other writers must see a deterministic conflict"
    );
}
