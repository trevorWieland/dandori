use std::sync::Arc;

use chrono::Utc;
use dandori_contract::{
    CreateIssueRequest, CreateIssueResponse, ErrorEnvelope, GetIssueResponse, IssueDto,
    IssuePriorityDto, IssueStateCategoryDto,
};
use dandori_domain::{
    AuthContext, CommandId, CreateIssueCommandV1, DomainError, IssueId, IssuePriority,
    IssueStateCategory,
};
use dandori_policy::{Action, PolicyEngine, Resource, RoleMatrixPolicy, Subject};
use dandori_store::{PgStore, StoreError, migrate_database};
use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tracing::error;
use uuid::Uuid;

/// Public-facing message used for every internal/infrastructure error. The
/// real detail is emitted via `tracing::error!` with the correlation id so
/// operators can correlate client-visible failures with private logs without
/// leaking DB internals to callers.
const INTERNAL_ERROR_PUBLIC_MESSAGE: &str = "an internal error occurred";

#[derive(Debug, Clone)]
pub struct IssueAppService {
    store: PgStore,
    policy: Arc<dyn PolicyEngine>,
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
    /// Correlation id attached to every error so operators can tie the
    /// public envelope (which may carry a sanitized message) to the private
    /// structured log line carrying the full detail.
    pub correlation_id: Uuid,
}

impl AppServiceError {
    #[must_use]
    pub fn validation(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            kind: ErrorKind::Validation,
            correlation_id: Uuid::now_v7(),
        }
    }

    /// Build a sanitized infrastructure error. The public `message` is the
    /// constant `INTERNAL_ERROR_PUBLIC_MESSAGE`; the detailed cause is logged
    /// via `tracing::error!` keyed on the fresh correlation id so operators
    /// can trace back without any client ever seeing internals.
    #[must_use]
    pub fn internal(code: &'static str, private_source: impl std::fmt::Display) -> Self {
        let correlation_id = Uuid::now_v7();
        error!(
            correlation_id = %correlation_id,
            error_code = code,
            detail = %private_source,
            "internal error sanitized at app-service boundary"
        );
        Self {
            code,
            message: INTERNAL_ERROR_PUBLIC_MESSAGE.to_owned(),
            kind: ErrorKind::Infrastructure,
            correlation_id,
        }
    }
}

impl IssueAppService {
    #[must_use]
    pub fn new(store: PgStore) -> Self {
        Self::with_policy(store, Arc::new(RoleMatrixPolicy::standard()))
    }

    #[must_use]
    pub fn with_policy(store: PgStore, policy: Arc<dyn PolicyEngine>) -> Self {
        Self { store, policy }
    }

    fn subject_from_auth(auth: &AuthContext) -> Subject {
        // Callers authenticated at the transport boundary are treated as
        // full Members of their workspace. This matches Phase 1 scope
        // (single-tier tenancy) and will tighten when RBAC lands — the
        // transport layer is then the only place that needs to change to
        // surface richer role claims.
        Subject::new(
            auth.actor_id,
            auth.workspace_id.0,
            [dandori_policy::Role::Owner, dandori_policy::Role::Member],
        )
    }

    fn enforce_policy(
        &self,
        auth: &AuthContext,
        action: Action,
        resource: Resource,
    ) -> Result<(), AppServiceError> {
        let subject = Self::subject_from_auth(auth);
        self.policy
            .evaluate(Some(&subject), action, resource)
            .into_result()
            .map_err(|err| {
                dandori_observability::metrics::increment_counter(
                    dandori_observability::metrics::names::API_AUTHZ_DENIED,
                    1,
                );
                AppServiceError {
                    code: err.0.code(),
                    message: err.0.message().to_owned(),
                    kind: ErrorKind::Authz,
                    correlation_id: Uuid::now_v7(),
                }
            })
    }

    pub async fn create_issue(
        &self,
        auth: &AuthContext,
        request: CreateIssueRequest,
    ) -> Result<CreateIssueResponse, AppServiceError> {
        self.enforce_policy(
            auth,
            Action::IssueCreate,
            Resource::Workspace {
                workspace_id: auth.workspace_id.0,
            },
        )?;
        let request_fingerprint = create_issue_fingerprint(&request)?;
        let idempotency_key = dandori_domain::IdempotencyKey::new(request.idempotency_key)
            .map_err(Self::map_domain_error)?;
        let command = CreateIssueCommandV1 {
            command_id: CommandId(Uuid::now_v7()),
            idempotency_key,
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
        self.enforce_policy(
            auth,
            Action::IssueRead,
            Resource::Issue {
                workspace_id: auth.workspace_id.0,
                issue_id,
            },
        )?;
        let issue = self
            .store
            .get_issue(auth, issue_id)
            .await
            .map_err(|error| Self::map_store_error_read(error, issue_id))?
            .ok_or_else(|| AppServiceError {
                code: "issue_not_found",
                message: format!("issue '{issue_id}' was not found"),
                kind: ErrorKind::NotFound,
                correlation_id: Uuid::now_v7(),
            })?;

        Ok(GetIssueResponse {
            issue: map_issue_to_dto(issue),
        })
    }

    fn map_domain_error(error: DomainError) -> AppServiceError {
        let correlation_id = Uuid::now_v7();
        match error {
            DomainError::Validation(inner) => AppServiceError {
                code: inner.code,
                message: inner.message,
                kind: ErrorKind::Validation,
                correlation_id,
            },
            DomainError::Precondition(inner) => AppServiceError {
                code: inner.code,
                message: inner.message,
                kind: ErrorKind::Precondition,
                correlation_id,
            },
            DomainError::Conflict(inner) => AppServiceError {
                code: inner.code,
                message: inner.message,
                kind: ErrorKind::Conflict,
                correlation_id,
            },
            DomainError::Authz(inner) => AppServiceError {
                code: inner.code,
                message: inner.message,
                kind: ErrorKind::Authz,
                correlation_id,
            },
            DomainError::TenantBoundary(inner) => AppServiceError {
                code: inner.code,
                message: inner.message,
                kind: ErrorKind::TenantBoundary,
                correlation_id,
            },
            DomainError::Infrastructure(inner) => {
                // Infrastructure DomainErrors carry internal detail that must
                // not reach clients. Sanitize while logging the original.
                AppServiceError::internal(inner.code, inner.message)
            }
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
                correlation_id: Uuid::now_v7(),
            },
            StoreError::MilestoneProjectMismatch => AppServiceError {
                code: "milestone_project_mismatch",
                message: "milestone does not belong to the requested project".to_owned(),
                kind: ErrorKind::Precondition,
                correlation_id: Uuid::now_v7(),
            },
            StoreError::IdempotencyConflict => {
                Self::map_domain_error(command.map_duplicate_conflict())
            }
            StoreError::Domain(domain) => Self::map_domain_error(domain),
            other => AppServiceError::internal("store_write_failed", other),
        }
    }

    fn map_store_error_read(error: StoreError, _issue_id: Uuid) -> AppServiceError {
        match error {
            StoreError::Domain(domain) => Self::map_domain_error(domain),
            other => AppServiceError::internal("store_read_failed", other),
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
            .map_err(|error| AppServiceError::internal("migration_failed", error))?;
    }

    let store = PgStore::connect(database_url)
        .await
        .map_err(|error| AppServiceError::internal("store_connect_failed", error))?;

    Ok(IssueAppService::new(store))
}

#[must_use]
pub fn map_error_to_transport(error: AppServiceError) -> ErrorEnvelope {
    ErrorEnvelope {
        code: error.code.to_owned(),
        message: error.message,
        correlation_id: Some(error.correlation_id),
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
    AppServiceError::validation(code, message)
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

    let bytes = serde_json::to_vec(&payload)
        .map_err(|error| AppServiceError::internal("fingerprint_serialize_failed", error))?;

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
