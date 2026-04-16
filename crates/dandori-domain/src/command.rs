use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    CommandId, ConflictError, DomainError, IdempotencyKey, IssueId, IssuePriority, MilestoneId,
    PreconditionError, ProjectId, ValidationError, WorkspaceId,
};

const MAX_NAME_LENGTH: usize = 200;
const MAX_IDEMPOTENCY_KEY_LENGTH: usize = 128;
const MAX_REQUEST_FINGERPRINT_LENGTH: usize = 128;
const MAX_ISSUE_DESCRIPTION_LENGTH: usize = 4000;

/// Stable, versioned command identifier used anywhere a command name crosses
/// a process, storage, or wire boundary. The only sanctioned source of truth
/// for command identifier strings; downstream code must route through
/// [`CommandName::as_str`] / [`CommandName::parse`] so the compiler catches
/// drift at build time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CommandName {
    IssueCreateV1,
    WorkspaceCreateV1,
    ProjectCreateV1,
}

impl CommandName {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IssueCreateV1 => "issue.create.v1",
            Self::WorkspaceCreateV1 => "workspace.create.v1",
            Self::ProjectCreateV1 => "project.create.v1",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "issue.create.v1" => Some(Self::IssueCreateV1),
            "workspace.create.v1" => Some(Self::WorkspaceCreateV1),
            "project.create.v1" => Some(Self::ProjectCreateV1),
            _ => None,
        }
    }
}

impl std::fmt::Display for CommandName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum IssueCommandV1 {
    CreateIssue(CreateIssueCommandV1),
}

impl IssueCommandV1 {
    #[must_use]
    pub fn name(&self) -> CommandName {
        match self {
            Self::CreateIssue(_) => CommandName::IssueCreateV1,
        }
    }

    #[must_use]
    pub fn command_name(&self) -> &'static str {
        self.name().as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum WorkspaceCommandV1 {
    CreateWorkspace(CreateWorkspaceCommandV1),
}

impl WorkspaceCommandV1 {
    #[must_use]
    pub fn name(&self) -> CommandName {
        match self {
            Self::CreateWorkspace(_) => CommandName::WorkspaceCreateV1,
        }
    }

    #[must_use]
    pub fn command_name(&self) -> &'static str {
        self.name().as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateWorkspaceCommandV1 {
    pub command_id: CommandId,
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub actor_id: Uuid,
}

impl CreateWorkspaceCommandV1 {
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.name.trim().is_empty() {
            return Err(DomainError::Validation(ValidationError {
                code: "workspace_name_required",
                message: "workspace name must not be empty".to_owned(),
            }));
        }
        if self.name.len() > MAX_NAME_LENGTH {
            return Err(DomainError::Validation(ValidationError {
                code: "workspace_name_too_long",
                message: "workspace name exceeds 200 characters".to_owned(),
            }));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ProjectCommandV1 {
    CreateProject(CreateProjectCommandV1),
}

impl ProjectCommandV1 {
    #[must_use]
    pub fn name(&self) -> CommandName {
        match self {
            Self::CreateProject(_) => CommandName::ProjectCreateV1,
        }
    }

    #[must_use]
    pub fn command_name(&self) -> &'static str {
        self.name().as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateProjectCommandV1 {
    pub command_id: CommandId,
    pub workspace_id: WorkspaceId,
    pub project_id: ProjectId,
    pub workflow_version_id: Uuid,
    pub name: String,
    pub actor_id: Uuid,
}

impl CreateProjectCommandV1 {
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.name.trim().is_empty() {
            return Err(DomainError::Validation(ValidationError {
                code: "project_name_required",
                message: "project name must not be empty".to_owned(),
            }));
        }
        if self.name.len() > MAX_NAME_LENGTH {
            return Err(DomainError::Validation(ValidationError {
                code: "project_name_too_long",
                message: "project name exceeds 200 characters".to_owned(),
            }));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateIssueCommandV1 {
    pub command_id: CommandId,
    pub idempotency_key: IdempotencyKey,
    pub request_fingerprint: String,
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
        if self.title.len() > MAX_NAME_LENGTH {
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
        if self.idempotency_key.as_str().len() > MAX_IDEMPOTENCY_KEY_LENGTH {
            return Err(DomainError::Validation(ValidationError {
                code: "idempotency_key_too_long",
                message: "idempotency key exceeds 128 characters".to_owned(),
            }));
        }
        if self.request_fingerprint.trim().is_empty() {
            return Err(DomainError::Validation(ValidationError {
                code: "request_fingerprint_required",
                message: "request fingerprint must not be empty".to_owned(),
            }));
        }
        if self.request_fingerprint.len() > MAX_REQUEST_FINGERPRINT_LENGTH {
            return Err(DomainError::Validation(ValidationError {
                code: "request_fingerprint_too_long",
                message: "request fingerprint exceeds 128 characters".to_owned(),
            }));
        }
        if self
            .description
            .as_ref()
            .is_some_and(|description| description.len() > MAX_ISSUE_DESCRIPTION_LENGTH)
        {
            return Err(DomainError::Validation(ValidationError {
                code: "description_too_long",
                message: "issue description exceeds 4000 characters".to_owned(),
            }));
        }
        Ok(())
    }

    pub fn map_duplicate_conflict(&self) -> DomainError {
        DomainError::Conflict(ConflictError {
            code: "duplicate_issue_command",
            message: format!(
                "issue command with idempotency key '{}' already exists with a different payload",
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

#[cfg(test)]
mod tests {
    use super::*;

    fn base_create_issue_command() -> CreateIssueCommandV1 {
        CreateIssueCommandV1 {
            command_id: CommandId(Uuid::now_v7()),
            idempotency_key: IdempotencyKey::new("idem").expect("test fixture"),
            request_fingerprint: "v2:abc".to_owned(),
            issue_id: IssueId(Uuid::now_v7()),
            workspace_id: WorkspaceId(Uuid::now_v7()),
            project_id: ProjectId(Uuid::now_v7()),
            milestone_id: None,
            title: "title".to_owned(),
            description: Some("description".to_owned()),
            priority: IssuePriority::Medium,
            actor_id: Uuid::now_v7(),
        }
    }

    #[test]
    fn create_issue_rejects_oversized_description() {
        let mut command = base_create_issue_command();
        command.description = Some("x".repeat(MAX_ISSUE_DESCRIPTION_LENGTH + 1));
        let error = command
            .validate()
            .expect_err("oversized description must fail validation");
        assert!(matches!(error, DomainError::Validation(_)));
    }

    #[test]
    fn create_issue_rejects_oversized_request_fingerprint() {
        let mut command = base_create_issue_command();
        command.request_fingerprint = "x".repeat(MAX_REQUEST_FINGERPRINT_LENGTH + 1);
        let error = command
            .validate()
            .expect_err("oversized fingerprint must fail validation");
        assert!(matches!(error, DomainError::Validation(_)));
    }

    #[test]
    fn command_name_round_trips_through_parse() {
        for name in [
            CommandName::IssueCreateV1,
            CommandName::WorkspaceCreateV1,
            CommandName::ProjectCreateV1,
        ] {
            assert_eq!(CommandName::parse(name.as_str()), Some(name));
        }
    }

    #[test]
    fn unknown_command_name_returns_none() {
        assert_eq!(CommandName::parse("issue.create.v999"), None);
        assert_eq!(CommandName::parse(""), None);
    }

    #[test]
    fn command_enums_expose_typed_name() {
        let command = IssueCommandV1::CreateIssue(base_create_issue_command());
        assert_eq!(command.name(), CommandName::IssueCreateV1);
        assert_eq!(command.command_name(), "issue.create.v1");
    }
}
