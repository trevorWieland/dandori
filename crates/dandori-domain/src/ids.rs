use std::fmt::{Display, Formatter};

use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use crate::DomainError;

macro_rules! uuid_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl Display for $name {
            fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self(value)
            }
        }

        impl From<$name> for Uuid {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

uuid_id!(WorkspaceId);
uuid_id!(ProjectId);
uuid_id!(MilestoneId);
uuid_id!(IssueId);
uuid_id!(ActivityId);
uuid_id!(OutboxId);
uuid_id!(CommandId);

/// Caller-supplied idempotency key. Validated at construction time: the
/// inner string is private and only constructible via [`IdempotencyKey::new`],
/// which enforces length 1..=128 and a printable-ASCII-only charset (no
/// control characters). This mirrors the database CHECK constraint and
/// guarantees invalid keys cannot sneak past the transport boundary.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    pub const MIN_LEN: usize = 1;
    pub const MAX_LEN: usize = 128;

    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let raw = value.into();
        Self::validate(&raw)?;
        Ok(Self(raw))
    }

    fn validate(value: &str) -> Result<(), DomainError> {
        if value.len() < Self::MIN_LEN || value.len() > Self::MAX_LEN {
            return Err(DomainError::validation(
                "idempotency_key_length",
                format!(
                    "idempotency_key length must be between {min} and {max} bytes (got {len})",
                    min = Self::MIN_LEN,
                    max = Self::MAX_LEN,
                    len = value.len(),
                ),
            ));
        }
        if !value
            .chars()
            .all(|c| c.is_ascii_graphic() || c == ' ' || c == '-' || c == '_')
        {
            return Err(DomainError::validation(
                "idempotency_key_charset",
                "idempotency_key must only contain printable ASCII (graphic, space, dash, underscore)",
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl Display for IdempotencyKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for IdempotencyKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::new(raw).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod idempotency_key_tests {
    use super::*;

    #[test]
    fn accepts_reasonable_value() {
        let key = IdempotencyKey::new("idem-abc_123").expect("valid");
        assert_eq!(key.as_str(), "idem-abc_123");
    }

    #[test]
    fn rejects_empty() {
        assert!(IdempotencyKey::new("").is_err());
    }

    #[test]
    fn rejects_too_long() {
        let long = "a".repeat(IdempotencyKey::MAX_LEN + 1);
        assert!(IdempotencyKey::new(long).is_err());
    }

    #[test]
    fn rejects_control_characters() {
        assert!(IdempotencyKey::new("bad\nvalue").is_err());
        assert!(IdempotencyKey::new("bad\tvalue").is_err());
    }

    #[test]
    fn deserialize_validates() {
        let good: IdempotencyKey =
            serde_json::from_str("\"idem-1\"").expect("valid json idempotency key");
        assert_eq!(good.as_str(), "idem-1");
        let bad: Result<IdempotencyKey, _> = serde_json::from_str("\"\"");
        assert!(bad.is_err());
    }
}
