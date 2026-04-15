use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    CommandId, ConflictError, DomainError, IdempotencyKey, IssueId, IssuePriority, MilestoneId,
    PreconditionError, ProjectId, ValidationError, WorkspaceId,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum IssueCommandV1 {
    CreateIssue(CreateIssueCommandV1),
}

impl IssueCommandV1 {
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        match self {
            Self::CreateIssue(_) => "issue.create.v1",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateIssueCommandV1 {
    pub command_id: CommandId,
    pub idempotency_key: IdempotencyKey,
    pub issue_id: IssueId,
    pub workspace_id: WorkspaceId,
    pub project_id: ProjectId,
    pub milestone_id: Option<MilestoneId>,
    pub title: String,
    pub description: Option<String>,
    pub priority: IssuePriority,
    pub actor_id: Uuid,
}

impl CreateIssueCommandV1 {
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.title.trim().is_empty() {
            return Err(DomainError::Validation(ValidationError {
                code: "title_required",
                message: "issue title must not be empty".to_owned(),
            }));
        }
        if self.title.len() > 200 {
            return Err(DomainError::Validation(ValidationError {
                code: "title_too_long",
                message: "issue title exceeds 200 characters".to_owned(),
            }));
        }
        if self.idempotency_key.as_str().trim().is_empty() {
            return Err(DomainError::Validation(ValidationError {
                code: "idempotency_key_required",
                message: "idempotency key must not be empty".to_owned(),
            }));
        }
        if self.idempotency_key.as_str().len() > 128 {
            return Err(DomainError::Validation(ValidationError {
                code: "idempotency_key_too_long",
                message: "idempotency key exceeds 128 characters".to_owned(),
            }));
        }
        Ok(())
    }

    pub fn map_duplicate_conflict(&self) -> DomainError {
        DomainError::Conflict(ConflictError {
            code: "duplicate_issue_command",
            message: format!(
                "issue command with idempotency key '{}' already exists with a different command",
                self.idempotency_key.as_str()
            ),
        })
    }

    pub fn map_missing_project_precondition(&self) -> DomainError {
        DomainError::Precondition(PreconditionError {
            code: "project_not_found",
            message: format!("project '{}' not found in workspace", self.project_id),
        })
    }
}
