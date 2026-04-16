use std::time::Duration;

use anyhow::Context;
use dandori_app_services::{OutboxWorkerConfig, OutboxWorkerService, build_outbox_worker_service};
use tracing::{info, warn};
use uuid::Uuid;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let database_url =
        std::env::var("DANDORI_DATABASE_URL").context("DANDORI_DATABASE_URL is required")?;
    let run_migrations = std::env::var("DANDORI_RUN_MIGRATIONS")
        .ok()
        .is_some_and(|value| value.eq_ignore_ascii_case("true"));

    let worker_instance_id = std::env::var("DANDORI_WORKER_INSTANCE_ID")
        .ok()
        .map(|value| Uuid::parse_str(value.as_str()).context("invalid worker instance uuid"))
        .transpose()?
        .unwrap_or_else(|| {
            let generated = Uuid::now_v7();
            warn!(
                worker_instance_id = %generated,
                "DANDORI_WORKER_INSTANCE_ID not set; generated a fresh instance id (not stable across restarts)"
            );
            generated
        });

    let interval_ms = parse_env_or("DANDORI_WORKER_INTERVAL_MS", 1_000u64);
    let partition_batch = parse_env_or("DANDORI_WORKER_PARTITION_BATCH", 64usize);
    let partition_lease_seconds = parse_env_or("DANDORI_WORKER_PARTITION_LEASE_SECONDS", 60i64);
    let publish_concurrency = parse_env_or("DANDORI_WORKER_PUBLISH_CONCURRENCY", 8usize);
    let http_connect_timeout_ms = parse_env_or("DANDORI_WORKER_HTTP_CONNECT_TIMEOUT_MS", 2_000u64);
    let http_request_timeout_ms = parse_env_or("DANDORI_WORKER_HTTP_REQUEST_TIMEOUT_MS", 10_000u64);
    let retry_jitter_ms = parse_env_or("DANDORI_WORKER_RETRY_JITTER_MS", 2_000u64);
    let circuit_failure_threshold = parse_env_or("DANDORI_WORKER_CIRCUIT_FAILURE_THRESHOLD", 10u32);
    let circuit_cooldown_seconds = parse_env_or("DANDORI_WORKER_CIRCUIT_COOLDOWN_SECONDS", 30i64);

    let config = OutboxWorkerConfig {
        workspace_ids: None,
        worker_instance_id,
        batch_size: 32,
        lease_seconds: 30,
        max_attempts: 5,
        retry_backoff_seconds: 15,
        delivered_retention_hours: 24,
        dead_letter_retention_hours: 168,
        idempotency_retention_hours: 168,
        publish_concurrency,
        http_connect_timeout_ms,
        http_request_timeout_ms,
        http_pool_max_idle_per_host: 16,
        retry_jitter_ms,
        circuit_failure_threshold,
        circuit_cooldown_seconds,
        partition_batch,
        partition_lease_seconds,
    };

    let worker = build_outbox_worker_service(&database_url, run_migrations, config).await?;

    info!(
        worker_instance_id = %worker_instance_id,
        interval_ms,
        "dandori-worker started"
    );

    let outcome = run_loop(&worker, interval_ms).await;
    if let Err(error) = worker.release_partitions().await {
        warn!(error = %error, "failed to release partitions on shutdown");
    }
    outcome
}

async fn run_loop(worker: &OutboxWorkerService, interval_ms: u64) -> anyhow::Result<()> {
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("dandori-worker received shutdown signal");
                return Ok(());
            }
            _ = tokio::time::sleep(Duration::from_millis(interval_ms)) => {
                let _report = worker.run_once().await?;
            }
        }
    }
}

fn parse_env_or<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<T>().ok())
        .unwrap_or(default)
}
