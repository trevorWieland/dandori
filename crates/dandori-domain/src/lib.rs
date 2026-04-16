//! Dandori core domain contracts for phase 1.
//!
//! Versioned command/event boundaries:
//! - [`IssueCommandV1`]
//! - [`IssueEventV1`]
//! - [`WorkspaceCommandV1`]
//! - [`WorkspaceEventV1`]
//! - [`ProjectCommandV1`]
//! - [`ProjectEventV1`]
//!
//! Typed identifier contracts:
//! - [`CommandName`] — stable command identifier enum
//! - [`EventType`] — stable event identifier enum

mod auth;
mod command;
mod error;
mod event;
mod ids;
mod model;

pub use auth::AuthContext;
pub use command::{
    CommandName, CreateIssueCommandV1, CreateProjectCommandV1, CreateWorkspaceCommandV1,
    IssueCommandV1, ProjectCommandV1, WorkspaceCommandV1,
};
pub use error::{
    AuthzError, ConflictError, DomainError, InfrastructureError, PreconditionError,
    TenantBoundaryError, ValidationError,
};
pub use event::{
    EventType, IssueCreatedEventV1, IssueEventV1, ProjectCreatedEventV1, ProjectEventV1,
    WorkspaceCreatedEventV1, WorkspaceEventV1,
};
pub use ids::{
    ActivityId, CommandId, IdempotencyKey, IssueId, MilestoneId, OutboxId, ProjectId, WorkspaceId,
};
pub use model::{
    Issue, IssuePriority, IssueStateCategory, Milestone, Project, WorkflowVersionRef, Workspace,
};
