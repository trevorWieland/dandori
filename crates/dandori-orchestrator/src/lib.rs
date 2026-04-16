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
pub struct OrchestrationRequest {
    pub workspace_id: EntityId,
    pub correlation_id: uuid::Uuid,
    pub steps: Vec<String>,
}

impl OrchestrationRequest {
    pub fn validate(&self) -> Result<(), CrateError> {
        if self.steps.is_empty() {
            return Err(CrateError::Validation(
                "orchestration requires at least one step".to_owned(),
            ));
        }
        if self.steps.iter().any(|step| step.trim().is_empty()) {
            return Err(CrateError::Validation(
                "orchestration steps must be non-empty".to_owned(),
            ));
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationStatus {
    Accepted,
    Rejected,
}

pub trait Orchestrator {
    fn submit(&self, request: &OrchestrationRequest) -> Result<OrchestrationStatus, CrateError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ValidationOnlyOrchestrator;

impl Orchestrator for ValidationOnlyOrchestrator {
    fn submit(&self, request: &OrchestrationRequest) -> Result<OrchestrationStatus, CrateError> {
        request.validate()?;
        Ok(OrchestrationStatus::Accepted)
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
    fn validation_only_orchestrator_rejects_empty_steps() {
        let orchestrator = ValidationOnlyOrchestrator;
        let request = OrchestrationRequest {
            workspace_id: EntityId::new(),
            correlation_id: uuid::Uuid::now_v7(),
            steps: vec![],
        };

        let error = orchestrator
            .submit(&request)
            .expect_err("empty steps should fail validation");
        assert!(matches!(error, CrateError::Validation(_)));
    }
}
