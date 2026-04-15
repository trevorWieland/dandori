use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{DomainError, PreconditionError, WorkspaceId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthContext {
    pub workspace_id: WorkspaceId,
    pub actor_id: Uuid,
}

impl AuthContext {
    pub fn enforce_workspace(&self, workspace_id: WorkspaceId) -> Result<(), DomainError> {
        if self.workspace_id != workspace_id {
            return Err(DomainError::Precondition(PreconditionError {
                code: "workspace_mismatch",
                message: "workspace in command does not match authenticated workspace".to_owned(),
            }));
        }
        Ok(())
    }
}
