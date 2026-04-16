use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::StoreError;

/// Inclusive shard-bucket window of workspaces a worker is authorized to
/// lease. Shard buckets are `[0, 1024)` and assigned deterministically by
/// `hashtext(workspace_id::text) % 1024` at migration time.
#[derive(Debug, Clone, Copy)]
pub struct ShardBucketRange {
    min: i32,
    max: i32,
}

impl ShardBucketRange {
    pub const MAX_BUCKET: i32 = 1023;

    pub fn new(min: i32, max: i32) -> Result<Self, StoreError> {
        if !(0..=Self::MAX_BUCKET).contains(&min) || !(0..=Self::MAX_BUCKET).contains(&max) {
            return Err(StoreError::InvalidInput(format!(
                "shard bucket range {min}..{max} out of bounds [0,{}]",
                Self::MAX_BUCKET
            )));
        }
        if min > max {
            return Err(StoreError::InvalidInput(format!(
                "shard bucket range min ({min}) exceeds max ({max})"
            )));
        }
        Ok(Self { min, max })
    }

    #[must_use]
    pub fn full() -> Self {
        Self {
            min: 0,
            max: Self::MAX_BUCKET,
        }
    }

    #[must_use]
    pub fn min(&self) -> i32 {
        self.min
    }

    #[must_use]
    pub fn max(&self) -> i32 {
        self.max
    }
}

/// Atomically lease up to `limit` workspaces from the given shard-bucket
/// window to `owner_id`. Uses `INSERT … ON CONFLICT … DO UPDATE … WHERE
/// leased_until <= $now` so racing workers converge deterministically:
/// the winner commits first; the loser's ON CONFLICT branch only takes
/// over the lease if it has already expired or the loser is the existing
/// owner (a renewal). Non-qualifying rows are dropped from the RETURNING
/// set so callers always observe the precise set they now own.
pub(crate) async fn acquire_partitions(
    pool: &PgPool,
    owner_id: Uuid,
    now: DateTime<Utc>,
    lease_until: DateTime<Utc>,
    limit: i64,
    buckets: ShardBucketRange,
) -> Result<Vec<Uuid>, StoreError> {
    let scan_limit: i32 = i32::try_from(limit.min(i64::from(i32::MAX))).unwrap_or(i32::MAX);
    let rows = sqlx::query!(
        r#"WITH candidates AS (
               SELECT w.id, w.shard_bucket
               FROM list_workspace_ids_for_partition_lease($4, $5, $6) AS w(id, shard_bucket)
               WHERE NOT EXISTS (
                   SELECT 1
                   FROM worker_partition_lease l
                   WHERE l.workspace_id = w.id
                     AND l.leased_until > $1
                     AND l.lease_owner <> $2
               )
               ORDER BY w.shard_bucket, w.id
           )
           INSERT INTO worker_partition_lease
               (workspace_id, shard_bucket, lease_owner, leased_at, leased_until, updated_at)
           SELECT id, shard_bucket, $2, $1, $3, $1 FROM candidates
           ON CONFLICT (workspace_id) DO UPDATE
               SET lease_owner = excluded.lease_owner,
                   shard_bucket = excluded.shard_bucket,
                   leased_at = excluded.leased_at,
                   leased_until = excluded.leased_until,
                   updated_at = excluded.updated_at
               WHERE worker_partition_lease.leased_until <= excluded.leased_at
                  OR worker_partition_lease.lease_owner = excluded.lease_owner
           RETURNING workspace_id as "workspace_id!: Uuid""#,
        now,
        owner_id,
        lease_until,
        buckets.min,
        buckets.max,
        scan_limit,
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
