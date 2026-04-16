use chrono::{Duration, Utc};
use dandori_domain::{
    AuthContext, CreateIssueCommandV1, Issue, IssueCreatedEventV1, IssuePriority,
    IssueStateCategory,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseTransaction,
    EntityTrait, QueryFilter, QueryResult, Set, Statement, TransactionTrait,
};
use uuid::Uuid;

use crate::entities::{activity, idempotency_record, milestone, project};
use crate::pg_store::{CreateIssueWriteResult, IdempotencyResponse, PgStore};
use crate::{StoreError, repositories::common::set_workspace_context_db};

const ISSUE_CREATE_COMMAND_NAME: &str = "issue.create.v1";

pub(crate) async fn create_issue_transactional(
    store: &PgStore,
    auth: &AuthContext,
    command: &CreateIssueCommandV1,
    event: &IssueCreatedEventV1,
) -> Result<CreateIssueWriteResult, StoreError> {
    let tx = store.db().begin().await?;
    set_workspace_context_db(&tx, auth.workspace_id.0).await?;

    let maybe_idempotency = idempotency_record::Entity::find_by_id((
        command.workspace_id.0,
        ISSUE_CREATE_COMMAND_NAME.to_owned(),
        command.idempotency_key.as_str().to_owned(),
    ))
    .one(&tx)
    .await?;

    if let Some(existing) = maybe_idempotency {
        if !matches_request_fingerprint(existing.request_fingerprint.as_str(), command) {
            return Err(StoreError::IdempotencyConflict);
        }
        let replay: IdempotencyResponse = serde_json::from_value(existing.response_payload)?;
        tx.commit().await?;
        return Ok(CreateIssueWriteResult {
            issue: replay.issue,
            idempotent_replay: true,
        });
    }

    let project_exists = project::Entity::find()
        .filter(project::Column::Id.eq(command.project_id.0))
        .filter(project::Column::WorkspaceId.eq(command.workspace_id.0))
        .one(&tx)
        .await?
        .is_some();
    if !project_exists {
        return Err(StoreError::ProjectNotFound);
    }

    if let Some(milestone_id) = command.milestone_id {
        let milestone_model = milestone::Entity::find()
            .filter(milestone::Column::Id.eq(milestone_id.0))
            .filter(milestone::Column::WorkspaceId.eq(command.workspace_id.0))
            .one(&tx)
            .await?;

        let milestone_model = milestone_model.ok_or(StoreError::MilestoneNotFound)?;
        if milestone_model.project_id != command.project_id.0 {
            return Err(StoreError::MilestoneProjectMismatch);
        }
    }

    let occurred_at = event.occurred_at.fixed_offset();

    tx.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
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
        vec![
            command.issue_id.0.into(),
            command.workspace_id.0.into(),
            command.project_id.0.into(),
            command.milestone_id.map(|value| value.0).into(),
            command.title.clone().into(),
            command.description.clone().into(),
            priority_as_str(command.priority).to_owned().into(),
            occurred_at.into(),
        ],
    ))
    .await?;

    activity::ActiveModel {
        id: Set(Uuid::now_v7()),
        workspace_id: Set(command.workspace_id.0),
        project_id: Set(command.project_id.0),
        issue_id: Set(Some(command.issue_id.0)),
        command_id: Set(command.command_id.0),
        actor_id: Set(command.actor_id),
        event_type: Set("issue.created.v1".to_owned()),
        event_payload: Set(serde_json::json!(event)),
        created_at: Set(occurred_at),
    }
    .insert(&tx)
    .await?;

    tx.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
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
        )",
        vec![
            Uuid::now_v7().into(),
            command.workspace_id.0.into(),
            event.event_id.into(),
            "issue.created.v1".to_owned().into(),
            "issue".to_owned().into(),
            command.issue_id.0.into(),
            occurred_at.into(),
            command.command_id.0.into(),
            serde_json::json!(event).into(),
            occurred_at.into(),
        ],
    ))
    .await?;

    let issue = fetch_issue_in_tx(&tx, command.issue_id.0)
        .await?
        .ok_or(StoreError::IdempotencyReplayMissingIssue)?;

    let idempotency_now = Utc::now().fixed_offset();
    idempotency_record::ActiveModel {
        workspace_id: Set(command.workspace_id.0),
        command_name: Set(ISSUE_CREATE_COMMAND_NAME.to_owned()),
        idempotency_key: Set(command.idempotency_key.as_str().to_owned()),
        request_fingerprint: Set(command.request_fingerprint.clone()),
        response_payload: Set(serde_json::json!(IdempotencyResponse {
            issue: issue.clone(),
        })),
        expires_at: Set((Utc::now() + Duration::days(7)).fixed_offset()),
        created_at: Set(idempotency_now),
    }
    .insert(&tx)
    .await?;

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
    let tx = store.db().begin().await?;
    set_workspace_context_db(&tx, auth.workspace_id.0).await?;
    let issue = fetch_issue_in_tx(&tx, issue_id).await?;
    tx.commit().await?;
    Ok(issue)
}

async fn fetch_issue_in_tx(
    tx: &DatabaseTransaction,
    issue_id: Uuid,
) -> Result<Option<Issue>, StoreError> {
    let row = tx
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"SELECT
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
               WHERE id = $1"#,
            vec![issue_id.into()],
        ))
        .await?;
    row.map(map_issue_row).transpose()
}

fn map_issue_row(row: QueryResult) -> Result<Issue, StoreError> {
    Ok(Issue {
        id: row.try_get::<Uuid>("", "id")?.into(),
        workspace_id: row.try_get::<Uuid>("", "workspace_id")?.into(),
        project_id: row.try_get::<Uuid>("", "project_id")?.into(),
        milestone_id: row
            .try_get::<Option<Uuid>>("", "milestone_id")?
            .map(Into::into),
        title: row.try_get("", "title")?,
        description: row.try_get("", "description")?,
        state_category: parse_state_category(row.try_get("", "state_category")?)?,
        priority: parse_priority(row.try_get("", "priority")?)?,
        archived_at: row
            .try_get::<Option<chrono::DateTime<chrono::FixedOffset>>>("", "archived_at")?
            .map(|value| value.with_timezone(&Utc)),
        row_version: row.try_get("", "row_version")?,
        created_at: row
            .try_get::<chrono::DateTime<chrono::FixedOffset>>("", "created_at")?
            .with_timezone(&Utc),
        updated_at: row
            .try_get::<chrono::DateTime<chrono::FixedOffset>>("", "updated_at")?
            .with_timezone(&Utc),
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
