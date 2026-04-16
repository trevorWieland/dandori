use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Duration, Utc};
use dandori_domain::{AuthContext, IssueCreatedEventV1};
use dandori_store::{OutboxFailureContext, OutboxMessage, PgStore, migrate_database};
use thiserror::Error;
use tracing::{info, warn};
use uuid::Uuid;

use crate::{AppServiceError, ErrorKind};

#[derive(Debug, Clone)]
pub struct OutboxWorkerConfig {
    pub workspace_ids: Vec<Uuid>,
    pub shard_index: u16,
    pub shard_total: u16,
    pub worker_instance_id: Uuid,
    pub batch_size: i64,
    pub lease_seconds: i64,
    pub max_attempts: i32,
    pub retry_backoff_seconds: i64,
    pub delivered_retention_hours: i64,
    pub dead_letter_retention_hours: i64,
    pub idempotency_retention_hours: i64,
}

impl Default for OutboxWorkerConfig {
    fn default() -> Self {
        Self {
            workspace_ids: Vec::new(),
            shard_index: 0,
            shard_total: 1,
            worker_instance_id: Uuid::nil(),
            batch_size: 32,
            lease_seconds: 30,
            max_attempts: 5,
            retry_backoff_seconds: 15,
            delivered_retention_hours: 24,
            dead_letter_retention_hours: 168,
            idempotency_retention_hours: 168,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WorkerRunReport {
    pub leased: usize,
    pub delivered: usize,
    pub failed: usize,
    pub dead_lettered: usize,
    pub cleaned_outbox_rows: u64,
    pub cleaned_idempotency_rows: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishErrorKind {
    Transient,
    Permanent,
    Unsupported,
    Serialization,
}

#[derive(Debug, Error)]
#[error("{kind:?}: {message}")]
pub struct PublishError {
    pub kind: PublishErrorKind,
    pub message: String,
}

#[async_trait]
pub trait OutboxPublisher: Send + Sync {
    async fn publish_issue_created(
        &self,
        message: &OutboxMessage,
        event: &IssueCreatedEventV1,
    ) -> Result<(), PublishError>;
}

#[derive(Debug, Clone)]
pub struct HttpOutboxPublisher {
    client: reqwest::Client,
    endpoint: String,
}

impl HttpOutboxPublisher {
    #[must_use]
    pub fn new(endpoint: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            endpoint,
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
        let response = self
            .client
            .post(self.endpoint.as_str())
            .json(&serde_json::json!({
                "event_type": "issue.created.v1",
                "event_id": message.event_id,
                "workspace_id": message.workspace_id,
                "event": event,
            }))
            .send()
            .await
            .map_err(|error| PublishError {
                kind: PublishErrorKind::Transient,
                message: format!("http publish request failed: {error}"),
            })?;

        let status = response.status();
        if status.is_success() {
            return Ok(());
        }

        let text = response.text().await.unwrap_or_default();
        let kind = if status.is_server_error() {
            PublishErrorKind::Transient
        } else {
            PublishErrorKind::Permanent
        };

        Err(PublishError {
            kind,
            message: format!("http publish rejected with status {status}: {text}"),
        })
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct NoopOutboxPublisher;

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

#[derive(Clone)]
pub struct OutboxWorkerService {
    store: PgStore,
    config: OutboxWorkerConfig,
    publisher: Arc<dyn OutboxPublisher>,
}

impl std::fmt::Debug for OutboxWorkerService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutboxWorkerService")
            .field("store", &self.store)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

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
        }
    }

    #[must_use]
    pub fn with_default_publisher(store: PgStore, config: OutboxWorkerConfig) -> Self {
        let publisher = if let Ok(endpoint) = std::env::var("DANDORI_OUTBOX_PUBLISH_URL") {
            Arc::new(HttpOutboxPublisher::new(endpoint)) as Arc<dyn OutboxPublisher>
        } else {
            Arc::new(NoopOutboxPublisher) as Arc<dyn OutboxPublisher>
        };
        Self::new(store, config, publisher)
    }

    pub async fn run_once(&self) -> Result<WorkerRunReport, AppServiceError> {
        let mut report = WorkerRunReport::default();

        let assigned_workspaces = self.assigned_workspace_ids();
        if assigned_workspaces.is_empty() {
            warn!("worker has no assigned workspaces for this shard");
            return Ok(report);
        }

        for workspace_id in assigned_workspaces {
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

            for message in leased {
                let previous_attempts = message.attempts;
                match self.route_and_publish(&message).await {
                    Ok(()) => {
                        self.store
                            .mark_outbox_delivered(
                                &auth,
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
                        let failure = OutboxFailureContext {
                            lease_token: message.lease_token,
                            lease_owner: message.lease_owner,
                            now: Utc::now(),
                            error_message: format!("{:?}: {}", error.kind, error.message),
                            max_attempts: self.config.max_attempts,
                            retry_backoff: Duration::seconds(self.config.retry_backoff_seconds),
                        };
                        self.store
                            .mark_outbox_failed(&auth, message.id, failure)
                            .await
                            .map_err(map_store_worker_error)?;

                        report.failed += 1;
                        if previous_attempts + 1 >= self.config.max_attempts {
                            report.dead_lettered += 1;
                        }
                        warn!(
                            outbox_id = %message.id,
                            event_type = %message.event_type,
                            previous_attempts,
                            failure_kind = ?error.kind,
                            failure = %error.message,
                            "outbox publish failed"
                        );
                    }
                }
            }

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

    fn assigned_workspace_ids(&self) -> Vec<Uuid> {
        let shard_total = self.config.shard_total.max(1);
        self.config
            .workspace_ids
            .iter()
            .copied()
            .filter(|workspace_id| {
                (workspace_id.as_u128() % u128::from(shard_total))
                    == u128::from(self.config.shard_index)
            })
            .collect()
    }

    async fn route_and_publish(&self, message: &OutboxMessage) -> Result<(), PublishError> {
        match message.event_type.as_str() {
            "issue.created.v1" => {
                let event: IssueCreatedEventV1 = serde_json::from_value(message.payload.clone())
                    .map_err(|error| PublishError {
                        kind: PublishErrorKind::Serialization,
                        message: format!("failed to deserialize issue.created payload: {error}"),
                    })?;
                self.publisher.publish_issue_created(message, &event).await
            }
            other => Err(PublishError {
                kind: PublishErrorKind::Unsupported,
                message: format!("unsupported outbox event type '{other}'"),
            }),
        }
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

    Ok(OutboxWorkerService::with_default_publisher(store, config))
}

fn map_store_worker_error(error: dandori_store::StoreError) -> AppServiceError {
    AppServiceError {
        code: "worker_store_failed",
        message: error.to_string(),
        kind: ErrorKind::Infrastructure,
    }
}
