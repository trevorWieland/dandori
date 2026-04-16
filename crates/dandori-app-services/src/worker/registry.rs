//! Versioned outbox event dispatcher.
//!
//! The worker's previous implementation hard-coded a single `match` on the
//! event type enum. Adding a new event required trait growth plus a code
//! change in the critical publish path. The registry here decouples the two:
//! handlers register themselves keyed on `(event_type, schema_version)`,
//! unknown events fall through to a dead-letter path with a metric, and the
//! publisher trait stays stable.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use dandori_store::OutboxMessage;

use super::publish::{OutboxPublisher, PublishError, PublishErrorKind};

/// Single outbox handler. Implementations are responsible for deserializing
/// the payload to their expected shape and dispatching to whatever external
/// surface consumes the event.
#[async_trait]
pub trait OutboxHandler: Send + Sync + std::fmt::Debug {
    fn event_type(&self) -> &'static str;
    fn schema_version(&self) -> u32;
    async fn handle(
        &self,
        publisher: &dyn OutboxPublisher,
        message: &OutboxMessage,
    ) -> Result<(), PublishError>;
}

/// A single registered handler keyed by its schema version.
type VersionedHandler = (u32, Arc<dyn OutboxHandler>);

/// Registry keyed on (event_type, schema_version). Handler lookup first
/// tries an exact match; if none is registered but a handler exists for
/// the same event type at a lower version, that one is returned so older
/// schema_versions remain routable during gradual rollouts.
#[derive(Default, Debug, Clone)]
pub struct OutboxRegistry {
    by_key: HashMap<(String, u32), Arc<dyn OutboxHandler>>,
    by_type: HashMap<String, Vec<VersionedHandler>>,
}

impl OutboxRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, handler: Arc<dyn OutboxHandler>) -> &mut Self {
        let key = (handler.event_type().to_owned(), handler.schema_version());
        self.by_key.insert(key.clone(), Arc::clone(&handler));
        let versions = self
            .by_type
            .entry(handler.event_type().to_owned())
            .or_default();
        versions.push((handler.schema_version(), handler));
        versions.sort_by_key(|(version, _)| *version);
        self
    }

    #[must_use]
    pub fn resolve(&self, event_type: &str, schema_version: u32) -> Option<Arc<dyn OutboxHandler>> {
        if let Some(exact) = self.by_key.get(&(event_type.to_owned(), schema_version)) {
            return Some(Arc::clone(exact));
        }
        let versions = self.by_type.get(event_type)?;
        versions
            .iter()
            .rev()
            .find(|(version, _)| *version <= schema_version)
            .map(|(_, handler)| Arc::clone(handler))
    }

    pub async fn dispatch(
        &self,
        publisher: &dyn OutboxPublisher,
        message: &OutboxMessage,
    ) -> Result<(), PublishError> {
        let version = resolve_schema_version(message).unwrap_or(1);
        match self.resolve(message.event_type.as_str(), version) {
            Some(handler) => handler.handle(publisher, message).await,
            None => {
                dandori_observability::metrics::increment_counter(
                    dandori_observability::metrics::names::WORKER_OUTBOX_DEAD_LETTER,
                    1,
                );
                Err(PublishError {
                    kind: PublishErrorKind::Unsupported,
                    message: format!(
                        "no handler registered for event_type={} schema_version={version}",
                        message.event_type
                    ),
                })
            }
        }
    }
}

/// Extract `schema_version` from the payload. The contract expects payloads
/// to include an explicit integer field so upgrades can be routed safely;
/// if absent we assume v1 for backward compatibility.
fn resolve_schema_version(message: &OutboxMessage) -> Option<u32> {
    message
        .payload
        .get("schema_version")
        .and_then(|v| v.as_u64())
        .and_then(|v| u32::try_from(v).ok())
}

pub(crate) mod handlers {
    //! Concrete handler implementations. Each event gets its own module so
    //! adding a new event type is an additive change (new module + one
    //! `register` call in the worker wiring) rather than a trait growth.

    use async_trait::async_trait;
    use dandori_domain::{EventType, IssueCreatedEventV1};
    use dandori_store::OutboxMessage;

    use super::super::publish::{OutboxPublisher, PublishError, PublishErrorKind};
    use super::OutboxHandler;

    #[derive(Debug, Default)]
    pub(crate) struct IssueCreatedV1Handler;

    #[async_trait]
    impl OutboxHandler for IssueCreatedV1Handler {
        fn event_type(&self) -> &'static str {
            EventType::IssueCreatedV1.as_str()
        }

        fn schema_version(&self) -> u32 {
            1
        }

        async fn handle(
            &self,
            publisher: &dyn OutboxPublisher,
            message: &OutboxMessage,
        ) -> Result<(), PublishError> {
            let event: IssueCreatedEventV1 = serde_json::from_value(message.payload.clone())
                .map_err(|error| PublishError {
                    kind: PublishErrorKind::Serialization,
                    message: format!("failed to deserialize issue.created payload: {error}"),
                })?;
            publisher.publish_issue_created(message, &event).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::Utc;
    use dandori_store::OutboxMessage;
    use serde_json::json;
    use uuid::Uuid;

    #[derive(Debug, Default)]
    struct StubPublisher;

    #[async_trait]
    impl OutboxPublisher for StubPublisher {
        async fn publish_issue_created(
            &self,
            _message: &OutboxMessage,
            _event: &dandori_domain::IssueCreatedEventV1,
        ) -> Result<(), PublishError> {
            Ok(())
        }
    }

    fn message(event_type: &str, schema_version: Option<u32>) -> OutboxMessage {
        let mut payload = json!({"foo": "bar"});
        if let Some(version) = schema_version {
            payload["schema_version"] = json!(version);
        }
        OutboxMessage {
            id: Uuid::now_v7(),
            workspace_id: Uuid::now_v7(),
            event_id: Uuid::now_v7(),
            event_type: event_type.to_owned(),
            aggregate_type: "issue".to_owned(),
            aggregate_id: Uuid::now_v7(),
            correlation_id: Uuid::now_v7(),
            payload,
            attempts: 0,
            lease_token: Uuid::now_v7(),
            lease_owner: Uuid::now_v7(),
            leased_until: Utc::now(),
        }
    }

    #[derive(Debug)]
    struct RecordingHandler {
        event_type: &'static str,
        version: u32,
    }

    #[async_trait]
    impl OutboxHandler for RecordingHandler {
        fn event_type(&self) -> &'static str {
            self.event_type
        }
        fn schema_version(&self) -> u32 {
            self.version
        }
        async fn handle(
            &self,
            _publisher: &dyn OutboxPublisher,
            _message: &OutboxMessage,
        ) -> Result<(), PublishError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn unknown_event_type_is_dead_lettered() {
        let registry = OutboxRegistry::new();
        let publisher = StubPublisher;
        let msg = message("totally.unknown", Some(1));
        let err = registry
            .dispatch(&publisher, &msg)
            .await
            .expect_err("must not route");
        assert_eq!(err.kind, PublishErrorKind::Unsupported);
    }

    #[tokio::test]
    async fn resolves_latest_compatible_version() {
        let mut registry = OutboxRegistry::new();
        registry.register(Arc::new(RecordingHandler {
            event_type: "issue.created",
            version: 1,
        }));
        registry.register(Arc::new(RecordingHandler {
            event_type: "issue.created",
            version: 3,
        }));
        let resolved = registry
            .resolve("issue.created", 2)
            .expect("fallback to v1");
        assert_eq!(resolved.schema_version(), 1);
        let exact = registry
            .resolve("issue.created", 3)
            .expect("exact v3 match");
        assert_eq!(exact.schema_version(), 3);
        assert!(registry.resolve("issue.created", 0).is_none());
    }
}
