//! Foundation observability primitives: structured tracing init, correlation
//! ids propagated across request/outbox boundaries, and typed metric
//! helpers. Everything is intentionally lightweight so every layer (transport,
//! app-services, store) can depend on it without pulling in a heavy exporter
//! runtime.

use std::sync::OnceLock;

use thiserror::Error;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

pub mod correlation;
pub mod metrics;

pub use correlation::{CORRELATION_ID_HEADER, CorrelationId};

/// Legacy id wrapper kept for backwards compatibility with existing call
/// sites. New code should prefer [`correlation::CorrelationId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct EntityId(pub uuid::Uuid);

impl EntityId {
    #[must_use]
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7())
    }
}

impl Default for EntityId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Error)]
pub enum ObservabilityError {
    #[error("tracing subscriber already installed")]
    AlreadyInstalled,
    #[error("failed to build env filter: {0}")]
    FilterBuild(String),
}

/// Tracing output format. JSON for production shipping, Pretty for local
/// development. Chosen by caller rather than env-sniffed so tests can
/// force a deterministic format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TracingFormat {
    Json,
    Pretty,
}

#[derive(Debug, Clone)]
pub struct TracingConfig {
    pub format: TracingFormat,
    pub default_filter: String,
    pub filter_env: &'static str,
    pub service_name: &'static str,
}

impl TracingConfig {
    #[must_use]
    pub fn for_service(service_name: &'static str) -> Self {
        Self {
            format: TracingFormat::Json,
            default_filter: "info,dandori=debug".to_owned(),
            filter_env: "RUST_LOG",
            service_name,
        }
    }

    #[must_use]
    pub fn pretty(mut self) -> Self {
        self.format = TracingFormat::Pretty;
        self
    }
}

static INSTALLED: OnceLock<()> = OnceLock::new();

/// Install the shared tracing subscriber. Safe to call multiple times — only
/// the first caller wins; subsequent calls are a no-op so that test
/// harnesses and top-level bins can both call it without racing.
pub fn init_tracing(cfg: TracingConfig) -> Result<(), ObservabilityError> {
    let mut first_install = false;
    INSTALLED.get_or_init(|| {
        first_install = true;
    });
    if !first_install {
        return Ok(());
    }

    let filter = EnvFilter::try_from_env(cfg.filter_env)
        .or_else(|_| EnvFilter::try_new(cfg.default_filter.as_str()))
        .map_err(|e| ObservabilityError::FilterBuild(e.to_string()))?;

    let service = cfg.service_name;
    let registry = tracing_subscriber::registry().with(filter);

    match cfg.format {
        TracingFormat::Json => {
            registry
                .with(
                    fmt::layer()
                        .json()
                        .with_target(true)
                        .with_current_span(true)
                        .with_span_list(false),
                )
                .try_init()
                .map_err(|_| ObservabilityError::AlreadyInstalled)?;
        }
        TracingFormat::Pretty => {
            registry
                .with(fmt::layer().pretty().with_target(true))
                .try_init()
                .map_err(|_| ObservabilityError::AlreadyInstalled)?;
        }
    }

    tracing::info!(service = service, "tracing initialized");
    Ok(())
}
