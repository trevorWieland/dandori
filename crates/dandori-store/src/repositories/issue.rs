use chrono::{DateTime, Duration, Utc};
use dandori_domain::{
    AuthContext, CreateIssueCommandV1, Issue, IssueCreatedEventV1, IssuePriority,
    IssueStateCategory,
};
use serde_json::json;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::pg_store::{CreateIssueWriteResult, IdempotencyResponse, PgStore};
use crate::{StoreError, repositories::common::set_workspace_context_tx};

#[derive(Debug, Clone, sqlx::FromRow)]
struct IssueRow {
    id: Uuid,
    workspace_id: Uuid,
    project_id: Uuid,
    milestone_id: Option<Uuid>,
    title: String,
    description: Option<String>,
    state_category: String,
    priority: String,
    archived_at: Option<DateTime<Utc>>,
    row_version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct IdempotencyRow {
    command_id: Uuid,
    response_payload: serde_json::Value,
}

pub(crate) async fn create_issue_transactional(
    store: &PgStore,
    auth: &AuthContext,
    command: &CreateIssueCommandV1,
    event: &IssueCreatedEventV1,
) -> Result<CreateIssueWriteResult, StoreError> {
    let mut tx = store.pool().begin().await?;
    set_workspace_context_tx(&mut tx, auth.workspace_id.0).await?;

    let maybe_idempotency = sqlx::query_as::<_, IdempotencyRow>(
        "SELECT command_id, response_payload
         FROM idempotency_record
         WHERE workspace_id = $1 AND command_name = $2 AND idempotency_key = $3",
    )
    .bind(command.workspace_id.0)
    .bind("issue.create.v1")
    .bind(command.idempotency_key.as_str())
    .fetch_optional(tx.as_mut())
    .await?;

    if let Some(existing) = maybe_idempotency {
        if existing.command_id != command.command_id.0 {
            return Err(StoreError::IdempotencyConflict);
        }

        let replay: IdempotencyResponse = serde_json::from_value(existing.response_payload)?;
        let issue = fetch_issue_in_tx(&mut tx, replay.issue_id)
            .await?
            .ok_or(StoreError::IdempotencyReplayMissingIssue)?;
        tx.commit().await?;

        return Ok(CreateIssueWriteResult {
            issue,
            idempotent_replay: true,
        });
    }

    let project_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1
            FROM project
            WHERE id = $1 AND workspace_id = $2
        )",
    )
    .bind(command.project_id.0)
    .bind(command.workspace_id.0)
    .fetch_one(tx.as_mut())
    .await?;

    if !project_exists {
        return Err(StoreError::ProjectNotFound);
    }

    sqlx::query(
        "INSERT INTO issue (
            id,
            workspace_id,
            project_id,
            milestone_id,
            title,
            description,
            state_category,
            priority,
            archived_at,
            row_version,
            created_at,
            updated_at
        ) VALUES (
            $1,
            $2,
            $3,
            $4,
            $5,
            $6,
            'open'::issue_state_category,
            $7::issue_priority,
            NULL,
            0,
            $8,
            $8
        )",
    )
    .bind(command.issue_id.0)
    .bind(command.workspace_id.0)
    .bind(command.project_id.0)
    .bind(command.milestone_id.map(|value| value.0))
    .bind(command.title.as_str())
    .bind(command.description.as_ref())
    .bind(priority_as_str(command.priority))
    .bind(event.occurred_at)
    .execute(tx.as_mut())
    .await?;

    sqlx::query(
        "INSERT INTO activity (
            id,
            workspace_id,
            project_id,
            issue_id,
            command_id,
            actor_id,
            event_type,
            event_payload,
            created_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(Uuid::now_v7())
    .bind(command.workspace_id.0)
    .bind(command.project_id.0)
    .bind(command.issue_id.0)
    .bind(command.command_id.0)
    .bind(command.actor_id)
    .bind("issue.created.v1")
    .bind(json!(event))
    .bind(event.occurred_at)
    .execute(tx.as_mut())
    .await?;

    sqlx::query(
        "INSERT INTO outbox (
            id,
            workspace_id,
            event_id,
            event_type,
            aggregate_type,
            aggregate_id,
            occurred_at,
            correlation_id,
            payload,
            attempts,
            available_at,
            status,
            leased_at,
            leased_until,
            published_at,
            last_error,
            created_at,
            updated_at
        ) VALUES (
            $1,
            $2,
            $3,
            $4,
            $5,
            $6,
            $7,
            $8,
            $9,
            0,
            $10,
            'pending'::outbox_status,
            NULL,
            NULL,
            NULL,
            NULL,
            $10,
            $10
        )",
    )
    .bind(Uuid::now_v7())
    .bind(command.workspace_id.0)
    .bind(event.event_id)
    .bind("issue.created.v1")
    .bind("issue")
    .bind(command.issue_id.0)
    .bind(event.occurred_at)
    .bind(command.command_id.0)
    .bind(json!(event))
    .bind(event.occurred_at)
    .execute(tx.as_mut())
    .await?;

    sqlx::query(
        "INSERT INTO idempotency_record (
            workspace_id,
            command_name,
            idempotency_key,
            command_id,
            response_payload,
            expires_at,
            created_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(command.workspace_id.0)
    .bind("issue.create.v1")
    .bind(command.idempotency_key.as_str())
    .bind(command.command_id.0)
    .bind(json!(IdempotencyResponse {
        issue_id: command.issue_id.0,
    }))
    .bind(Utc::now() + Duration::days(7))
    .bind(Utc::now())
    .execute(tx.as_mut())
    .await?;

    let issue = fetch_issue_in_tx(&mut tx, command.issue_id.0)
        .await?
        .ok_or(StoreError::IdempotencyReplayMissingIssue)?;

    tx.commit().await?;

    Ok(CreateIssueWriteResult {
        issue,
        idempotent_replay: false,
    })
}

pub(crate) async fn get_issue(
    store: &PgStore,
    auth: &AuthContext,
    issue_id: Uuid,
) -> Result<Option<Issue>, StoreError> {
    let mut tx = store.pool().begin().await?;
    set_workspace_context_tx(&mut tx, auth.workspace_id.0).await?;
    let issue = fetch_issue_in_tx(&mut tx, issue_id).await?;
    tx.commit().await?;
    Ok(issue)
}

async fn fetch_issue_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    issue_id: Uuid,
) -> Result<Option<Issue>, StoreError> {
    let row = sqlx::query_as::<_, IssueRow>(
        "SELECT
            id,
            workspace_id,
            project_id,
            milestone_id,
            title,
            description,
            state_category::text AS state_category,
            priority::text AS priority,
            archived_at,
            row_version,
            created_at,
            updated_at
        FROM issue
        WHERE id = $1",
    )
    .bind(issue_id)
    .fetch_optional(tx.as_mut())
    .await?;

    row.map(map_issue_row).transpose()
}

fn map_issue_row(row: IssueRow) -> Result<Issue, StoreError> {
    Ok(Issue {
        id: row.id.into(),
        workspace_id: row.workspace_id.into(),
        project_id: row.project_id.into(),
        milestone_id: row.milestone_id.map(Into::into),
        title: row.title,
        description: row.description,
        state_category: parse_state_category(row.state_category)?,
        priority: parse_priority(row.priority)?,
        archived_at: row.archived_at,
        row_version: row.row_version,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn parse_state_category(value: String) -> Result<IssueStateCategory, StoreError> {
    match value.as_str() {
        "open" => Ok(IssueStateCategory::Open),
        "active" => Ok(IssueStateCategory::Active),
        "done" => Ok(IssueStateCategory::Done),
        "cancelled" => Ok(IssueStateCategory::Cancelled),
        _ => Err(StoreError::InvalidState(value)),
    }
}

fn parse_priority(value: String) -> Result<IssuePriority, StoreError> {
    match value.as_str() {
        "low" => Ok(IssuePriority::Low),
        "medium" => Ok(IssuePriority::Medium),
        "high" => Ok(IssuePriority::High),
        "urgent" => Ok(IssuePriority::Urgent),
        _ => Err(StoreError::InvalidPriority(value)),
    }
}

fn priority_as_str(priority: IssuePriority) -> &'static str {
    match priority {
        IssuePriority::Low => "low",
        IssuePriority::Medium => "medium",
        IssuePriority::High => "high",
        IssuePriority::Urgent => "urgent",
    }
}
