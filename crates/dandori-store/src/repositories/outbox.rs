use chrono::{DateTime, Duration, Utc};
use dandori_domain::AuthContext;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::pg_store::{OutboxMessage, PgStore};
use crate::{StoreError, repositories::common::set_workspace_context_tx};

#[derive(Debug, sqlx::FromRow)]
struct OutboxRow {
    id: Uuid,
    workspace_id: Uuid,
    event_id: Uuid,
    event_type: String,
    aggregate_type: String,
    aggregate_id: Uuid,
    correlation_id: Uuid,
    payload: serde_json::Value,
    attempts: i32,
}

pub(crate) async fn lease_outbox_batch(
    store: &PgStore,
    auth: &AuthContext,
    now: DateTime<Utc>,
    lease_for: Duration,
    max_items: i64,
) -> Result<Vec<OutboxMessage>, StoreError> {
    let mut tx = store.pool().begin().await?;
    set_workspace_context_tx(&mut tx, auth.workspace_id.0).await?;

    let lease_until = now + lease_for;

    let rows = sqlx::query_as::<_, OutboxRow>(
        "WITH candidates AS (
            SELECT id
            FROM outbox
            WHERE workspace_id = $1
              AND (
                ((status = 'pending'::outbox_status OR status = 'failed'::outbox_status) AND available_at <= $2)
                OR (status = 'leased'::outbox_status AND leased_until <= $2)
              )
            ORDER BY available_at, id
            LIMIT $3
            FOR UPDATE SKIP LOCKED
        )
        UPDATE outbox AS o
        SET status = 'leased'::outbox_status,
            leased_at = $2,
            leased_until = $4,
            updated_at = $2
        FROM candidates
        WHERE o.id = candidates.id
        RETURNING o.id, o.workspace_id, o.event_id, o.event_type, o.aggregate_type,
                  o.aggregate_id, o.correlation_id, o.payload, o.attempts",
    )
    .bind(auth.workspace_id.0)
    .bind(now)
    .bind(max_items)
    .bind(lease_until)
    .fetch_all(tx.as_mut())
    .await?;

    tx.commit().await?;

    Ok(rows
        .into_iter()
        .map(|row| OutboxMessage {
            id: row.id,
            workspace_id: row.workspace_id,
            event_id: row.event_id,
            event_type: row.event_type,
            aggregate_type: row.aggregate_type,
            aggregate_id: row.aggregate_id,
            correlation_id: row.correlation_id,
            payload: row.payload,
            attempts: row.attempts,
        })
        .collect())
}

pub(crate) async fn mark_outbox_delivered(
    store: &PgStore,
    auth: &AuthContext,
    outbox_id: Uuid,
    now: DateTime<Utc>,
) -> Result<(), StoreError> {
    let mut tx = store.pool().begin().await?;
    set_workspace_context_tx(&mut tx, auth.workspace_id.0).await?;

    sqlx::query(
        "UPDATE outbox
         SET status = 'delivered'::outbox_status,
             published_at = $1,
             leased_at = NULL,
             leased_until = NULL,
             updated_at = $1,
             last_error = NULL
         WHERE id = $2",
    )
    .bind(now)
    .bind(outbox_id)
    .execute(tx.as_mut())
    .await?;

    tx.commit().await?;
    Ok(())
}

pub(crate) async fn mark_outbox_failed(
    store: &PgStore,
    auth: &AuthContext,
    outbox_id: Uuid,
    now: DateTime<Utc>,
    error_message: &str,
    max_attempts: i32,
    retry_backoff: Duration,
) -> Result<(), StoreError> {
    let mut tx = store.pool().begin().await?;
    set_workspace_context_tx(&mut tx, auth.workspace_id.0).await?;

    let current_attempts = fetch_attempts_for_update(&mut tx, outbox_id).await?;
    let next_attempts = current_attempts + 1;

    if next_attempts >= max_attempts {
        sqlx::query(
            "UPDATE outbox
             SET status = 'dead_letter'::outbox_status,
                 attempts = $1,
                 available_at = $2,
                 leased_at = NULL,
                 leased_until = NULL,
                 updated_at = $2,
                 last_error = $3
             WHERE id = $4",
        )
        .bind(next_attempts)
        .bind(now)
        .bind(error_message)
        .bind(outbox_id)
        .execute(tx.as_mut())
        .await?;
    } else {
        sqlx::query(
            "UPDATE outbox
             SET status = 'failed'::outbox_status,
                 attempts = $1,
                 available_at = $2,
                 leased_at = NULL,
                 leased_until = NULL,
                 updated_at = $3,
                 last_error = $4
             WHERE id = $5",
        )
        .bind(next_attempts)
        .bind(now + retry_backoff)
        .bind(now)
        .bind(error_message)
        .bind(outbox_id)
        .execute(tx.as_mut())
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

pub(crate) async fn cleanup_outbox(
    store: &PgStore,
    auth: &AuthContext,
    delivered_before: DateTime<Utc>,
    dead_letter_before: DateTime<Utc>,
) -> Result<u64, StoreError> {
    let mut tx = store.pool().begin().await?;
    set_workspace_context_tx(&mut tx, auth.workspace_id.0).await?;

    let deleted = sqlx::query(
        "DELETE FROM outbox
         WHERE (status = 'delivered'::outbox_status AND published_at < $1)
            OR (status = 'dead_letter'::outbox_status AND updated_at < $2)",
    )
    .bind(delivered_before)
    .bind(dead_letter_before)
    .execute(tx.as_mut())
    .await?
    .rows_affected();

    tx.commit().await?;
    Ok(deleted)
}

pub(crate) async fn cleanup_idempotency(
    store: &PgStore,
    auth: &AuthContext,
    expires_before: DateTime<Utc>,
) -> Result<u64, StoreError> {
    let mut tx = store.pool().begin().await?;
    set_workspace_context_tx(&mut tx, auth.workspace_id.0).await?;

    let deleted = sqlx::query("DELETE FROM idempotency_record WHERE expires_at < $1")
        .bind(expires_before)
        .execute(tx.as_mut())
        .await?
        .rows_affected();

    tx.commit().await?;
    Ok(deleted)
}

async fn fetch_attempts_for_update(
    tx: &mut Transaction<'_, Postgres>,
    outbox_id: Uuid,
) -> Result<i32, StoreError> {
    let attempts = sqlx::query_scalar::<_, i32>(
        "SELECT attempts
         FROM outbox
         WHERE id = $1
         FOR UPDATE",
    )
    .bind(outbox_id)
    .fetch_one(tx.as_mut())
    .await?;

    Ok(attempts)
}
