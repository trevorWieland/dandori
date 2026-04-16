mod support;

use chrono::{Duration, Utc};
use dandori_store::{OutboxFailureContext, StoreError};
use sqlx::query;
use uuid::Uuid;

use support::{auth_context, make_command, make_event, setup_db};

#[tokio::test]
async fn outbox_lease_retry_dead_letter_and_retention_flow() {
    let db = setup_db().await;
    let _ = (db._workspace_b, db._workflow_a);

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
            OutboxFailureContext {
                lease_token: leased.lease_token,
                lease_owner: leased.lease_owner,
                now: Utc::now(),
                error_message: "first failure".to_owned(),
                max_attempts: 2,
                retry_backoff: Duration::seconds(0),
            },
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
            OutboxFailureContext {
                lease_token: second_lease[0].lease_token,
                lease_owner: second_lease[0].lease_owner,
                now: Utc::now(),
                error_message: "second failure".to_owned(),
                max_attempts: 2,
                retry_backoff: Duration::seconds(0),
            },
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
    let _ = (db._workspace_b, db._workflow_a);

    query(
        "INSERT INTO idempotency_record (
            workspace_id,
            command_name,
            idempotency_key,
            request_fingerprint,
            response_payload,
            expires_at,
            created_at
        ) VALUES ($1, $2, $3, $4, '{}'::jsonb, $5, $6)",
    )
    .bind(db.workspace_a)
    .bind("issue.create.v1")
    .bind("expired-key")
    .bind("expired-fingerprint")
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

#[tokio::test]
async fn outbox_status_updates_require_matching_workspace_and_row() {
    let db = setup_db().await;

    let command = make_command(
        db.workspace_a,
        db.project_a,
        Uuid::now_v7(),
        Uuid::now_v7(),
        "idem-outbox-workspace-check",
    );
    let event = make_event(&command);
    let auth_a = auth_context(db.workspace_a, command.actor_id);
    let auth_b = auth_context(db._workspace_b, Uuid::now_v7());

    db.app_store
        .create_issue_transactional(&auth_a, &command, &event)
        .await
        .expect("create issue");

    let leased = db
        .app_store
        .lease_outbox_batch(&auth_a, Utc::now(), Duration::seconds(30), 10)
        .await
        .expect("lease");

    let outbox_id = leased[0].id;
    let lease_token = leased[0].lease_token;
    let lease_owner = leased[0].lease_owner;
    let delivered_err = db
        .app_store
        .mark_outbox_delivered(&auth_b, outbox_id, lease_token, lease_owner, Utc::now())
        .await
        .expect_err("cross-workspace delivery update should fail");
    assert!(matches!(
        delivered_err,
        StoreError::OutboxLeaseMissing { .. }
    ));

    let failed_err = db
        .app_store
        .mark_outbox_failed(
            &auth_b,
            outbox_id,
            OutboxFailureContext {
                lease_token,
                lease_owner,
                now: Utc::now(),
                error_message: "cross workspace".to_owned(),
                max_attempts: 3,
                retry_backoff: Duration::seconds(0),
            },
        )
        .await
        .expect_err("cross-workspace failure update should fail");
    assert!(matches!(failed_err, StoreError::OutboxLeaseMissing { .. }));
}

#[tokio::test]
async fn outbox_status_updates_reject_stale_duplicate_and_invalid_lease_identity() {
    let db = setup_db().await;
    let command = make_command(
        db.workspace_a,
        db.project_a,
        Uuid::now_v7(),
        Uuid::now_v7(),
        "idem-outbox-lease-guards",
    );
    let event = make_event(&command);
    let auth = auth_context(db.workspace_a, command.actor_id);

    db.app_store
        .create_issue_transactional(&auth, &command, &event)
        .await
        .expect("create issue");

    let leased = db
        .app_store
        .lease_outbox_batch(&auth, Utc::now(), Duration::seconds(30), 10)
        .await
        .expect("lease");
    assert_eq!(leased.len(), 1);
    let message = &leased[0];

    let wrong_owner_auth = auth_context(db.workspace_a, Uuid::now_v7());
    let wrong_owner_err = db
        .app_store
        .mark_outbox_delivered(
            &wrong_owner_auth,
            message.id,
            message.lease_token,
            wrong_owner_auth.actor_id,
            Utc::now(),
        )
        .await
        .expect_err("wrong owner must fail");
    assert!(matches!(
        wrong_owner_err,
        StoreError::OutboxLeaseOwnerMismatch { .. }
    ));

    let wrong_token_err = db
        .app_store
        .mark_outbox_delivered(
            &auth,
            message.id,
            Uuid::now_v7(),
            message.lease_owner,
            Utc::now(),
        )
        .await
        .expect_err("wrong token must fail");
    assert!(matches!(
        wrong_token_err,
        StoreError::OutboxLeaseTokenMismatch { .. }
    ));

    let expired_err = db
        .app_store
        .mark_outbox_delivered(
            &auth,
            message.id,
            message.lease_token,
            message.lease_owner,
            message.leased_until + Duration::seconds(1),
        )
        .await
        .expect_err("expired lease must fail");
    assert!(matches!(expired_err, StoreError::OutboxLeaseExpired { .. }));

    db.app_store
        .mark_outbox_delivered(
            &auth,
            message.id,
            message.lease_token,
            message.lease_owner,
            Utc::now(),
        )
        .await
        .expect("valid delivery transition");

    let stale_duplicate_err = db
        .app_store
        .mark_outbox_delivered(
            &auth,
            message.id,
            message.lease_token,
            message.lease_owner,
            Utc::now(),
        )
        .await
        .expect_err("stale duplicate delivery must fail");
    assert!(matches!(
        stale_duplicate_err,
        StoreError::OutboxNotLeased { .. }
    ));
}
