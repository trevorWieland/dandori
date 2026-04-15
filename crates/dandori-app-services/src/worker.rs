use chrono::{Duration, Utc};
use dandori_domain::AuthContext;
use dandori_store::{PgStore, migrate_database};
use thiserror::Error;
use uuid::Uuid;

use crate::{AppServiceError, ErrorKind};

#[derive(Debug, Clone)]
pub struct OutboxWorkerConfig {
    pub workspace_id: Uuid,
    pub actor_id: Uuid,
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
            workspace_id: Uuid::nil(),
            actor_id: Uuid::nil(),
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

#[derive(Debug, Error)]
enum WorkerError {
    #[error("unsupported outbox event type '{0}'")]
    UnsupportedEvent(String),
}

#[derive(Debug, Clone)]
pub struct OutboxWorkerService {
    store: PgStore,
    config: OutboxWorkerConfig,
}

impl OutboxWorkerService {
    #[must_use]
    pub fn new(store: PgStore, config: OutboxWorkerConfig) -> Self {
        Self { store, config }
    }

    pub async fn run_once(&self) -> Result<WorkerRunReport, AppServiceError> {
        let auth = AuthContext {
            workspace_id: self.config.workspace_id.into(),
            actor_id: self.config.actor_id,
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

        let mut report = WorkerRunReport {
            leased: leased.len(),
            ..WorkerRunReport::default()
        };

        for message in leased {
            if process_outbox_event(message.event_type.as_str(), &message.payload).is_ok() {
                self.store
                    .mark_outbox_delivered(&auth, message.id, Utc::now())
                    .await
                    .map_err(map_store_worker_error)?;
                report.delivered += 1;
            } else {
                let previous_attempts = message.attempts;
                self.store
                    .mark_outbox_failed(
                        &auth,
                        message.id,
                        Utc::now(),
                        "worker_publish_failed",
                        self.config.max_attempts,
                        Duration::seconds(self.config.retry_backoff_seconds),
                    )
                    .await
                    .map_err(map_store_worker_error)?;
                report.failed += 1;
                if previous_attempts + 1 >= self.config.max_attempts {
                    report.dead_lettered += 1;
                }
            }
        }

        report.cleaned_outbox_rows = self
            .store
            .cleanup_outbox(
                &auth,
                Utc::now() - Duration::hours(self.config.delivered_retention_hours),
                Utc::now() - Duration::hours(self.config.dead_letter_retention_hours),
            )
            .await
            .map_err(map_store_worker_error)?;

        report.cleaned_idempotency_rows = self
            .store
            .cleanup_idempotency(
                &auth,
                Utc::now() - Duration::hours(self.config.idempotency_retention_hours),
            )
            .await
            .map_err(map_store_worker_error)?;

        Ok(report)
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

    Ok(OutboxWorkerService::new(store, config))
}

fn process_outbox_event(event_type: &str, payload: &serde_json::Value) -> Result<(), WorkerError> {
    match event_type {
        "issue.created.v1" => {
            let _ = payload;
            Ok(())
        }
        _ => Err(WorkerError::UnsupportedEvent(event_type.to_owned())),
    }
}

fn map_store_worker_error(error: dandori_store::StoreError) -> AppServiceError {
    AppServiceError {
        code: "worker_store_failed",
        message: error.to_string(),
        kind: ErrorKind::Infrastructure,
    }
}
