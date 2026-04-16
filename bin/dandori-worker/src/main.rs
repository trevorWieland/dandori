use std::env::VarError;
use std::ops::RangeInclusive;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, bail};
use dandori_app_services::{
    OutboxWorkerConfig, OutboxWorkerService, ShardBucketRange, build_outbox_worker_service,
};
use tracing::{info, warn};
use uuid::Uuid;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let database_url =
        std::env::var("DANDORI_DATABASE_URL").context("DANDORI_DATABASE_URL is required")?;
    let run_migrations = parse_env_bool("DANDORI_RUN_MIGRATIONS", false)?;

    let worker_instance_id = match std::env::var("DANDORI_WORKER_INSTANCE_ID") {
        Ok(raw) => Uuid::parse_str(raw.as_str())
            .with_context(|| format!("DANDORI_WORKER_INSTANCE_ID invalid uuid: {raw:?}"))?,
        Err(VarError::NotPresent) => {
            let generated = Uuid::now_v7();
            warn!(
                worker_instance_id = %generated,
                "DANDORI_WORKER_INSTANCE_ID not set; generated a fresh instance id (not stable across restarts)"
            );
            generated
        }
        Err(VarError::NotUnicode(raw)) => {
            bail!("DANDORI_WORKER_INSTANCE_ID contains non-UTF8 bytes: {raw:?}")
        }
    };

    let interval_ms = parse_env_bounded("DANDORI_WORKER_INTERVAL_MS", 1_000u64, 50..=600_000)?;
    let partition_batch = parse_env_bounded("DANDORI_WORKER_PARTITION_BATCH", 64usize, 1..=10_000)?;
    let partition_lease_seconds =
        parse_env_bounded("DANDORI_WORKER_PARTITION_LEASE_SECONDS", 60i64, 1..=3_600)?;
    let publish_concurrency =
        parse_env_bounded("DANDORI_WORKER_PUBLISH_CONCURRENCY", 8usize, 1..=256)?;
    let tenant_max_concurrency =
        parse_env_bounded("DANDORI_WORKER_TENANT_MAX_CONCURRENCY", 8usize, 1..=256)?;
    let http_connect_timeout_ms = parse_env_bounded(
        "DANDORI_WORKER_HTTP_CONNECT_TIMEOUT_MS",
        2_000u64,
        1..=120_000,
    )?;
    let http_request_timeout_ms = parse_env_bounded(
        "DANDORI_WORKER_HTTP_REQUEST_TIMEOUT_MS",
        10_000u64,
        1..=600_000,
    )?;
    let retry_jitter_ms =
        parse_env_bounded("DANDORI_WORKER_RETRY_JITTER_MS", 2_000u64, 0..=60_000)?;
    let circuit_failure_threshold = parse_env_bounded(
        "DANDORI_WORKER_CIRCUIT_FAILURE_THRESHOLD",
        10u32,
        0..=10_000,
    )?;
    let circuit_cooldown_seconds =
        parse_env_bounded("DANDORI_WORKER_CIRCUIT_COOLDOWN_SECONDS", 30i64, 0..=3_600)?;
    let batch_size = parse_env_bounded("DANDORI_WORKER_BATCH_SIZE", 32i64, 1..=10_000)?;
    let lease_seconds = parse_env_bounded("DANDORI_WORKER_LEASE_SECONDS", 30i64, 1..=3_600)?;
    let max_attempts = parse_env_bounded("DANDORI_WORKER_MAX_ATTEMPTS", 5i32, 1..=1_000)?;
    let retry_backoff_seconds =
        parse_env_bounded("DANDORI_WORKER_RETRY_BACKOFF_SECONDS", 15i64, 0..=3_600)?;
    let delivered_retention_hours =
        parse_env_bounded("DANDORI_WORKER_DELIVERED_RETENTION_HOURS", 24i64, 1..=8_760)?;
    let dead_letter_retention_hours = parse_env_bounded(
        "DANDORI_WORKER_DEAD_LETTER_RETENTION_HOURS",
        168i64,
        1..=17_520,
    )?;
    let idempotency_retention_hours = parse_env_bounded(
        "DANDORI_WORKER_IDEMPOTENCY_RETENTION_HOURS",
        168i64,
        1..=17_520,
    )?;
    let http_pool_max_idle_per_host = parse_env_bounded(
        "DANDORI_WORKER_HTTP_POOL_MAX_IDLE_PER_HOST",
        16usize,
        0..=1_024,
    )?;
    let partition_shard_buckets = parse_shard_bucket_range("DANDORI_WORKER_BUCKET_RANGE")?;

    let config = OutboxWorkerConfig {
        workspace_ids: None,
        worker_instance_id,
        batch_size,
        lease_seconds,
        max_attempts,
        retry_backoff_seconds,
        delivered_retention_hours,
        dead_letter_retention_hours,
        idempotency_retention_hours,
        publish_concurrency,
        http_connect_timeout_ms,
        http_request_timeout_ms,
        http_pool_max_idle_per_host,
        retry_jitter_ms,
        circuit_failure_threshold,
        circuit_cooldown_seconds,
        partition_batch,
        partition_lease_seconds,
        partition_shard_buckets,
        tenant_max_concurrency,
    };

    info!(
        worker_instance_id = %worker_instance_id,
        interval_ms,
        partition_batch,
        tenant_max_concurrency,
        publish_concurrency,
        bucket_min = partition_shard_buckets.min(),
        bucket_max = partition_shard_buckets.max(),
        "dandori-worker configuration resolved"
    );

    let worker = build_outbox_worker_service(&database_url, run_migrations, config).await?;

    info!(
        worker_instance_id = %worker_instance_id,
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

/// Read an env var and parse it. Returns the default when the var is unset,
/// but **fails closed** with a descriptive error on parse failure or
/// out-of-range values. Silent fallback on bad input is never acceptable for
/// worker configuration.
fn parse_env_bounded<T>(name: &str, default: T, bounds: RangeInclusive<T>) -> anyhow::Result<T>
where
    T: FromStr + PartialOrd + Copy + std::fmt::Display,
    <T as FromStr>::Err: std::fmt::Display,
{
    let raw = read_env(name)?;
    parse_bounded(name, raw.as_deref(), default, bounds)
}

fn parse_env_bool(name: &str, default: bool) -> anyhow::Result<bool> {
    let raw = read_env(name)?;
    parse_bool(name, raw.as_deref(), default)
}

fn parse_shard_bucket_range(name: &str) -> anyhow::Result<ShardBucketRange> {
    let raw = read_env(name)?;
    parse_bucket_range(name, raw.as_deref())
}

fn read_env(name: &str) -> anyhow::Result<Option<String>> {
    match std::env::var(name) {
        Ok(raw) => Ok(Some(raw)),
        Err(VarError::NotPresent) => Ok(None),
        Err(VarError::NotUnicode(raw)) => bail!("{name} contains non-UTF8 bytes: {raw:?}"),
    }
}

fn parse_bounded<T>(
    name: &str,
    raw: Option<&str>,
    default: T,
    bounds: RangeInclusive<T>,
) -> anyhow::Result<T>
where
    T: FromStr + PartialOrd + Copy + std::fmt::Display,
    <T as FromStr>::Err: std::fmt::Display,
{
    let Some(raw) = raw else { return Ok(default) };
    let parsed = raw
        .parse::<T>()
        .map_err(|err| anyhow::anyhow!("{name} invalid value {raw:?}: {err}"))?;
    if !bounds.contains(&parsed) {
        bail!(
            "{name}={parsed} out of range [{min}, {max}]",
            min = bounds.start(),
            max = bounds.end()
        );
    }
    Ok(parsed)
}

fn parse_bool(name: &str, raw: Option<&str>, default: bool) -> anyhow::Result<bool> {
    let Some(raw) = raw else { return Ok(default) };
    match raw.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" | "" => Ok(false),
        other => bail!("{name} invalid boolean: {other:?}"),
    }
}

fn parse_bucket_range(name: &str, raw: Option<&str>) -> anyhow::Result<ShardBucketRange> {
    let Some(raw) = raw else {
        return Ok(ShardBucketRange::full());
    };
    let (min_str, max_str) = raw
        .split_once("..")
        .with_context(|| format!("{name} must be formatted as MIN..MAX, got {raw:?}"))?;
    let min: i32 = min_str
        .parse()
        .with_context(|| format!("{name} min {min_str:?} is not an integer"))?;
    let max: i32 = max_str
        .parse()
        .with_context(|| format!("{name} max {max_str:?} is not an integer"))?;
    ShardBucketRange::new(min, max)
        .map_err(|err| anyhow::anyhow!("{name} invalid shard bucket range: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_rejects_parse_failure() {
        let r: anyhow::Result<u64> = parse_bounded("X", Some("not-a-number"), 1, 0..=10);
        assert!(r.is_err(), "expected parse failure");
    }

    #[test]
    fn bounded_rejects_out_of_range() {
        let r: anyhow::Result<u64> = parse_bounded("Y", Some("42"), 1, 0..=10);
        assert!(r.is_err(), "expected range failure");
    }

    #[test]
    fn bounded_accepts_in_range() {
        let r: anyhow::Result<u64> = parse_bounded("Z", Some("7"), 1, 0..=10);
        assert_eq!(r.expect("in range"), 7);
    }

    #[test]
    fn bounded_uses_default_when_unset() {
        let r: anyhow::Result<u64> = parse_bounded("W", None, 5, 0..=10);
        assert_eq!(r.expect("default returned"), 5);
    }

    #[test]
    fn negative_duration_rejected_for_signed_bound() {
        let r: anyhow::Result<i64> = parse_bounded("N", Some("-3"), 30, 1..=3_600);
        assert!(r.is_err(), "negatives must fail closed");
    }

    #[test]
    fn bucket_range_parses_valid() {
        let r = parse_bucket_range("B", Some("0..511")).expect("valid range");
        assert_eq!(r.min(), 0);
        assert_eq!(r.max(), 511);
    }

    #[test]
    fn bucket_range_rejects_malformed() {
        assert!(parse_bucket_range("B", Some("0-511")).is_err());
    }

    #[test]
    fn bucket_range_rejects_out_of_bounds() {
        assert!(parse_bucket_range("B", Some("0..5000")).is_err());
    }

    #[test]
    fn bucket_range_defaults_when_unset() {
        let r = parse_bucket_range("B", None).expect("default returned");
        assert_eq!(r.min(), 0);
        assert_eq!(r.max(), ShardBucketRange::MAX_BUCKET);
    }

    #[test]
    fn bool_accepts_truthy_and_falsy() {
        assert!(parse_bool("F", Some("yes"), false).expect("parse yes"));
        assert!(!parse_bool("F", Some("no"), true).expect("parse no"));
        assert!(parse_bool("F", Some("bogus"), true).is_err());
    }
}
