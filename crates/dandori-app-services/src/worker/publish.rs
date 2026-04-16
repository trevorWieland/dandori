use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::time::Duration as StdDuration;

use async_trait::async_trait;
use chrono::{Duration, Utc};
use dandori_domain::{EventType, IssueCreatedEventV1};
use dandori_store::OutboxMessage;
use thiserror::Error;

use super::config::OutboxWorkerConfig;

/// Classification of a publish failure. Mirrors
/// [`dandori_store::OutboxFailureClassification`](dandori_store::OutboxFailureClassification)
/// but preserves the finer-grained kind for logs and retry decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishErrorKind {
    /// Retriable — network, 5xx, breaker open.
    Transient,
    /// Non-retriable client-side — 4xx.
    Permanent,
    /// Unknown event type — cannot be routed.
    Unsupported,
    /// Payload could not be deserialised to the expected event.
    Serialization,
}

impl PublishErrorKind {
    /// Whether the kind should burn retry budget or dead-letter immediately.
    #[must_use]
    pub const fn is_transient(self) -> bool {
        matches!(self, Self::Transient)
    }
}

#[derive(Debug, Error)]
#[error("{kind:?}: {message}")]
pub struct PublishError {
    pub kind: PublishErrorKind,
    pub message: String,
}

#[async_trait]
pub trait OutboxPublisher: Send + Sync + std::fmt::Debug {
    async fn publish_issue_created(
        &self,
        message: &OutboxMessage,
        event: &IssueCreatedEventV1,
    ) -> Result<(), PublishError>;
}

/// No-op publisher intended for local dev / explicit opt-in only. The worker
/// startup policy refuses to wire this in production (see
/// `with_default_publisher`).
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct NoopOutboxPublisher;

#[async_trait]
impl OutboxPublisher for NoopOutboxPublisher {
    async fn publish_issue_created(
        &self,
        _message: &OutboxMessage,
        _event: &IssueCreatedEventV1,
    ) -> Result<(), PublishError> {
        Ok(())
    }
}

/// HTTP publisher with explicit timeouts, bounded pool, and an in-process
/// circuit breaker. The breaker opens after
/// `circuit_failure_threshold` consecutive transient failures and stays
/// open for `circuit_cooldown_seconds`, returning `Transient` without
/// hitting the network so upstream noise does not multiply.
#[derive(Debug, Clone)]
pub struct HttpOutboxPublisher {
    inner: Arc<HttpPublisherInner>,
}

#[derive(Debug)]
struct HttpPublisherInner {
    client: reqwest::Client,
    endpoint: String,
    consecutive_failures: AtomicU32,
    circuit_open_until_epoch: AtomicI64,
    failure_threshold: u32,
    cooldown_seconds: i64,
}

impl HttpOutboxPublisher {
    pub fn new(endpoint: String, config: &OutboxWorkerConfig) -> Result<Self, PublishError> {
        let client = reqwest::ClientBuilder::new()
            .connect_timeout(StdDuration::from_millis(config.http_connect_timeout_ms))
            .timeout(StdDuration::from_millis(config.http_request_timeout_ms))
            .pool_max_idle_per_host(config.http_pool_max_idle_per_host)
            .tcp_keepalive(StdDuration::from_secs(30))
            .build()
            .map_err(|error| PublishError {
                kind: PublishErrorKind::Transient,
                message: format!("http client build failed: {error}"),
            })?;
        Ok(Self {
            inner: Arc::new(HttpPublisherInner {
                client,
                endpoint,
                consecutive_failures: AtomicU32::new(0),
                circuit_open_until_epoch: AtomicI64::new(0),
                failure_threshold: config.circuit_failure_threshold,
                cooldown_seconds: config.circuit_cooldown_seconds,
            }),
        })
    }

    fn breaker_open(&self, now_epoch: i64) -> bool {
        let until = self.inner.circuit_open_until_epoch.load(Ordering::Acquire);
        until > now_epoch
    }

    fn record_success(&self) {
        self.inner.consecutive_failures.store(0, Ordering::Release);
        self.inner
            .circuit_open_until_epoch
            .store(0, Ordering::Release);
    }

    fn record_transient_failure(&self, now_epoch: i64) {
        if self.inner.failure_threshold == 0 {
            return;
        }
        let next = self
            .inner
            .consecutive_failures
            .fetch_add(1, Ordering::AcqRel)
            + 1;
        if next >= self.inner.failure_threshold {
            self.inner.circuit_open_until_epoch.store(
                now_epoch.saturating_add(self.inner.cooldown_seconds),
                Ordering::Release,
            );
        }
    }
}

#[async_trait]
impl OutboxPublisher for HttpOutboxPublisher {
    async fn publish_issue_created(
        &self,
        message: &OutboxMessage,
        event: &IssueCreatedEventV1,
    ) -> Result<(), PublishError> {
        let now_epoch = Utc::now().timestamp();
        if self.breaker_open(now_epoch) {
            return Err(PublishError {
                kind: PublishErrorKind::Transient,
                message: "circuit breaker open; skipping remote call".to_owned(),
            });
        }

        let send_result = self
            .inner
            .client
            .post(self.inner.endpoint.as_str())
            .json(&serde_json::json!({
                "event_type": EventType::IssueCreatedV1.as_str(),
                "event_id": message.event_id,
                "workspace_id": message.workspace_id,
                "event": event,
            }))
            .send()
            .await;

        let response = match send_result {
            Ok(response) => response,
            Err(error) => {
                self.record_transient_failure(now_epoch);
                return Err(PublishError {
                    kind: PublishErrorKind::Transient,
                    message: format!("http publish request failed: {error}"),
                });
            }
        };

        let status = response.status();
        if status.is_success() {
            self.record_success();
            return Ok(());
        }

        let text = response.text().await.unwrap_or_default();
        if status.is_server_error() {
            self.record_transient_failure(now_epoch);
            Err(PublishError {
                kind: PublishErrorKind::Transient,
                message: format!("http publish rejected with status {status}: {text}"),
            })
        } else {
            // Client errors are terminal — don't count toward breaker.
            self.record_success();
            Err(PublishError {
                kind: PublishErrorKind::Permanent,
                message: format!("http publish rejected with status {status}: {text}"),
            })
        }
    }
}

/// Sample the full retry-backoff including jitter. Exposed so worker runs can
/// pass the same `Duration` to the store's failure context.
pub(super) fn retry_backoff_with_jitter(base_seconds: i64, jitter_ms: u64) -> Duration {
    use rand::Rng;
    let base = Duration::seconds(base_seconds.max(0));
    if jitter_ms == 0 {
        return base;
    }
    let jitter = rand::rng().random_range(0..jitter_ms.max(1));
    base + Duration::milliseconds(i64::try_from(jitter).unwrap_or(0))
}
