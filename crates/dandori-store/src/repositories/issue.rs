use chrono::{DateTime, Duration, FixedOffset, Utc};
use dandori_domain::{
    AuthContext, CommandName, CreateIssueCommandV1, EventType, Issue, IssueCreatedEventV1,
    IssuePriority, IssueStateCategory,
};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::pg_store::{CreateIssueWriteResult, IdempotencyResponse, PgStore};
use crate::{StoreError, repositories::common::set_workspace_context_tx};

/// Full issue-create write path. Idempotency is reserved atomically as the
/// **first** statement in the transaction, with a placeholder payload. The
/// `(xmax = 0)` sentinel from the `INSERT … ON CONFLICT … RETURNING` tells
/// us whether this transaction won the race (proceeds with writes) or lost
/// it (short-circuits with the stored replay payload). There is no
/// pre-read round-trip and no write-then-rollback on races.
pub(crate) async fn create_issue_transactional(
    store: &PgStore,
    auth: &AuthContext,
    command: &CreateIssueCommandV1,
    event: &IssueCreatedEventV1,
) -> Result<CreateIssueWriteResult, StoreError> {
    let occurred_at = event.occurred_at.fixed_offset();
    let idempotency_now = Utc::now().fixed_offset();
    let idempotency_expires = (Utc::now() + Duration::days(7)).fixed_offset();
    let placeholder_payload = serde_json::json!({ "status": "reserving" });

    let mut tx = store.pool().begin().await?;
    set_workspace_context_tx(&mut tx, auth.workspace_id.0).await?;

    let reservation = reserve_idempotency(
        &mut tx,
        command,
        &placeholder_payload,
        idempotency_now,
        idempotency_expires,
    )
    .await?;

    if !reservation.inserted {
        // Race loss path: no writes have happened yet, so we just short-circuit
        // based on whether the fingerprint of this request matches the winner.
        tx.commit().await?;
        dandori_observability::metrics::increment_counter(
            dandori_observability::metrics::names::STORE_IDEMPOTENCY_REPLAY,
            1,
        );
        return branch_on_stored_fingerprint(command, reservation);
    }

    ensure_project_exists(&mut tx, command).await?;
    ensure_milestone_belongs_to_project(&mut tx, command).await?;

    let issue = insert_issue_returning(&mut tx, command, occurred_at).await?;
    insert_activity(&mut tx, command, event, occurred_at).await?;
    insert_outbox(&mut tx, command, event, occurred_at).await?;

    let final_payload = serde_json::to_value(IdempotencyResponse {
        issue: issue.clone(),
    })?;
    update_idempotency_payload(&mut tx, command, &final_payload).await?;

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

async fn ensure_project_exists(
    tx: &mut Transaction<'_, Postgres>,
    command: &CreateIssueCommandV1,
) -> Result<(), StoreError> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (
               SELECT 1 FROM project WHERE id = $1 AND workspace_id = $2
           ) as "exists!""#,
        command.project_id.0,
        command.workspace_id.0,
    )
    .fetch_one(tx.as_mut())
    .await?;
    if !exists {
        return Err(StoreError::ProjectNotFound);
    }
    Ok(())
}

async fn ensure_milestone_belongs_to_project(
    tx: &mut Transaction<'_, Postgres>,
    command: &CreateIssueCommandV1,
) -> Result<(), StoreError> {
    let Some(milestone_id) = command.milestone_id else {
        return Ok(());
    };

    let row = sqlx::query!(
        r#"SELECT project_id as "project_id!: Uuid"
           FROM milestone
           WHERE id = $1 AND workspace_id = $2"#,
        milestone_id.0,
        command.workspace_id.0,
    )
    .fetch_optional(tx.as_mut())
    .await?;

    let row = row.ok_or(StoreError::MilestoneNotFound)?;
    if row.project_id != command.project_id.0 {
        return Err(StoreError::MilestoneProjectMismatch);
    }
    Ok(())
}

async fn insert_issue_returning(
    tx: &mut Transaction<'_, Postgres>,
    command: &CreateIssueCommandV1,
    occurred_at: DateTime<FixedOffset>,
) -> Result<Issue, StoreError> {
    let priority_text = priority_as_str(command.priority).to_owned();
    let milestone_id = command.milestone_id.map(|value| value.0);
    let row = sqlx::query_as!(
        IssueRow,
        r#"INSERT INTO issue (
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
               $7::text::issue_priority,
               NULL,
               0,
               $8,
               $8
           )
           RETURNING
               id as "id!: Uuid",
               workspace_id as "workspace_id!: Uuid",
               project_id as "project_id!: Uuid",
               milestone_id as "milestone_id: Uuid",
               title,
               description,
               state_category::text as "state_category!",
               priority::text as "priority!",
               archived_at as "archived_at: DateTime<FixedOffset>",
               row_version,
               created_at as "created_at!: DateTime<FixedOffset>",
               updated_at as "updated_at!: DateTime<FixedOffset>""#,
        command.issue_id.0,
        command.workspace_id.0,
        command.project_id.0,
        milestone_id,
        command.title,
        command.description,
        priority_text,
        occurred_at,
    )
    .fetch_one(tx.as_mut())
    .await?;
    map_issue_row(row)
}

async fn insert_activity(
    tx: &mut Transaction<'_, Postgres>,
    command: &CreateIssueCommandV1,
    event: &IssueCreatedEventV1,
    occurred_at: DateTime<FixedOffset>,
) -> Result<(), StoreError> {
    let activity_id = Uuid::now_v7();
    let event_payload = serde_json::to_value(event)?;
    let issue_id = Some(command.issue_id.0);
    sqlx::query!(
        r#"INSERT INTO activity (
               id,
               workspace_id,
               project_id,
               issue_id,
               command_id,
               actor_id,
               event_type,
               event_payload,
               created_at
           ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
        activity_id,
        command.workspace_id.0,
        command.project_id.0,
        issue_id,
        command.command_id.0,
        command.actor_id,
        EventType::IssueCreatedV1.as_str(),
        event_payload,
        occurred_at,
    )
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

async fn insert_outbox(
    tx: &mut Transaction<'_, Postgres>,
    command: &CreateIssueCommandV1,
    event: &IssueCreatedEventV1,
    occurred_at: DateTime<FixedOffset>,
) -> Result<(), StoreError> {
    let outbox_id = Uuid::now_v7();
    let payload = serde_json::to_value(event)?;
    sqlx::query!(
        r#"INSERT INTO outbox (
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
               lease_token,
               lease_owner,
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
               NULL,
               NULL,
               $10,
               $10
           )"#,
        outbox_id,
        command.workspace_id.0,
        event.event_id,
        EventType::IssueCreatedV1.as_str(),
        "issue",
        command.issue_id.0,
        occurred_at,
        command.command_id.0,
        payload,
        occurred_at,
    )
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

struct IdempotencyReservation {
    inserted: bool,
    stored_fingerprint: String,
    stored_payload: serde_json::Value,
}

async fn reserve_idempotency(
    tx: &mut Transaction<'_, Postgres>,
    command: &CreateIssueCommandV1,
    response_payload: &serde_json::Value,
    now: DateTime<FixedOffset>,
    expires_at: DateTime<FixedOffset>,
) -> Result<IdempotencyReservation, StoreError> {
    let row = sqlx::query!(
        r#"INSERT INTO idempotency_record (
               workspace_id,
               command_name,
               idempotency_key,
               request_fingerprint,
               response_payload,
               expires_at,
               created_at
           ) VALUES ($1, $2, $3, $4, $5, $6, $7)
           ON CONFLICT (workspace_id, command_name, idempotency_key)
           DO UPDATE SET expires_at = idempotency_record.expires_at
           RETURNING
               (xmax = 0) as "inserted!",
               request_fingerprint as "stored_fingerprint!",
               response_payload as "stored_payload!: serde_json::Value""#,
        command.workspace_id.0,
        CommandName::IssueCreateV1.as_str(),
        command.idempotency_key.as_str(),
        command.request_fingerprint,
        response_payload,
        expires_at,
        now,
    )
    .fetch_one(tx.as_mut())
    .await?;

    Ok(IdempotencyReservation {
        inserted: row.inserted,
        stored_fingerprint: row.stored_fingerprint,
        stored_payload: row.stored_payload,
    })
}

fn branch_on_stored_fingerprint(
    command: &CreateIssueCommandV1,
    reservation: IdempotencyReservation,
) -> Result<CreateIssueWriteResult, StoreError> {
    if !matches_request_fingerprint(reservation.stored_fingerprint.as_str(), command) {
        return Err(StoreError::IdempotencyConflict);
    }
    let replay: IdempotencyResponse = serde_json::from_value(reservation.stored_payload)?;
    Ok(CreateIssueWriteResult {
        issue: replay.issue,
        idempotent_replay: true,
    })
}

async fn update_idempotency_payload(
    tx: &mut Transaction<'_, Postgres>,
    command: &CreateIssueCommandV1,
    response_payload: &serde_json::Value,
) -> Result<(), StoreError> {
    sqlx::query!(
        r#"UPDATE idempotency_record
           SET response_payload = $4
           WHERE workspace_id = $1
             AND command_name = $2
             AND idempotency_key = $3"#,
        command.workspace_id.0,
        CommandName::IssueCreateV1.as_str(),
        command.idempotency_key.as_str(),
        response_payload,
    )
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

struct IssueRow {
    id: Uuid,
    workspace_id: Uuid,
    project_id: Uuid,
    milestone_id: Option<Uuid>,
    title: String,
    description: Option<String>,
    state_category: String,
    priority: String,
    archived_at: Option<DateTime<FixedOffset>>,
    row_version: i64,
    created_at: DateTime<FixedOffset>,
    updated_at: DateTime<FixedOffset>,
}

async fn fetch_issue_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    issue_id: Uuid,
) -> Result<Option<Issue>, StoreError> {
    let row = sqlx::query_as!(
        IssueRow,
        r#"SELECT
               id as "id!: Uuid",
               workspace_id as "workspace_id!: Uuid",
               project_id as "project_id!: Uuid",
               milestone_id as "milestone_id: Uuid",
               title,
               description,
               state_category::text as "state_category!",
               priority::text as "priority!",
               archived_at as "archived_at: DateTime<FixedOffset>",
               row_version,
               created_at as "created_at!: DateTime<FixedOffset>",
               updated_at as "updated_at!: DateTime<FixedOffset>"
           FROM issue
           WHERE id = $1"#,
        issue_id,
    )
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
        archived_at: row.archived_at.map(|value| value.with_timezone(&Utc)),
        row_version: row.row_version,
        created_at: row.created_at.with_timezone(&Utc),
        updated_at: row.updated_at.with_timezone(&Utc),
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

fn matches_request_fingerprint(existing: &str, command: &CreateIssueCommandV1) -> bool {
    if existing == command.request_fingerprint {
        return true;
    }

    !existing.starts_with("v2:") && existing == legacy_v1_fingerprint(command)
}

fn legacy_v1_fingerprint(command: &CreateIssueCommandV1) -> String {
    format!(
        "project_id={}|milestone_id={}|title={}|description={}|priority={}",
        command.project_id.0,
        command
            .milestone_id
            .map_or_else(|| "null".to_owned(), |id| id.0.to_string()),
        command.title,
        command.description.clone().unwrap_or_default(),
        priority_as_str(command.priority),
    )
}
