use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::StoreError;

/// Atomically lease up to `limit` workspaces to `owner_id` using
/// `INSERT … ON CONFLICT … DO UPDATE … WHERE leased_until <= $now`.
/// Partitions held by other owners whose leases are still valid are
/// skipped; expired leases are taken over.
pub(crate) async fn acquire_partitions(
    pool: &PgPool,
    owner_id: Uuid,
    now: DateTime<Utc>,
    lease_until: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<Uuid>, StoreError> {
    // Workspace discovery goes through a SECURITY DEFINER helper so the
    // worker sees every tenant without granting BYPASSRLS to the app role.
    // Concurrency is resolved by the `worker_partition_lease` PK: racing
    // workers both pre-filter with `NOT EXISTS`, but the winner is the
    // first to commit the INSERT; the loser's ON CONFLICT branch only
    // accepts the takeover when the existing lease has already expired or
    // it is the lease's own owner (renewal). Non-qualifying rows are
    // silently dropped from the RETURNING set, so callers always observe
    // the precise set they own after the call.
    let rows = sqlx::query!(
        r#"WITH candidates AS (
               SELECT w.id
               FROM list_workspace_ids_for_partition_lease() AS w(id)
               WHERE NOT EXISTS (
                   SELECT 1
                   FROM worker_partition_lease l
                   WHERE l.workspace_id = w.id
                     AND l.leased_until > $1
                     AND l.lease_owner <> $3
               )
               ORDER BY w.id
               LIMIT $2
           )
           INSERT INTO worker_partition_lease
               (workspace_id, lease_owner, leased_at, leased_until, updated_at)
           SELECT id, $3, $1, $4, $1 FROM candidates
           ON CONFLICT (workspace_id) DO UPDATE
               SET lease_owner = excluded.lease_owner,
                   leased_at = excluded.leased_at,
                   leased_until = excluded.leased_until,
                   updated_at = excluded.updated_at
               WHERE worker_partition_lease.leased_until <= excluded.leased_at
                  OR worker_partition_lease.lease_owner = excluded.lease_owner
           RETURNING workspace_id as "workspace_id!: Uuid""#,
        now,
        limit,
        owner_id,
        lease_until,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|row| row.workspace_id).collect())
}

/// Extend the lease window for partitions already owned by `owner_id`. Rows
/// held by someone else, or whose lease has already expired (allowing
/// another worker to take over), are skipped.
pub(crate) async fn renew_partitions(
    pool: &PgPool,
    owner_id: Uuid,
    workspace_ids: &[Uuid],
    now: DateTime<Utc>,
    new_lease_until: DateTime<Utc>,
) -> Result<Vec<Uuid>, StoreError> {
    if workspace_ids.is_empty() {
        return Ok(Vec::new());
    }
    let workspace_slice = workspace_ids.to_owned();
    let rows = sqlx::query!(
        r#"UPDATE worker_partition_lease
           SET leased_until = $3,
               updated_at = $2
           WHERE workspace_id = ANY($1)
             AND lease_owner = $4
             AND leased_until > $2
           RETURNING workspace_id as "workspace_id!: Uuid""#,
        &workspace_slice,
        now,
        new_lease_until,
        owner_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|row| row.workspace_id).collect())
}

/// Release partitions owned by `owner_id`. Safe to call on shutdown; rows
/// owned by other workers are left untouched.
pub(crate) async fn release_partitions(
    pool: &PgPool,
    owner_id: Uuid,
    workspace_ids: &[Uuid],
) -> Result<u64, StoreError> {
    if workspace_ids.is_empty() {
        return Ok(0);
    }
    let workspace_slice = workspace_ids.to_owned();
    let deleted = sqlx::query!(
        r#"DELETE FROM worker_partition_lease
           WHERE workspace_id = ANY($1)
             AND lease_owner = $2"#,
        &workspace_slice,
        owner_id,
    )
    .execute(pool)
    .await?
    .rows_affected();
    Ok(deleted)
}
