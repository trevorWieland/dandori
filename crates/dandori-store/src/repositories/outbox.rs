use chrono::{DateTime, Duration, Utc};
use dandori_domain::AuthContext;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::pg_store::{OutboxFailureContext, OutboxMessage, PgStore};
use crate::{StoreError, repositories::common::set_workspace_context_tx};

#[derive(Debug)]
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
    lease_token: Uuid,
    lease_owner: Uuid,
    leased_until: DateTime<Utc>,
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
    let lease_token = Uuid::now_v7();

    let rows = sqlx::query_as!(
        OutboxRow,
        r#"WITH candidates AS (
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
            lease_token = $5,
            lease_owner = $6,
            updated_at = $2
        FROM candidates
        WHERE o.id = candidates.id
        RETURNING o.id,
                  o.workspace_id,
                  o.event_id,
                  o.event_type,
                  o.aggregate_type,
                  o.aggregate_id,
                  o.correlation_id,
                  o.payload as "payload!: serde_json::Value",
                  o.attempts,
                  o.lease_token as "lease_token!: Uuid",
                  o.lease_owner as "lease_owner!: Uuid",
                  o.leased_until as "leased_until!: DateTime<Utc>""#,
        auth.workspace_id.0,
        now,
        max_items,
        lease_until,
        lease_token,
        auth.actor_id,
    )
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
            lease_token: row.lease_token,
            lease_owner: row.lease_owner,
            leased_until: row.leased_until,
        })
        .collect())
}

pub(crate) async fn mark_outbox_delivered(
    store: &PgStore,
    auth: &AuthContext,
    outbox_id: Uuid,
    lease_token: Uuid,
    lease_owner: Uuid,
    now: DateTime<Utc>,
) -> Result<(), StoreError> {
    let mut tx = store.pool().begin().await?;
    set_workspace_context_tx(&mut tx, auth.workspace_id.0).await?;
    let lease = fetch_lease_for_update(&mut tx, auth.workspace_id.0, outbox_id)
        .await?
        .ok_or(StoreError::OutboxLeaseMissing {
            workspace_id: auth.workspace_id.0,
            outbox_id,
        })?;
    if lease_owner != auth.actor_id {
        return Err(StoreError::OutboxLeaseOwnerMismatch {
            workspace_id: auth.workspace_id.0,
            outbox_id,
            expected_owner: auth.actor_id,
            actual_owner: lease_owner,
        });
    }

    validate_lease_transition(
        LeaseMetadata {
            workspace_id: auth.workspace_id.0,
            outbox_id,
            status: lease.status.as_str(),
            stored_token: lease.lease_token,
            stored_owner: lease.lease_owner,
            stored_until: lease.leased_until,
        },
        lease_token,
        lease_owner,
        now,
    )?;

    let rows_affected = sqlx::query!(
        r#"UPDATE outbox
           SET status = 'delivered'::outbox_status,
               published_at = $1,
               leased_at = NULL,
               leased_until = NULL,
               lease_token = NULL,
               lease_owner = NULL,
               updated_at = $1,
               last_error = NULL
           WHERE workspace_id = $2
             AND id = $3
             AND status = 'leased'::outbox_status
             AND lease_token = $4
             AND lease_owner = $5
             AND leased_until > $1"#,
        now,
        auth.workspace_id.0,
        outbox_id,
        lease_token,
        lease_owner,
    )
    .execute(tx.as_mut())
    .await?
    .rows_affected();

    if rows_affected != 1 {
        return Err(StoreError::OutboxUpdateNotSingleRow {
            workspace_id: auth.workspace_id.0,
            outbox_id,
            rows_affected,
        });
    }

    tx.commit().await?;
    Ok(())
}

pub(crate) async fn mark_outbox_failed(
    store: &PgStore,
    auth: &AuthContext,
    outbox_id: Uuid,
    failure: &OutboxFailureContext,
) -> Result<(), StoreError> {
    let mut tx = store.pool().begin().await?;
    set_workspace_context_tx(&mut tx, auth.workspace_id.0).await?;
    let lease = fetch_lease_for_update(&mut tx, auth.workspace_id.0, outbox_id)
        .await?
        .ok_or(StoreError::OutboxLeaseMissing {
            workspace_id: auth.workspace_id.0,
            outbox_id,
        })?;
    if failure.lease_owner != auth.actor_id {
        return Err(StoreError::OutboxLeaseOwnerMismatch {
            workspace_id: auth.workspace_id.0,
            outbox_id,
            expected_owner: auth.actor_id,
            actual_owner: failure.lease_owner,
        });
    }

    validate_lease_transition(
        LeaseMetadata {
            workspace_id: auth.workspace_id.0,
            outbox_id,
            status: lease.status.as_str(),
            stored_token: lease.lease_token,
            stored_owner: lease.lease_owner,
            stored_until: lease.leased_until,
        },
        failure.lease_token,
        failure.lease_owner,
        failure.now,
    )?;

    let next_attempts = lease.attempts + 1;

    let rows_affected = if next_attempts >= failure.max_attempts {
        sqlx::query!(
            r#"UPDATE outbox
               SET status = 'dead_letter'::outbox_status,
                   attempts = $1,
                   available_at = $2,
                   leased_at = NULL,
                   leased_until = NULL,
                   lease_token = NULL,
                   lease_owner = NULL,
                   updated_at = $2,
                   last_error = $3
               WHERE workspace_id = $4
                 AND id = $5
                 AND status = 'leased'::outbox_status
                 AND lease_token = $6
                 AND lease_owner = $7
                 AND leased_until > $2"#,
            next_attempts,
            failure.now,
            failure.error_message,
            auth.workspace_id.0,
            outbox_id,
            failure.lease_token,
            failure.lease_owner,
        )
        .execute(tx.as_mut())
        .await?
        .rows_affected()
    } else {
        sqlx::query!(
            r#"UPDATE outbox
               SET status = 'failed'::outbox_status,
                   attempts = $1,
                   available_at = $2,
                   leased_at = NULL,
                   leased_until = NULL,
                   lease_token = NULL,
                   lease_owner = NULL,
                   updated_at = $3,
                   last_error = $4
               WHERE workspace_id = $5
                 AND id = $6
                 AND status = 'leased'::outbox_status
                 AND lease_token = $7
                 AND lease_owner = $8
                 AND leased_until > $3"#,
            next_attempts,
            failure.now + failure.retry_backoff,
            failure.now,
            failure.error_message,
            auth.workspace_id.0,
            outbox_id,
            failure.lease_token,
            failure.lease_owner,
        )
        .execute(tx.as_mut())
        .await?
        .rows_affected()
    };

    if rows_affected != 1 {
        return Err(StoreError::OutboxUpdateNotSingleRow {
            workspace_id: auth.workspace_id.0,
            outbox_id,
            rows_affected,
        });
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

    let deleted = sqlx::query!(
        r#"DELETE FROM outbox
           WHERE workspace_id = $1
             AND (
                  (status = 'delivered'::outbox_status AND published_at < $2)
               OR (status = 'dead_letter'::outbox_status AND updated_at < $3)
             )"#,
        auth.workspace_id.0,
        delivered_before,
        dead_letter_before,
    )
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

    let deleted = sqlx::query!(
        r#"DELETE FROM idempotency_record
           WHERE workspace_id = $1 AND expires_at < $2"#,
        auth.workspace_id.0,
        expires_before,
    )
    .execute(tx.as_mut())
    .await?
    .rows_affected();

    tx.commit().await?;
    Ok(deleted)
}

struct OutboxLeaseRow {
    status: String,
    attempts: i32,
    lease_token: Option<Uuid>,
    lease_owner: Option<Uuid>,
    leased_until: Option<DateTime<Utc>>,
}

async fn fetch_lease_for_update(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    outbox_id: Uuid,
) -> Result<Option<OutboxLeaseRow>, StoreError> {
    let row = sqlx::query!(
        r#"SELECT
               status::text as "status!",
               attempts,
               lease_token,
               lease_owner,
               leased_until
           FROM outbox
           WHERE workspace_id = $1 AND id = $2
           FOR UPDATE"#,
        workspace_id,
        outbox_id,
    )
    .fetch_optional(tx.as_mut())
    .await?;

    Ok(row.map(|row| OutboxLeaseRow {
        status: row.status,
        attempts: row.attempts,
        lease_token: row.lease_token,
        lease_owner: row.lease_owner,
        leased_until: row.leased_until,
    }))
}

fn validate_lease_transition(
    metadata: LeaseMetadata,
    expected_token: Uuid,
    expected_owner: Uuid,
    now: DateTime<Utc>,
) -> Result<(), StoreError> {
    if metadata.status != "leased" {
        return Err(StoreError::OutboxNotLeased {
            workspace_id: metadata.workspace_id,
            outbox_id: metadata.outbox_id,
            status: metadata.status.to_owned(),
        });
    }

    let actual_owner = metadata.stored_owner.ok_or(StoreError::OutboxNotLeased {
        workspace_id: metadata.workspace_id,
        outbox_id: metadata.outbox_id,
        status: metadata.status.to_owned(),
    })?;
    if actual_owner != expected_owner {
        return Err(StoreError::OutboxLeaseOwnerMismatch {
            workspace_id: metadata.workspace_id,
            outbox_id: metadata.outbox_id,
            expected_owner,
            actual_owner,
        });
    }

    let actual_token = metadata.stored_token.ok_or(StoreError::OutboxNotLeased {
        workspace_id: metadata.workspace_id,
        outbox_id: metadata.outbox_id,
        status: metadata.status.to_owned(),
    })?;
    if actual_token != expected_token {
        return Err(StoreError::OutboxLeaseTokenMismatch {
            workspace_id: metadata.workspace_id,
            outbox_id: metadata.outbox_id,
            expected_token,
            actual_token,
        });
    }

    let leased_until = metadata.stored_until.ok_or(StoreError::OutboxNotLeased {
        workspace_id: metadata.workspace_id,
        outbox_id: metadata.outbox_id,
        status: metadata.status.to_owned(),
    })?;

    if leased_until <= now {
        return Err(StoreError::OutboxLeaseExpired {
            workspace_id: metadata.workspace_id,
            outbox_id: metadata.outbox_id,
            leased_until,
            now,
        });
    }

    Ok(())
}

struct LeaseMetadata<'a> {
    workspace_id: Uuid,
    outbox_id: Uuid,
    status: &'a str,
    stored_token: Option<Uuid>,
    stored_owner: Option<Uuid>,
    stored_until: Option<DateTime<Utc>>,
}
