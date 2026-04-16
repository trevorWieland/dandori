use std::sync::Arc;

use chrono::{Duration, Utc};
use dandori_domain::{AuthContext, EventType, IssueCreatedEventV1};
use dandori_store::{
    OutboxFailureClassification, OutboxFailureContext, OutboxMessage, PgStore, migrate_database,
};
use futures::stream::StreamExt;
use tokio::sync::Mutex;
use tracing::{info, warn};
use uuid::Uuid;

use super::config::{OutboxWorkerConfig, WorkerRunReport};
use super::publish::{
    HttpOutboxPublisher, NoopOutboxPublisher, OutboxPublisher, PublishError, PublishErrorKind,
    retry_backoff_with_jitter,
};
use crate::{AppServiceError, ErrorKind};

/// Top-level worker service. Holds the store, publisher, and config; callers
/// drive one cycle at a time via [`run_once`](Self::run_once).
#[derive(Clone)]
pub struct OutboxWorkerService {
    store: PgStore,
    config: OutboxWorkerConfig,
    publisher: Arc<dyn OutboxPublisher>,
    partition_state: Arc<Mutex<PartitionState>>,
}

#[derive(Debug, Default)]
struct PartitionState {
    leased: Vec<Uuid>,
}

impl std::fmt::Debug for OutboxWorkerService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutboxWorkerService")
            .field("store", &self.store)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// Environment variable that enables the no-op publisher (dev-only escape
/// hatch). The worker otherwise refuses to start without a real publisher.
const NOOP_PUBLISHER_ENV: &str = "DANDORI_OUTBOX_ALLOW_NOOP_PUBLISHER";
/// Environment variable that configures the HTTP publisher endpoint.
const PUBLISH_URL_ENV: &str = "DANDORI_OUTBOX_PUBLISH_URL";

impl OutboxWorkerService {
    #[must_use]
    pub fn new(
        store: PgStore,
        config: OutboxWorkerConfig,
        publisher: Arc<dyn OutboxPublisher>,
    ) -> Self {
        Self {
            store,
            config,
            publisher,
            partition_state: Arc::new(Mutex::new(PartitionState::default())),
        }
    }

    /// Wire the service with a publisher chosen from environment variables.
    /// Fails closed: a missing `DANDORI_OUTBOX_PUBLISH_URL` is an error unless
    /// `DANDORI_OUTBOX_ALLOW_NOOP_PUBLISHER` is explicitly set to a truthy
    /// value.
    pub fn with_default_publisher(
        store: PgStore,
        config: OutboxWorkerConfig,
    ) -> Result<Self, AppServiceError> {
        let publish_url = std::env::var(PUBLISH_URL_ENV).ok();
        let allow_noop_raw = std::env::var(NOOP_PUBLISHER_ENV).ok();
        Self::with_publisher_selection(
            store,
            config,
            publish_url.as_deref(),
            allow_noop_raw.as_deref(),
        )
    }

    /// Pure publisher-selection entry point used by
    /// [`with_default_publisher`](Self::with_default_publisher) and by
    /// integration tests that do not want to touch process-wide env state.
    /// Always fails closed when `publish_url` is `None` unless
    /// `allow_noop_raw` is a truthy literal (`"1"` or `"true"`).
    pub fn with_publisher_selection(
        store: PgStore,
        config: OutboxWorkerConfig,
        publish_url: Option<&str>,
        allow_noop_raw: Option<&str>,
    ) -> Result<Self, AppServiceError> {
        let allow_noop =
            allow_noop_raw.is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));

        let publisher: Arc<dyn OutboxPublisher> = match (publish_url, allow_noop) {
            (Some(endpoint), _) => Arc::new(
                HttpOutboxPublisher::new(endpoint.to_owned(), &config).map_err(|error| {
                    AppServiceError {
                        code: "publisher_init_failed",
                        message: error.message,
                        kind: ErrorKind::Infrastructure,
                    }
                })?,
            ),
            (None, true) => {
                warn!(
                    "outbox publisher is NOOP ({NOOP_PUBLISHER_ENV}=1); events will not leave the database"
                );
                Arc::new(NoopOutboxPublisher)
            }
            (None, false) => {
                return Err(AppServiceError {
                    code: "publisher_not_configured",
                    message: format!(
                        "{PUBLISH_URL_ENV} is required unless {NOOP_PUBLISHER_ENV}=1 is set (dev only)"
                    ),
                    kind: ErrorKind::Infrastructure,
                });
            }
        };

        Ok(Self::new(store, config, publisher))
    }

    pub async fn run_once(&self) -> Result<WorkerRunReport, AppServiceError> {
        let mut report = WorkerRunReport::default();
        let assigned = self.acquire_workspaces().await?;
        if assigned.is_empty() {
            return Ok(report);
        }

        for workspace_id in assigned {
            let auth = AuthContext {
                workspace_id: workspace_id.into(),
                actor_id: self.config.worker_instance_id,
            };

            let now = Utc::now();
            let leased = self
                .store
                .lease_outbox_batch(
                    &auth,
                    now,
                    Duration::seconds(self.config.lease_seconds),
                    self.config.batch_size,
                )
                .await
                .map_err(map_store_worker_error)?;

            info!(
                workspace_id = %auth.workspace_id,
                leased = leased.len(),
                "worker leased outbox rows"
            );
            report.leased += leased.len();

            self.process_leased(&auth, leased, &mut report).await?;

            report.cleaned_outbox_rows += self
                .store
                .cleanup_outbox(
                    &auth,
                    Utc::now() - Duration::hours(self.config.delivered_retention_hours),
                    Utc::now() - Duration::hours(self.config.dead_letter_retention_hours),
                )
                .await
                .map_err(map_store_worker_error)?;

            report.cleaned_idempotency_rows += self
                .store
                .cleanup_idempotency(
                    &auth,
                    Utc::now() - Duration::hours(self.config.idempotency_retention_hours),
                )
                .await
                .map_err(map_store_worker_error)?;
        }

        info!(
            leased = report.leased,
            delivered = report.delivered,
            failed = report.failed,
            dead_lettered = report.dead_lettered,
            cleaned_outbox_rows = report.cleaned_outbox_rows,
            cleaned_idempotency_rows = report.cleaned_idempotency_rows,
            "worker run finished"
        );

        Ok(report)
    }

    /// Release dynamic partition leases. Call on graceful shutdown so peer
    /// workers can pick up the partitions faster than waiting for lease
    /// expiry.
    pub async fn release_partitions(&self) -> Result<(), AppServiceError> {
        if self.config.workspace_ids.is_some() {
            return Ok(());
        }
        let mut state = self.partition_state.lock().await;
        if state.leased.is_empty() {
            return Ok(());
        }
        let partitions = std::mem::take(&mut state.leased);
        self.store
            .release_partitions(self.config.worker_instance_id, &partitions)
            .await
            .map_err(map_store_worker_error)?;
        Ok(())
    }

    async fn acquire_workspaces(&self) -> Result<Vec<Uuid>, AppServiceError> {
        if let Some(static_list) = self.config.workspace_ids.as_ref() {
            return Ok(static_list.clone());
        }

        let now = Utc::now();
        let lease_until = now + Duration::seconds(self.config.partition_lease_seconds);
        let partitions = self
            .store
            .acquire_partitions(
                self.config.worker_instance_id,
                now,
                lease_until,
                i64::try_from(self.config.partition_batch).unwrap_or(i64::MAX),
            )
            .await
            .map_err(map_store_worker_error)?;
        if partitions.is_empty() {
            return Ok(Vec::new());
        }

        let mut state = self.partition_state.lock().await;
        state.leased = partitions.clone();
        drop(state);

        Ok(partitions)
    }

    async fn process_leased(
        &self,
        auth: &AuthContext,
        leased: Vec<OutboxMessage>,
        report: &mut WorkerRunReport,
    ) -> Result<(), AppServiceError> {
        let concurrency = self.config.publish_concurrency.max(1);
        let publisher = Arc::clone(&self.publisher);

        let completed: Vec<(OutboxMessage, Result<(), PublishError>)> =
            futures::stream::iter(leased)
                .map(|message| {
                    let publisher = Arc::clone(&publisher);
                    async move {
                        let outcome = route_and_publish(publisher.as_ref(), &message).await;
                        (message, outcome)
                    }
                })
                .buffer_unordered(concurrency)
                .collect()
                .await;

        for (message, outcome) in completed {
            self.apply_publish_outcome(auth, message, outcome, report)
                .await?;
        }
        Ok(())
    }

    async fn apply_publish_outcome(
        &self,
        auth: &AuthContext,
        message: OutboxMessage,
        outcome: Result<(), PublishError>,
        report: &mut WorkerRunReport,
    ) -> Result<(), AppServiceError> {
        let previous_attempts = message.attempts;
        match outcome {
            Ok(()) => {
                self.store
                    .mark_outbox_delivered(
                        auth,
                        message.id,
                        message.lease_token,
                        message.lease_owner,
                        Utc::now(),
                    )
                    .await
                    .map_err(map_store_worker_error)?;
                report.delivered += 1;
                info!(
                    outbox_id = %message.id,
                    event_type = %message.event_type,
                    "outbox message delivered"
                );
            }
            Err(error) => {
                let classification = if error.kind.is_transient() {
                    OutboxFailureClassification::Transient
                } else {
                    OutboxFailureClassification::Terminal
                };
                let retry_backoff = retry_backoff_with_jitter(
                    self.config.retry_backoff_seconds,
                    self.config.retry_jitter_ms,
                );
                let failure = OutboxFailureContext {
                    classification,
                    lease_token: message.lease_token,
                    lease_owner: message.lease_owner,
                    now: Utc::now(),
                    error_message: format!("{:?}: {}", error.kind, error.message),
                    max_attempts: self.config.max_attempts,
                    retry_backoff,
                };
                self.store
                    .mark_outbox_failed(auth, message.id, failure)
                    .await
                    .map_err(map_store_worker_error)?;

                report.failed += 1;
                let terminal = classification == OutboxFailureClassification::Terminal;
                let exhausted = previous_attempts + 1 >= self.config.max_attempts;
                if terminal || exhausted {
                    report.dead_lettered += 1;
                }
                warn!(
                    outbox_id = %message.id,
                    event_type = %message.event_type,
                    previous_attempts,
                    failure_kind = ?error.kind,
                    terminal,
                    exhausted_budget = exhausted,
                    failure = %error.message,
                    "outbox publish failed"
                );
            }
        }
        Ok(())
    }
}

async fn route_and_publish(
    publisher: &dyn OutboxPublisher,
    message: &OutboxMessage,
) -> Result<(), PublishError> {
    match EventType::parse(message.event_type.as_str()) {
        Some(EventType::IssueCreatedV1) => {
            let event: IssueCreatedEventV1 = serde_json::from_value(message.payload.clone())
                .map_err(|error| PublishError {
                    kind: PublishErrorKind::Serialization,
                    message: format!("failed to deserialize issue.created payload: {error}"),
                })?;
            publisher.publish_issue_created(message, &event).await
        }
        Some(other) => Err(PublishError {
            kind: PublishErrorKind::Unsupported,
            message: format!("event type '{other}' is not yet routable by the worker"),
        }),
        None => Err(PublishError {
            kind: PublishErrorKind::Unsupported,
            message: format!(
                "unknown outbox event type '{}' (not in domain event registry)",
                message.event_type
            ),
        }),
    }
}

pub async fn build_outbox_worker_service(
    database_url: &str,
    run_migrations: bool,
    config: OutboxWorkerConfig,
) -> Result<OutboxWorkerService, AppServiceError> {
    if run_migrations {
        migrate_database(database_url)
            .await
            .map_err(|error| AppServiceError {
                code: "migration_failed",
                message: error.to_string(),
                kind: ErrorKind::Infrastructure,
            })?;
    }

    let store = PgStore::connect(database_url)
        .await
        .map_err(|error| AppServiceError {
            code: "store_connect_failed",
            message: error.to_string(),
            kind: ErrorKind::Infrastructure,
        })?;

    OutboxWorkerService::with_default_publisher(store, config)
}

fn map_store_worker_error(error: dandori_store::StoreError) -> AppServiceError {
    AppServiceError {
        code: "worker_store_failed",
        message: error.to_string(),
        kind: ErrorKind::Infrastructure,
    }
}
