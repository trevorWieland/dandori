use uuid::Uuid;

/// Full configuration surface for the outbox worker. All knobs that affect
/// availability, throughput, or retry semantics live here; nothing is hidden
/// behind a default constructor that drops safety.
#[derive(Debug, Clone)]
pub struct OutboxWorkerConfig {
    /// Optional explicit workspace allow-list. When `None`, the worker uses
    /// dynamic partition leasing and discovers workspaces from the DB.
    /// Setting this is supported only for integration tests and the
    /// deprecated static-shard fallback; production deployments should leave
    /// it unset.
    pub workspace_ids: Option<Vec<Uuid>>,
    pub worker_instance_id: Uuid,
    pub batch_size: i64,
    pub lease_seconds: i64,
    pub max_attempts: i32,
    pub retry_backoff_seconds: i64,
    pub delivered_retention_hours: i64,
    pub dead_letter_retention_hours: i64,
    pub idempotency_retention_hours: i64,
    /// Number of outbox messages published concurrently per workspace.
    pub publish_concurrency: usize,
    pub http_connect_timeout_ms: u64,
    pub http_request_timeout_ms: u64,
    pub http_pool_max_idle_per_host: usize,
    /// Upper bound on retry-backoff jitter (in ms). Retry delay is
    /// `retry_backoff_seconds` plus a uniform random value in
    /// `[0, retry_jitter_ms)`.
    pub retry_jitter_ms: u64,
    /// Consecutive transient failures before the per-publisher circuit
    /// breaker opens. Zero disables the breaker.
    pub circuit_failure_threshold: u32,
    /// Seconds the breaker stays open before probing again.
    pub circuit_cooldown_seconds: i64,
    /// Max partitions leased per worker cycle when dynamic leasing is active.
    pub partition_batch: usize,
    /// Lease duration applied to partition claims.
    pub partition_lease_seconds: i64,
}

impl Default for OutboxWorkerConfig {
    fn default() -> Self {
        Self {
            workspace_ids: None,
            worker_instance_id: Uuid::nil(),
            batch_size: 32,
            lease_seconds: 30,
            max_attempts: 5,
            retry_backoff_seconds: 15,
            delivered_retention_hours: 24,
            dead_letter_retention_hours: 168,
            idempotency_retention_hours: 168,
            publish_concurrency: 8,
            http_connect_timeout_ms: 2_000,
            http_request_timeout_ms: 10_000,
            http_pool_max_idle_per_host: 16,
            retry_jitter_ms: 2_000,
            circuit_failure_threshold: 10,
            circuit_cooldown_seconds: 30,
            partition_batch: 64,
            partition_lease_seconds: 60,
        }
    }
}

/// Report returned by a single worker cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WorkerRunReport {
    pub leased: usize,
    pub delivered: usize,
    pub failed: usize,
    pub dead_lettered: usize,
    pub cleaned_outbox_rows: u64,
    pub cleaned_idempotency_rows: u64,
}
