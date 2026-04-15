//! Dandori core domain contracts for phase 1.
//!
//! Versioned command/event boundaries:
//! - [`IssueCommandV1`]
//! - [`IssueEventV1`]

mod auth;
mod command;
mod error;
mod event;
mod ids;
mod model;

pub use auth::AuthContext;
pub use command::{CreateIssueCommandV1, IssueCommandV1};
pub use error::{
    AuthzError, ConflictError, DomainError, InfrastructureError, PreconditionError,
    TenantBoundaryError, ValidationError,
};
pub use event::{IssueCreatedEventV1, IssueEventV1};
pub use ids::{
    ActivityId, CommandId, IdempotencyKey, IssueId, MilestoneId, OutboxId, ProjectId, WorkspaceId,
};
pub use model::{
    Issue, IssuePriority, IssueStateCategory, Milestone, Project, WorkflowVersionRef, Workspace,
};
