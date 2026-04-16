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
pub struct Transition {
    pub from: String,
    pub to: String,
    pub action: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkflowSpec {
    pub name: String,
    pub initial_state: String,
    pub states: Vec<String>,
    pub transitions: Vec<Transition>,
}

impl WorkflowSpec {
    pub fn validate(&self) -> Result<(), CrateError> {
        if self.name.trim().is_empty() {
            return Err(CrateError::Validation(
                "workflow name must not be empty".to_owned(),
            ));
        }
        if self.states.is_empty() {
            return Err(CrateError::Validation(
                "workflow must contain at least one state".to_owned(),
            ));
        }

        let state_set: BTreeSet<&str> = self.states.iter().map(String::as_str).collect();
        if !state_set.contains(self.initial_state.as_str()) {
            return Err(CrateError::Validation(
                "initial state must be part of the declared state set".to_owned(),
            ));
        }

        for transition in &self.transitions {
            if transition.action.trim().is_empty() {
                return Err(CrateError::Validation(
                    "transition action must not be empty".to_owned(),
                ));
            }
            if !state_set.contains(transition.from.as_str())
                || !state_set.contains(transition.to.as_str())
            {
                return Err(CrateError::Validation(
                    "transition endpoints must reference known states".to_owned(),
                ));
            }
        }

        Ok(())
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
    fn workflow_spec_requires_initial_state_membership() {
        let spec = WorkflowSpec {
            name: "issue-lifecycle".to_owned(),
            initial_state: "active".to_owned(),
            states: vec!["open".to_owned(), "done".to_owned()],
            transitions: vec![],
        };

        let error = spec
            .validate()
            .expect_err("invalid initial state must fail validation");
        assert!(matches!(error, CrateError::Validation(_)));
    }
}
