use std::time::Duration;

use anyhow::Context;
use dandori_app_services::{OutboxWorkerConfig, build_outbox_worker_service};
use uuid::Uuid;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let database_url = std::env::var("DANDORI_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5432/postgres".to_owned());
    let run_migrations = std::env::var("DANDORI_RUN_MIGRATIONS")
        .ok()
        .is_some_and(|value| value.eq_ignore_ascii_case("true"));

    let workspace_id = std::env::var("DANDORI_WORKER_WORKSPACE_ID")
        .context("DANDORI_WORKER_WORKSPACE_ID is required")
        .and_then(|value| Uuid::parse_str(value.as_str()).context("invalid workspace uuid"))?;

    let actor_id = std::env::var("DANDORI_WORKER_ACTOR_ID")
        .ok()
        .map(|value| Uuid::parse_str(value.as_str()).context("invalid actor uuid"))
        .transpose()?
        .unwrap_or_else(Uuid::nil);

    let interval_ms = std::env::var("DANDORI_WORKER_INTERVAL_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1_000);

    let config = OutboxWorkerConfig {
        workspace_id,
        actor_id,
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
