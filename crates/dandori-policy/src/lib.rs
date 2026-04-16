//! Typed authorization policy primitives. Decisions are returned as an
//! explicit enum with a structured deny reason so callers can surface a
//! stable `authz_denied` envelope without inventing ad-hoc error strings.
//!
//! The default [`RoleMatrixPolicy`] implements a tight (Role × Action) matrix
//! and is the engine the app-service wires by default. Legacy capability
//! allowlisting is still available through [`CapabilityAllowList`] for
//! migration-era callers; it delegates to the same decision shape so mixed
//! deployments remain coherent.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable identifier for any entity referenced by a policy request. Kept
/// separately from `WorkspaceId`/`IssueId` in the domain crate so the policy
/// crate does not depend upwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntityId(pub Uuid);

impl EntityId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for EntityId {
    fn default() -> Self {
        Self::new()
    }
}

/// Roles recognized by the policy engine. Closed set on purpose — adding a
/// role requires a matrix update that tests will flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Owner,
    Member,
    Viewer,
    /// The worker actor identity. Only granted permissions necessary to
    /// process outbox / partition-lease work — never direct CRUD on
    /// business aggregates.
    Worker,
}

/// Business actions that can be authorized. Non-exhaustive on purpose: new
/// actions added later should not force a recompile of downstream crates
/// that only match the variants they care about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Action {
    WorkspaceRead,
    IssueCreate,
    IssueRead,
    IssueUpdate,
    ProjectRead,
}

/// Typed resource a decision is about. Always includes workspace so the
/// engine can enforce tenant boundaries without re-parsing context from
/// the action string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Resource {
    Workspace {
        workspace_id: Uuid,
    },
    Project {
        workspace_id: Uuid,
        project_id: Uuid,
    },
    Issue {
        workspace_id: Uuid,
        issue_id: Uuid,
    },
}

impl Resource {
    #[must_use]
    pub fn workspace_id(&self) -> Uuid {
        match self {
            Resource::Workspace { workspace_id }
            | Resource::Project { workspace_id, .. }
            | Resource::Issue { workspace_id, .. } => *workspace_id,
        }
    }
}

/// Caller identity. `roles` is a set so deployments can grant multiple
/// roles simultaneously (e.g., Owner + Worker for system principals).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subject {
    pub actor_id: Uuid,
    pub workspace_id: Uuid,
    pub roles: BTreeSet<Role>,
}

impl Subject {
    #[must_use]
    pub fn new(actor_id: Uuid, workspace_id: Uuid, roles: impl IntoIterator<Item = Role>) -> Self {
        Self {
            actor_id,
            workspace_id,
            roles: roles.into_iter().collect(),
        }
    }
}

/// Structured deny reason. Converts cleanly into an `authz_denied` error
/// envelope at the transport layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DenyReason {
    MissingRole,
    CrossTenant,
    MissingSubject,
}

impl DenyReason {
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            DenyReason::MissingRole => "authz_missing_role",
            DenyReason::CrossTenant => "authz_cross_tenant",
            DenyReason::MissingSubject => "authz_missing_subject",
        }
    }

    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            DenyReason::MissingRole => "subject does not have a role granting this action",
            DenyReason::CrossTenant => "subject's workspace does not match the resource workspace",
            DenyReason::MissingSubject => "no subject was provided for this action",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PolicyDecision {
    Allow,
    Deny { reason: DenyReason },
}

impl PolicyDecision {
    #[must_use]
    pub fn is_allowed(&self) -> bool {
        matches!(self, PolicyDecision::Allow)
    }

    /// Returns `Ok(())` for Allow or `Err(AuthzError)` for Deny.
    pub fn into_result(self) -> Result<(), AuthzError> {
        match self {
            PolicyDecision::Allow => Ok(()),
            PolicyDecision::Deny { reason } => Err(AuthzError(reason)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{}: {}", .0.code(), .0.message())]
pub struct AuthzError(pub DenyReason);

/// The core policy trait. Implementations must be cheap to evaluate (no
/// I/O) so decisions can be applied inline on the hot path.
pub trait PolicyEngine: Send + Sync + std::fmt::Debug {
    fn evaluate(
        &self,
        subject: Option<&Subject>,
        action: Action,
        resource: Resource,
    ) -> PolicyDecision;
}

/// Default matrix-based policy: each action has a set of roles that grant
/// it. Deny-by-default. Tenant boundary is enforced before the matrix
/// lookup so a cross-workspace subject is always denied even if the role
/// would normally allow the action.
#[derive(Debug, Clone)]
pub struct RoleMatrixPolicy {
    matrix: BTreeMap<Action, BTreeSet<Role>>,
}

impl RoleMatrixPolicy {
    #[must_use]
    pub fn standard() -> Self {
        let mut matrix: BTreeMap<Action, BTreeSet<Role>> = BTreeMap::new();
        matrix.insert(
            Action::WorkspaceRead,
            [Role::Owner, Role::Member, Role::Viewer]
                .into_iter()
                .collect(),
        );
        matrix.insert(
            Action::IssueCreate,
            [Role::Owner, Role::Member].into_iter().collect(),
        );
        matrix.insert(
            Action::IssueRead,
            [Role::Owner, Role::Member, Role::Viewer]
                .into_iter()
                .collect(),
        );
        matrix.insert(
            Action::IssueUpdate,
            [Role::Owner, Role::Member].into_iter().collect(),
        );
        matrix.insert(
            Action::ProjectRead,
            [Role::Owner, Role::Member, Role::Viewer]
                .into_iter()
                .collect(),
        );
        Self { matrix }
    }

    #[must_use]
    pub fn with_override(mut self, action: Action, roles: impl IntoIterator<Item = Role>) -> Self {
        self.matrix.insert(action, roles.into_iter().collect());
        self
    }
}

impl Default for RoleMatrixPolicy {
    fn default() -> Self {
        Self::standard()
    }
}

impl PolicyEngine for RoleMatrixPolicy {
    fn evaluate(
        &self,
        subject: Option<&Subject>,
        action: Action,
        resource: Resource,
    ) -> PolicyDecision {
        let Some(subject) = subject else {
            return PolicyDecision::Deny {
                reason: DenyReason::MissingSubject,
            };
        };
        if subject.workspace_id != resource.workspace_id() {
            return PolicyDecision::Deny {
                reason: DenyReason::CrossTenant,
            };
        }
        let Some(allowed_roles) = self.matrix.get(&action) else {
            return PolicyDecision::Deny {
                reason: DenyReason::MissingRole,
            };
        };
        if subject.roles.iter().any(|r| allowed_roles.contains(r)) {
            PolicyDecision::Allow
        } else {
            PolicyDecision::Deny {
                reason: DenyReason::MissingRole,
            }
        }
    }
}

/// Legacy capability allowlist, preserved for callers in the middle of
/// migrating to the typed matrix. Resolves to the same `PolicyDecision`
/// shape so callers can swap implementations without changing handling.
#[derive(Debug, Clone, Default)]
pub struct CapabilityAllowList {
    allowed: BTreeSet<String>,
}

impl CapabilityAllowList {
    #[must_use]
    pub fn new<I>(allowed: I) -> Self
    where
        I: IntoIterator<Item = String>,
    {
        Self {
            allowed: allowed.into_iter().collect(),
        }
    }

    /// Map a typed `Action` to its legacy capability string.
    #[must_use]
    fn capability_for(action: Action) -> &'static str {
        match action {
            Action::WorkspaceRead => "workspace.read",
            Action::IssueCreate => "issue.create",
            Action::IssueRead => "issue.read",
            Action::IssueUpdate => "issue.update",
            Action::ProjectRead => "project.read",
        }
    }
}

impl PolicyEngine for CapabilityAllowList {
    fn evaluate(
        &self,
        subject: Option<&Subject>,
        action: Action,
        resource: Resource,
    ) -> PolicyDecision {
        let Some(subject) = subject else {
            return PolicyDecision::Deny {
                reason: DenyReason::MissingSubject,
            };
        };
        if subject.workspace_id != resource.workspace_id() {
            return PolicyDecision::Deny {
                reason: DenyReason::CrossTenant,
            };
        }
        if self.allowed.contains(Self::capability_for(action)) {
            PolicyDecision::Allow
        } else {
            PolicyDecision::Deny {
                reason: DenyReason::MissingRole,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws() -> Uuid {
        Uuid::now_v7()
    }

    #[test]
    fn owner_can_create_issue_in_own_workspace() {
        let policy = RoleMatrixPolicy::standard();
        let workspace_id = ws();
        let subject = Subject::new(Uuid::now_v7(), workspace_id, [Role::Owner]);
        let decision = policy.evaluate(
            Some(&subject),
            Action::IssueCreate,
            Resource::Workspace { workspace_id },
        );
        assert!(decision.is_allowed());
    }

    #[test]
    fn viewer_cannot_create_issue() {
        let policy = RoleMatrixPolicy::standard();
        let workspace_id = ws();
        let subject = Subject::new(Uuid::now_v7(), workspace_id, [Role::Viewer]);
        let decision = policy.evaluate(
            Some(&subject),
            Action::IssueCreate,
            Resource::Workspace { workspace_id },
        );
        assert_eq!(
            decision,
            PolicyDecision::Deny {
                reason: DenyReason::MissingRole
            }
        );
    }

    #[test]
    fn cross_tenant_is_always_denied() {
        let policy = RoleMatrixPolicy::standard();
        let subject_ws = ws();
        let resource_ws = ws();
        let subject = Subject::new(Uuid::now_v7(), subject_ws, [Role::Owner]);
        let decision = policy.evaluate(
            Some(&subject),
            Action::IssueRead,
            Resource::Workspace {
                workspace_id: resource_ws,
            },
        );
        assert_eq!(
            decision,
            PolicyDecision::Deny {
                reason: DenyReason::CrossTenant
            }
        );
    }

    #[test]
    fn missing_subject_denies() {
        let policy = RoleMatrixPolicy::standard();
        let decision = policy.evaluate(
            None,
            Action::IssueRead,
            Resource::Workspace { workspace_id: ws() },
        );
        assert_eq!(
            decision,
            PolicyDecision::Deny {
                reason: DenyReason::MissingSubject
            }
        );
    }

    #[test]
    fn worker_role_is_not_granted_crud_by_default() {
        let policy = RoleMatrixPolicy::standard();
        let workspace_id = ws();
        let subject = Subject::new(Uuid::now_v7(), workspace_id, [Role::Worker]);
        for action in [
            Action::IssueCreate,
            Action::IssueRead,
            Action::IssueUpdate,
            Action::ProjectRead,
        ] {
            let decision =
                policy.evaluate(Some(&subject), action, Resource::Workspace { workspace_id });
            assert!(
                !decision.is_allowed(),
                "worker role should not be granted {action:?} by default"
            );
        }
    }

    #[test]
    fn capability_allowlist_respects_tenant_boundary() {
        let engine = CapabilityAllowList::new(["issue.create".to_owned()]);
        let workspace_id = ws();
        let subject = Subject::new(Uuid::now_v7(), workspace_id, [Role::Owner]);
        let decision = engine.evaluate(
            Some(&subject),
            Action::IssueCreate,
            Resource::Workspace { workspace_id: ws() },
        );
        assert_eq!(
            decision,
            PolicyDecision::Deny {
                reason: DenyReason::CrossTenant
            }
        );
    }

    #[test]
    fn into_result_converts_decision_to_error() {
        assert!(PolicyDecision::Allow.into_result().is_ok());
        let err = PolicyDecision::Deny {
            reason: DenyReason::MissingRole,
        }
        .into_result()
        .expect_err("deny produces error");
        assert_eq!(err.0, DenyReason::MissingRole);
    }
}
