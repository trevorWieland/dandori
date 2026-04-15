use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{IssueId, IssuePriority, MilestoneId, ProjectId, WorkspaceId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum IssueEventV1 {
    IssueCreated(IssueCreatedEventV1),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum WorkspaceEventV1 {
    WorkspaceCreated(WorkspaceCreatedEventV1),
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
