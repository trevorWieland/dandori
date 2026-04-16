//! Thin tracing-based counter/histogram helpers. We intentionally avoid
//! pulling a full `metrics` crate runtime here — the workload is small and a
//! pure-tracing approach keeps the dependency footprint tight while still
//! letting operators build dashboards off `metric=... value=...` log fields.

use tracing::info;

/// Increment a counter metric. Emits a structured log line at `INFO` that
/// a log-collector (Vector/Loki/ELK) can pick up and route to Prometheus.
pub fn increment_counter(name: &'static str, value: u64) {
    info!(metric = "counter", metric_name = name, value);
}

/// Observe a histogram value. Millisecond-granularity durations should be
/// passed as `u64` millis; callers are responsible for their own units.
pub fn observe_histogram(name: &'static str, value: f64) {
    info!(metric = "histogram", metric_name = name, value);
}

/// Record a gauge (point-in-time) value.
pub fn set_gauge(name: &'static str, value: f64) {
    info!(metric = "gauge", metric_name = name, value);
}

pub mod names {
    //! Conventional metric name constants. Using constants keeps dashboards
    //! and alerts stable across refactors.

    pub const WORKER_TENANT_FAILURES: &str = "dandori_worker_tenant_failures_total";
    pub const WORKER_TENANT_DURATION_MS: &str = "dandori_worker_tenant_duration_ms";
    pub const WORKER_OUTBOX_DEAD_LETTER: &str = "dandori_worker_outbox_dead_letter_total";
    pub const STORE_IDEMPOTENCY_REPLAY: &str = "dandori_store_idempotency_replay_total";
    pub const API_AUTHZ_DENIED: &str = "dandori_api_authz_denied_total";
}
