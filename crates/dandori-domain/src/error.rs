use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("validation error ({code}): {message}")]
pub struct ValidationError {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("precondition failed ({code}): {message}")]
pub struct PreconditionError {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("conflict ({code}): {message}")]
pub struct ConflictError {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("authorization denied ({code}): {message}")]
pub struct AuthzError {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("tenant boundary violation ({code}): {message}")]
pub struct TenantBoundaryError {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("infrastructure failure ({code}): {message}")]
pub struct InfrastructureError {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DomainError {
    #[error(transparent)]
    Validation(ValidationError),
    #[error(transparent)]
    Precondition(PreconditionError),
    #[error(transparent)]
    Conflict(ConflictError),
    #[error(transparent)]
    Authz(AuthzError),
    #[error(transparent)]
    TenantBoundary(TenantBoundaryError),
    #[error(transparent)]
    Infrastructure(InfrastructureError),
}
