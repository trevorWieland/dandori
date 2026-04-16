use chrono::Utc;
use dandori_contract::{
    CreateIssueRequest, CreateIssueResponse, ErrorEnvelope, GetIssueResponse, IssueDto,
    IssuePriorityDto, IssueStateCategoryDto,
};
use dandori_domain::{
    AuthContext, CommandId, CreateIssueCommandV1, DomainError, IssueId, IssuePriority,
    IssueStateCategory,
};
use dandori_store::{PgStore, StoreError, migrate_database};
use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct IssueAppService {
    store: PgStore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Validation,
    Precondition,
    Conflict,
    NotFound,
    Authz,
    TenantBoundary,
    Infrastructure,
}

#[derive(Debug, Error)]
#[error("{code}: {message}")]
pub struct AppServiceError {
    pub code: &'static str,
    pub message: String,
    pub kind: ErrorKind,
}

impl IssueAppService {
    #[must_use]
    pub fn new(store: PgStore) -> Self {
        Self { store }
    }

    pub async fn create_issue(
        &self,
        auth: &AuthContext,
        request: CreateIssueRequest,
    ) -> Result<CreateIssueResponse, AppServiceError> {
        let request_fingerprint = create_issue_fingerprint(&request)?;
        let command = CreateIssueCommandV1 {
            command_id: CommandId(Uuid::now_v7()),
            idempotency_key: dandori_domain::IdempotencyKey(request.idempotency_key),
            request_fingerprint,
            issue_id: IssueId::new(),
            workspace_id: auth.workspace_id,
            project_id: request.project_id.into(),
            milestone_id: request.milestone_id.map(Into::into),
            title: request.title,
            description: request.description,
            priority: map_priority_from_dto(request.priority),
            actor_id: auth.actor_id,
        };

        command.validate().map_err(Self::map_domain_error)?;
        auth.enforce_workspace(command.workspace_id)
            .map_err(Self::map_domain_error)?;

        let event = dandori_domain::IssueCreatedEventV1 {
            event_id: Uuid::now_v7(),
            issue_id: command.issue_id,
            workspace_id: command.workspace_id,
            project_id: command.project_id,
            milestone_id: command.milestone_id,
            actor_id: command.actor_id,
            occurred_at: Utc::now(),
            title: command.title.clone(),
            description: command.description.clone(),
            priority: command.priority,
        };

        let write_result = self
            .store
            .create_issue_transactional(auth, &command, &event)
            .await
            .map_err(|error| Self::map_store_error(error, &command))?;

        Ok(CreateIssueResponse {
            issue: map_issue_to_dto(write_result.issue),
            idempotent_replay: write_result.idempotent_replay,
        })
    }

    pub async fn get_issue(
        &self,
        auth: &AuthContext,
        issue_id: Uuid,
    ) -> Result<GetIssueResponse, AppServiceError> {
        let issue = self
            .store
            .get_issue(auth, issue_id)
            .await
            .map_err(|error| Self::map_store_error_read(error, issue_id))?
            .ok_or_else(|| AppServiceError {
                code: "issue_not_found",
                message: format!("issue '{issue_id}' was not found"),
                kind: ErrorKind::NotFound,
            })?;

        Ok(GetIssueResponse {
            issue: map_issue_to_dto(issue),
        })
    }

    fn map_domain_error(error: DomainError) -> AppServiceError {
        match error {
            DomainError::Validation(inner) => AppServiceError {
                code: inner.code,
                message: inner.message,
                kind: ErrorKind::Validation,
            },
            DomainError::Precondition(inner) => AppServiceError {
                code: inner.code,
                message: inner.message,
                kind: ErrorKind::Precondition,
            },
            DomainError::Conflict(inner) => AppServiceError {
                code: inner.code,
                message: inner.message,
                kind: ErrorKind::Conflict,
            },
            DomainError::Authz(inner) => AppServiceError {
                code: inner.code,
                message: inner.message,
                kind: ErrorKind::Authz,
            },
            DomainError::TenantBoundary(inner) => AppServiceError {
                code: inner.code,
                message: inner.message,
                kind: ErrorKind::TenantBoundary,
            },
            DomainError::Infrastructure(inner) => AppServiceError {
                code: inner.code,
                message: inner.message,
                kind: ErrorKind::Infrastructure,
            },
        }
    }

    fn map_store_error(error: StoreError, command: &CreateIssueCommandV1) -> AppServiceError {
        match error {
            StoreError::ProjectNotFound => {
                Self::map_domain_error(command.map_missing_project_precondition())
            }
            StoreError::MilestoneNotFound => AppServiceError {
                code: "milestone_not_found",
                message: "milestone not found in workspace".to_owned(),
                kind: ErrorKind::Precondition,
            },
            StoreError::MilestoneProjectMismatch => AppServiceError {
                code: "milestone_project_mismatch",
                message: "milestone does not belong to the requested project".to_owned(),
                kind: ErrorKind::Precondition,
            },
            StoreError::IdempotencyConflict => {
                Self::map_domain_error(command.map_duplicate_conflict())
            }
            StoreError::Domain(domain) => Self::map_domain_error(domain),
            other => AppServiceError {
                code: "store_write_failed",
                message: other.to_string(),
                kind: ErrorKind::Infrastructure,
            },
        }
    }

    fn map_store_error_read(error: StoreError, issue_id: Uuid) -> AppServiceError {
        match error {
            StoreError::Domain(domain) => Self::map_domain_error(domain),
            other => AppServiceError {
                code: "store_read_failed",
                message: format!("failed to read issue '{issue_id}': {other}"),
                kind: ErrorKind::Infrastructure,
            },
        }
    }
}

pub async fn build_issue_service(
    database_url: &str,
    run_migrations: bool,
) -> Result<IssueAppService, AppServiceError> {
    if run_migrations {
        migrate_database(database_url)
            .await
            .map_err(|error| AppServiceError {
                code: "migration_failed",
                message: error.to_string(),
                kind: ErrorKind::Infrastructure,
            })?;
    }

    let store = PgStore::connect(database_url)
        .await
        .map_err(|error| AppServiceError {
            code: "store_connect_failed",
            message: error.to_string(),
            kind: ErrorKind::Infrastructure,
        })?;

    Ok(IssueAppService::new(store))
}

#[must_use]
pub fn map_error_to_transport(error: AppServiceError) -> ErrorEnvelope {
    ErrorEnvelope {
        code: error.code.to_owned(),
        message: error.message,
    }
}

fn map_priority_from_dto(priority: IssuePriorityDto) -> IssuePriority {
    match priority {
        IssuePriorityDto::Low => IssuePriority::Low,
        IssuePriorityDto::Medium => IssuePriority::Medium,
        IssuePriorityDto::High => IssuePriority::High,
        IssuePriorityDto::Urgent => IssuePriority::Urgent,
    }
}

fn map_state_to_dto(state: IssueStateCategory) -> IssueStateCategoryDto {
    match state {
        IssueStateCategory::Open => IssueStateCategoryDto::Open,
        IssueStateCategory::Active => IssueStateCategoryDto::Active,
        IssueStateCategory::Done => IssueStateCategoryDto::Done,
        IssueStateCategory::Cancelled => IssueStateCategoryDto::Cancelled,
    }
}

fn map_priority_to_dto(priority: IssuePriority) -> IssuePriorityDto {
    match priority {
        IssuePriority::Low => IssuePriorityDto::Low,
        IssuePriority::Medium => IssuePriorityDto::Medium,
        IssuePriority::High => IssuePriorityDto::High,
        IssuePriority::Urgent => IssuePriorityDto::Urgent,
    }
}

fn map_issue_to_dto(issue: dandori_domain::Issue) -> IssueDto {
    IssueDto {
        id: issue.id.0,
        workspace_id: issue.workspace_id.0,
        project_id: issue.project_id.0,
        milestone_id: issue.milestone_id.map(|value| value.0),
        title: issue.title,
        description: issue.description,
        state_category: map_state_to_dto(issue.state_category),
        priority: map_priority_to_dto(issue.priority),
        archived_at: issue.archived_at,
        row_version: issue.row_version,
        created_at: issue.created_at,
        updated_at: issue.updated_at,
    }
}

#[must_use]
pub fn validation_error(code: &'static str, message: String) -> AppServiceError {
    AppServiceError {
        code,
        message,
        kind: ErrorKind::Validation,
    }
}

fn create_issue_fingerprint(request: &CreateIssueRequest) -> Result<String, AppServiceError> {
    #[derive(Serialize)]
    struct FingerprintPayload<'a> {
        schema: &'static str,
        project_id: Uuid,
        milestone_id: Option<Uuid>,
        title: &'a str,
        description: Option<&'a str>,
        priority: &'static str,
    }

    let payload = FingerprintPayload {
        schema: "issue.create.fingerprint.v2",
        project_id: request.project_id,
        milestone_id: request.milestone_id,
        title: request.title.as_str(),
        description: request.description.as_deref(),
        priority: priority_literal(request.priority),
    };

    let bytes = serde_json::to_vec(&payload).map_err(|error| AppServiceError {
        code: "fingerprint_serialize_failed",
        message: format!("failed to serialize idempotency fingerprint payload: {error}"),
        kind: ErrorKind::Infrastructure,
    })?;

    let digest = Sha256::digest(bytes);
    Ok(format!("v2:{}", hex::encode(digest)))
}

fn priority_literal(priority: IssuePriorityDto) -> &'static str {
    match priority {
        IssuePriorityDto::Low => "low",
        IssuePriorityDto::Medium => "medium",
        IssuePriorityDto::High => "high",
        IssuePriorityDto::Urgent => "urgent",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_request() -> CreateIssueRequest {
        CreateIssueRequest {
            idempotency_key: "idem".to_owned(),
            project_id: Uuid::now_v7(),
            milestone_id: None,
            title: "title".to_owned(),
            description: None,
            priority: IssuePriorityDto::Medium,
        }
    }

    #[test]
    fn fingerprint_is_versioned_and_hash_based() {
        let request = base_request();
        let fingerprint = create_issue_fingerprint(&request).expect("fingerprint");
        assert!(fingerprint.starts_with("v2:"));
        assert_eq!(fingerprint.len(), 67);
    }

    #[test]
    fn fingerprint_distinguishes_none_and_empty_description() {
        let request_none = base_request();
        let mut request_empty = base_request();
        request_empty.description = Some(String::new());

        let none_fingerprint = create_issue_fingerprint(&request_none).expect("none fingerprint");
        let empty_fingerprint =
            create_issue_fingerprint(&request_empty).expect("empty fingerprint");

        assert_ne!(none_fingerprint, empty_fingerprint);
    }
}
