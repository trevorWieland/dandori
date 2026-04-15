use chrono::{DateTime, Duration, Utc};
use dandori_domain::{
    AuthContext, CreateIssueCommandV1, DomainError, Issue, IssueCreatedEventV1, Project, Workspace,
};
use sea_orm::{Database, DatabaseConnection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use thiserror::Error;
use uuid::Uuid;

use crate::repositories::{issue, outbox, project, workspace};

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
    #[error("idempotency conflict")]
    IdempotencyConflict,
    #[error("idempotency replay payload missing target issue")]
    IdempotencyReplayMissingIssue,
    #[error("invalid state category in database: {0}")]
    InvalidState(String),
    #[error("invalid priority in database: {0}")]
    InvalidPriority(String),
    #[error("domain violation: {0}")]
    Domain(#[from] DomainError),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct IdempotencyResponse {
    pub issue_id: Uuid,
}

impl PgStore {
    #[must_use]
    pub fn from_connections(pool: PgPool, db: DatabaseConnection) -> Self {
        Self { pool, db }
    }

    pub async fn connect(database_url: &str) -> Result<Self, StoreError> {
        let pool = PgPool::connect(database_url).await?;
        let db = Database::connect(database_url).await?;
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
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        outbox::mark_outbox_delivered(self, auth, outbox_id, now).await
    }

    pub async fn mark_outbox_failed(
        &self,
        auth: &AuthContext,
        outbox_id: Uuid,
        now: DateTime<Utc>,
        error_message: &str,
        max_attempts: i32,
        retry_backoff: Duration,
    ) -> Result<(), StoreError> {
        outbox::mark_outbox_failed(
            self,
            auth,
            outbox_id,
            now,
            error_message,
            max_attempts,
            retry_backoff,
        )
        .await
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
}
