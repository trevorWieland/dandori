mod support;

use chrono::{Duration, Utc};
use sqlx::query;
use uuid::Uuid;

use support::{auth_context, make_command, make_event, setup_db};

#[tokio::test]
async fn outbox_lease_retry_dead_letter_and_retention_flow() {
    let db = setup_db().await;
    let _ = (db.workspace_b, db.workflow_a);

    let command = make_command(
        db.workspace_a,
        db.project_a,
        Uuid::now_v7(),
        Uuid::now_v7(),
        "idem-outbox-flow",
    );
    let event = make_event(&command);

    let auth = auth_context(db.workspace_a, command.actor_id);

    db.app_store
        .create_issue_transactional(&auth, &command, &event)
        .await
        .expect("create issue should queue outbox record");

    let first_lease = db
        .app_store
        .lease_outbox_batch(&auth, Utc::now(), Duration::seconds(30), 10)
        .await
        .expect("lease pending record");

    assert_eq!(first_lease.len(), 1);

    let leased = &first_lease[0];
    db.app_store
        .mark_outbox_failed(
            &auth,
            leased.id,
            Utc::now(),
            "first failure",
            2,
            Duration::seconds(0),
        )
        .await
        .expect("mark failed");

    let second_lease = db
        .app_store
        .lease_outbox_batch(&auth, Utc::now(), Duration::seconds(30), 10)
        .await
        .expect("lease failed record again");

    assert_eq!(second_lease.len(), 1);

    db.app_store
        .mark_outbox_failed(
            &auth,
            second_lease[0].id,
            Utc::now(),
            "second failure",
            2,
            Duration::seconds(0),
        )
        .await
        .expect("mark dead letter");

    let dead_letter_status: String =
        sqlx::query_scalar("SELECT status::text FROM outbox WHERE id = $1")
            .bind(second_lease[0].id)
            .fetch_one(&db.admin_pool)
            .await
            .expect("read dead-letter status");

    assert_eq!(dead_letter_status, "dead_letter");

    let deleted_outbox = db
        .app_store
        .cleanup_outbox(
            &auth,
            Utc::now() + Duration::hours(1),
            Utc::now() + Duration::hours(1),
        )
        .await
        .expect("cleanup outbox rows");

    assert_eq!(deleted_outbox, 1);
}

#[tokio::test]
async fn idempotency_cleanup_deletes_expired_rows() {
    let db = setup_db().await;
    let _ = (db.workspace_b, db.workflow_a);

    query(
        "INSERT INTO idempotency_record (
            workspace_id,
            command_name,
            idempotency_key,
            command_id,
            response_payload,
            expires_at,
            created_at
        ) VALUES ($1, $2, $3, $4, '{}'::jsonb, $5, $6)",
    )
    .bind(db.workspace_a)
    .bind("issue.create.v1")
    .bind("expired-key")
    .bind(Uuid::now_v7())
    .bind(Utc::now() - Duration::days(1))
    .bind(Utc::now() - Duration::days(2))
    .execute(&db.admin_pool)
    .await
    .expect("insert expired idempotency record");

    let auth = auth_context(db.workspace_a, Uuid::now_v7());

    let deleted = db
        .app_store
        .cleanup_idempotency(&auth, Utc::now())
        .await
        .expect("cleanup idempotency");

    assert_eq!(deleted, 1);
}
