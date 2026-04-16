use chrono::{DateTime, Duration, Utc};
use dandori_domain::{
    AuthContext, CreateIssueCommandV1, DomainError, Issue, IssueCreatedEventV1, Project, Workspace,
};
use sea_orm::{DatabaseConnection, SqlxPostgresConnector};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use thiserror::Error;
use uuid::Uuid;

use crate::repositories::{issue, outbox, partition, project, workspace};

pub use crate::repositories::partition::ShardBucketRange;

#[derive(Debug, Clone)]
pub struct PgStore {
    pool: PgPool,
    db: DatabaseConnection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateIssueWriteResult {
    pub issue: Issue,
    pub idempotent_replay: bool,
}

#[derive(Debug, Clone)]
pub struct WorkspaceWriteInput {
    pub workspace_id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct ProjectWriteInput {
    pub project_id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub workflow_version_id: Uuid,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutboxMessage {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub event_id: Uuid,
    pub event_type: String,
    pub aggregate_type: String,
    pub aggregate_id: Uuid,
    pub correlation_id: Uuid,
    pub payload: Value,
    pub attempts: i32,
    pub lease_token: Uuid,
    pub lease_owner: Uuid,
    pub leased_until: DateTime<Utc>,
}

/// Classification of an outbox publish failure. Only [`Transient`](Self::Transient)
/// spends the per-row retry budget; [`Terminal`](Self::Terminal) causes the row
/// to land in `dead_letter` on first failure so unretriable errors do not soak
/// the worker's time or the DLQ scanner's attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboxFailureClassification {
    /// Retriable error (e.g. network blip, server 5xx, breaker open). Honours
    /// the attempt budget and backoff.
    Transient,
    /// Non-retriable error (e.g. client 4xx, unsupported event type,
    /// serialization failure). Dead-letters immediately.
    Terminal,
}

#[derive(Debug, Clone)]
pub struct OutboxFailureContext {
    pub classification: OutboxFailureClassification,
    pub lease_token: Uuid,
    pub lease_owner: Uuid,
    pub now: DateTime<Utc>,
    pub error_message: String,
    pub max_attempts: i32,
    pub retry_backoff: Duration,
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("sea-orm error: {0}")]
    SeaOrm(#[from] sea_orm::DbErr),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("project not found")]
    ProjectNotFound,
    #[error("workflow version not found")]
    WorkflowVersionNotFound,
    #[error("milestone not found")]
    MilestoneNotFound,
    #[error("milestone project mismatch")]
    MilestoneProjectMismatch,
    #[error("idempotency conflict")]
    IdempotencyConflict,
    #[error("idempotency replay payload missing target issue")]
    IdempotencyReplayMissingIssue,
    #[error("invalid state category in database: {0}")]
    InvalidState(String),
    #[error("invalid priority in database: {0}")]
    InvalidPriority(String),
    #[error(
        "outbox state update expected exactly one row for id '{outbox_id}' in workspace '{workspace_id}', updated {rows_affected}"
    )]
    OutboxUpdateNotSingleRow {
        workspace_id: Uuid,
        outbox_id: Uuid,
        rows_affected: u64,
    },
    #[error("outbox row '{outbox_id}' not found in workspace '{workspace_id}'")]
    OutboxLeaseMissing { workspace_id: Uuid, outbox_id: Uuid },
    #[error(
        "outbox row '{outbox_id}' in workspace '{workspace_id}' is not leased (current status: {status})"
    )]
    OutboxNotLeased {
        workspace_id: Uuid,
        outbox_id: Uuid,
        status: String,
    },
    #[error(
        "outbox row '{outbox_id}' in workspace '{workspace_id}' lease expired at '{leased_until}' (now '{now}')"
    )]
    OutboxLeaseExpired {
        workspace_id: Uuid,
        outbox_id: Uuid,
        leased_until: DateTime<Utc>,
        now: DateTime<Utc>,
    },
    #[error(
        "outbox row '{outbox_id}' in workspace '{workspace_id}' lease owner mismatch (expected '{expected_owner}', actual '{actual_owner}')"
    )]
    OutboxLeaseOwnerMismatch {
        workspace_id: Uuid,
        outbox_id: Uuid,
        expected_owner: Uuid,
        actual_owner: Uuid,
    },
    #[error(
        "outbox row '{outbox_id}' in workspace '{workspace_id}' lease token mismatch (expected '{expected_token}', actual '{actual_token}')"
    )]
    OutboxLeaseTokenMismatch {
        workspace_id: Uuid,
        outbox_id: Uuid,
        expected_token: Uuid,
        actual_token: Uuid,
    },
    #[error("domain violation: {0}")]
    Domain(#[from] DomainError),
    #[error("invalid input: {0}")]
    InvalidInput(String),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct IdempotencyResponse {
    pub issue: Issue,
}

impl PgStore {
    #[must_use]
    pub fn from_connections(pool: PgPool, db: DatabaseConnection) -> Self {
        Self { pool, db }
    }

    pub async fn connect(database_url: &str) -> Result<Self, StoreError> {
        let pool = PgPool::connect(database_url).await?;
        let db = SqlxPostgresConnector::from_sqlx_postgres_pool(pool.clone());
        Ok(Self { pool, db })
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    #[must_use]
    pub fn db(&self) -> &DatabaseConnection {
        &self.db
    }

    pub async fn create_issue_transactional(
        &self,
        auth: &AuthContext,
        command: &CreateIssueCommandV1,
        event: &IssueCreatedEventV1,
    ) -> Result<CreateIssueWriteResult, StoreError> {
        issue::create_issue_transactional(self, auth, command, event).await
    }

    pub async fn get_issue(
        &self,
        auth: &AuthContext,
        issue_id: Uuid,
    ) -> Result<Option<Issue>, StoreError> {
        issue::get_issue(self, auth, issue_id).await
    }

    pub async fn create_workspace(
        &self,
        auth: &AuthContext,
        input: WorkspaceWriteInput,
    ) -> Result<Workspace, StoreError> {
        workspace::create_workspace(self, auth, input).await
    }

    pub async fn get_workspace(
        &self,
        auth: &AuthContext,
        workspace_id: Uuid,
    ) -> Result<Option<Workspace>, StoreError> {
        workspace::get_workspace(self, auth, workspace_id).await
    }

    pub async fn create_project(
        &self,
        auth: &AuthContext,
        input: ProjectWriteInput,
    ) -> Result<Project, StoreError> {
        project::create_project(self, auth, input).await
    }

    pub async fn get_project(
        &self,
        auth: &AuthContext,
        project_id: Uuid,
    ) -> Result<Option<Project>, StoreError> {
        project::get_project(self, auth, project_id).await
    }

    pub async fn lease_outbox_batch(
        &self,
        auth: &AuthContext,
        now: DateTime<Utc>,
        lease_for: Duration,
        max_items: i64,
    ) -> Result<Vec<OutboxMessage>, StoreError> {
        outbox::lease_outbox_batch(self, auth, now, lease_for, max_items).await
    }

    pub async fn mark_outbox_delivered(
        &self,
        auth: &AuthContext,
        outbox_id: Uuid,
        lease_token: Uuid,
        lease_owner: Uuid,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        outbox::mark_outbox_delivered(self, auth, outbox_id, lease_token, lease_owner, now).await
    }

    pub async fn mark_outbox_failed(
        &self,
        auth: &AuthContext,
        outbox_id: Uuid,
        failure: OutboxFailureContext,
    ) -> Result<(), StoreError> {
        outbox::mark_outbox_failed(self, auth, outbox_id, &failure).await
    }

    pub async fn cleanup_outbox(
        &self,
        auth: &AuthContext,
        delivered_before: DateTime<Utc>,
        dead_letter_before: DateTime<Utc>,
    ) -> Result<u64, StoreError> {
        outbox::cleanup_outbox(self, auth, delivered_before, dead_letter_before).await
    }

    pub async fn cleanup_idempotency(
        &self,
        auth: &AuthContext,
        expires_before: DateTime<Utc>,
    ) -> Result<u64, StoreError> {
        outbox::cleanup_idempotency(self, auth, expires_before).await
    }

    /// Atomically lease workspaces for this worker from a bounded shard-bucket
    /// window. Partitions held by another worker with a still-valid lease are
    /// skipped; expired leases are taken over. Callers must treat the returned
    /// list as definitive ownership for the lease window.
    pub async fn acquire_partitions(
        &self,
        owner_id: Uuid,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        limit: i64,
        buckets: ShardBucketRange,
    ) -> Result<Vec<Uuid>, StoreError> {
        partition::acquire_partitions(&self.pool, owner_id, now, lease_until, limit, buckets).await
    }

    /// Extend the lease window on partitions currently owned by `owner_id`.
    pub async fn renew_partitions(
        &self,
        owner_id: Uuid,
        workspace_ids: &[Uuid],
        now: DateTime<Utc>,
        new_lease_until: DateTime<Utc>,
    ) -> Result<Vec<Uuid>, StoreError> {
        partition::renew_partitions(&self.pool, owner_id, workspace_ids, now, new_lease_until).await
    }

    /// Release partition leases held by this worker. Intended for graceful
    /// shutdown; safe to call repeatedly.
    pub async fn release_partitions(
        &self,
        owner_id: Uuid,
        workspace_ids: &[Uuid],
    ) -> Result<u64, StoreError> {
        partition::release_partitions(&self.pool, owner_id, workspace_ids).await
    }
}
