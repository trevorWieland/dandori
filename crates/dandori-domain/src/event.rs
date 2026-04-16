use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{IssueId, IssuePriority, MilestoneId, ProjectId, WorkspaceId};

/// Stable, versioned event identifier. Anywhere an event name crosses a
/// persistence, wire, or routing boundary it must flow through
/// [`EventType::as_str`] / [`EventType::parse`] so the compiler forbids
/// drift between the store, worker, and transport layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventType {
    IssueCreatedV1,
    ProjectCreatedV1,
    WorkspaceCreatedV1,
}

impl EventType {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IssueCreatedV1 => "issue.created.v1",
            Self::ProjectCreatedV1 => "project.created.v1",
            Self::WorkspaceCreatedV1 => "workspace.created.v1",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "issue.created.v1" => Some(Self::IssueCreatedV1),
            "project.created.v1" => Some(Self::ProjectCreatedV1),
            "workspace.created.v1" => Some(Self::WorkspaceCreatedV1),
            _ => None,
        }
    }
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum IssueEventV1 {
    IssueCreated(IssueCreatedEventV1),
}

impl IssueEventV1 {
    #[must_use]
    pub fn event_type(&self) -> EventType {
        match self {
            Self::IssueCreated(_) => EventType::IssueCreatedV1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum WorkspaceEventV1 {
    WorkspaceCreated(WorkspaceCreatedEventV1),
}

impl WorkspaceEventV1 {
    #[must_use]
    pub fn event_type(&self) -> EventType {
        match self {
            Self::WorkspaceCreated(_) => EventType::WorkspaceCreatedV1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceCreatedEventV1 {
    pub event_id: Uuid,
    pub workspace_id: WorkspaceId,
    pub actor_id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ProjectEventV1 {
    ProjectCreated(ProjectCreatedEventV1),
}

impl ProjectEventV1 {
    #[must_use]
    pub fn event_type(&self) -> EventType {
        match self {
            Self::ProjectCreated(_) => EventType::ProjectCreatedV1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectCreatedEventV1 {
    pub event_id: Uuid,
    pub workspace_id: WorkspaceId,
    pub project_id: ProjectId,
    pub workflow_version_id: Uuid,
    pub actor_id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueCreatedEventV1 {
    pub event_id: Uuid,
    pub issue_id: IssueId,
    pub workspace_id: WorkspaceId,
    pub project_id: ProjectId,
    pub milestone_id: Option<MilestoneId>,
    pub actor_id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub title: String,
    pub description: Option<String>,
    pub priority: IssuePriority,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_type_round_trips_through_parse() {
        for event_type in [
            EventType::IssueCreatedV1,
            EventType::ProjectCreatedV1,
            EventType::WorkspaceCreatedV1,
        ] {
            assert_eq!(EventType::parse(event_type.as_str()), Some(event_type));
        }
    }

    #[test]
    fn unknown_event_type_returns_none() {
        assert_eq!(EventType::parse("issue.created.v999"), None);
        assert_eq!(EventType::parse(""), None);
    }
}
