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
pub struct SyncEnvelope {
    pub workspace_id: EntityId,
    pub external_id: String,
    pub etag: String,
}

impl SyncEnvelope {
    pub fn validate(&self) -> Result<(), CrateError> {
        if self.external_id.trim().is_empty() {
            return Err(CrateError::Validation(
                "external_id must not be empty".to_owned(),
            ));
        }
        if self.etag.trim().is_empty() {
            return Err(CrateError::Validation("etag must not be empty".to_owned()));
        }
        Ok(())
    }
}

pub trait SyncGateway {
    fn upsert(&self, envelope: &SyncEnvelope) -> Result<(), CrateError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoopSyncGateway;

impl SyncGateway for NoopSyncGateway {
    fn upsert(&self, envelope: &SyncEnvelope) -> Result<(), CrateError> {
        envelope.validate()
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
    fn noop_gateway_enforces_sync_envelope_invariants() {
        let gateway = NoopSyncGateway;
        let bad = SyncEnvelope {
            workspace_id: EntityId::new(),
            external_id: String::new(),
            etag: "etag".to_owned(),
        };

        let error = gateway
            .upsert(&bad)
            .expect_err("invalid envelope must fail");
        assert!(matches!(error, CrateError::Validation(_)));
    }
}
