use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct EntityId(pub uuid::Uuid);

impl EntityId {
    #[must_use]
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7())
    }
}

impl Default for EntityId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PolicyRequest {
    pub workspace_id: EntityId,
    pub actor_id: uuid::Uuid,
    pub capability: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyDecision {
    Allow,
    Deny,
}

pub trait PolicyEngine {
    fn evaluate(&self, request: &PolicyRequest) -> Result<PolicyDecision, CrateError>;
}

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
}

impl PolicyEngine for CapabilityAllowList {
    fn evaluate(&self, request: &PolicyRequest) -> Result<PolicyDecision, CrateError> {
        if request.capability.trim().is_empty() {
            return Err(CrateError::Validation(
                "capability must not be empty".to_owned(),
            ));
        }

        if self.allowed.contains(request.capability.as_str()) {
            Ok(PolicyDecision::Allow)
        } else {
            Ok(PolicyDecision::Deny)
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CrateError {
    #[error("validation error: {0}")]
    Validation(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_list_engine_applies_capability_policy() {
        let engine = CapabilityAllowList::new(["issue.create".to_owned()]);

        let allowed = PolicyRequest {
            workspace_id: EntityId::new(),
            actor_id: uuid::Uuid::now_v7(),
            capability: "issue.create".to_owned(),
        };
        let denied = PolicyRequest {
            workspace_id: EntityId::new(),
            actor_id: uuid::Uuid::now_v7(),
            capability: "issue.delete".to_owned(),
        };

        assert_eq!(
            engine.evaluate(&allowed).expect("allow decision"),
            PolicyDecision::Allow
        );
        assert_eq!(
            engine.evaluate(&denied).expect("deny decision"),
            PolicyDecision::Deny
        );
    }
}
