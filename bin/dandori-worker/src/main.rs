use std::time::Duration;

use anyhow::Context;
use dandori_app_services::{OutboxWorkerConfig, build_outbox_worker_service};
use uuid::Uuid;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let database_url =
        std::env::var("DANDORI_DATABASE_URL").context("DANDORI_DATABASE_URL is required")?;
    let run_migrations = std::env::var("DANDORI_RUN_MIGRATIONS")
        .ok()
        .is_some_and(|value| value.eq_ignore_ascii_case("true"));

    let workspace_ids = std::env::var("DANDORI_WORKER_WORKSPACE_IDS")
        .context("DANDORI_WORKER_WORKSPACE_IDS is required (comma-separated UUID list)")
        .and_then(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| Uuid::parse_str(value).context("invalid workspace uuid in shard list"))
                .collect::<Result<Vec<_>, _>>()
        })?;
    if workspace_ids.is_empty() {
        anyhow::bail!("DANDORI_WORKER_WORKSPACE_IDS must contain at least one workspace UUID");
    }

    let shard_index = std::env::var("DANDORI_WORKER_SHARD_INDEX")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(0);
    let shard_total = std::env::var("DANDORI_WORKER_SHARD_TOTAL")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(1)
        .max(1);
    if shard_index >= shard_total {
        anyhow::bail!(
            "DANDORI_WORKER_SHARD_INDEX ({shard_index}) must be less than DANDORI_WORKER_SHARD_TOTAL ({shard_total})"
        );
    }

    let worker_instance_id = std::env::var("DANDORI_WORKER_INSTANCE_ID")
        .ok()
        .map(|value| Uuid::parse_str(value.as_str()).context("invalid worker instance uuid"))
        .transpose()?
        .unwrap_or_else(Uuid::now_v7);

    let interval_ms = std::env::var("DANDORI_WORKER_INTERVAL_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1_000);

    let config = OutboxWorkerConfig {
        workspace_ids,
        shard_index,
        shard_total,
        worker_instance_id,
        batch_size: 32,
        lease_seconds: 30,
        max_attempts: 5,
        retry_backoff_seconds: 15,
        delivered_retention_hours: 24,
        dead_letter_retention_hours: 168,
        idempotency_retention_hours: 168,
    };

    let worker = build_outbox_worker_service(&database_url, run_migrations, config).await?;

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = tokio::time::sleep(Duration::from_millis(interval_ms)) => {
                let _report = worker.run_once().await?;
            }
        }
    }

    Ok(())
}
