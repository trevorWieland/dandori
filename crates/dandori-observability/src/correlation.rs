//! Correlation identifiers that flow through REST, MCP, app-service, store,
//! and outbox boundaries so operators can tie a user-visible failure back to
//! private log lines and outbox events without leaking internal detail.

use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Header used to propagate a correlation id over HTTP. The REST transport
/// accepts incoming values and generates one if absent. The same constant is
/// exposed to downstream crates so propagation does not drift.
pub const CORRELATION_ID_HEADER: &str = "x-correlation-id";

/// Canonical correlation identifier. Internally a v7 UUID (time-ordered) so
/// logs stay roughly sorted by creation time even if the observation order
/// differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CorrelationId(Uuid);

impl CorrelationId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    #[must_use]
    pub fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    /// Parse an incoming correlation id from a header or payload. Unknown
    /// or malformed values silently fall back to a freshly generated id;
    /// the caller should log when this happens but we never reject a
    /// request purely on a bad correlation id.
    #[must_use]
    pub fn parse_or_new(raw: Option<&str>) -> Self {
        raw.and_then(|s| Uuid::parse_str(s.trim()).ok())
            .map_or_else(Self::new, Self)
    }

    #[must_use]
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for CorrelationId {
    fn default() -> Self {
        Self::new()
    }
}

impl Display for CorrelationId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for CorrelationId {
    fn from(value: Uuid) -> Self {
        Self(value)
    }
}

impl From<CorrelationId> for Uuid {
    fn from(value: CorrelationId) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_or_new_accepts_valid_uuid() {
        let id = Uuid::now_v7();
        let parsed = CorrelationId::parse_or_new(Some(id.to_string().as_str()));
        assert_eq!(parsed.as_uuid(), id);
    }

    #[test]
    fn parse_or_new_generates_for_missing() {
        let a = CorrelationId::parse_or_new(None);
        let b = CorrelationId::parse_or_new(None);
        assert_ne!(a.as_uuid(), b.as_uuid());
    }

    #[test]
    fn parse_or_new_generates_for_invalid() {
        let parsed = CorrelationId::parse_or_new(Some("not-a-uuid"));
        assert_ne!(parsed.as_uuid(), Uuid::nil());
    }
}
