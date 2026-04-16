//! Outbox worker service.
//!
//! Responsibilities:
//! - Claim partitions (workspace shards) via dynamic DB leasing so peer
//!   workers never double-process a tenant.
//! - Lease a batch of outbox messages per claimed workspace.
//! - Route each message through a typed [`EventType`](dandori_domain::EventType)
//!   match and publish via the injected [`OutboxPublisher`].
//! - Mark each row delivered, retried, or dead-lettered based on the
//!   publish outcome and its [`PublishErrorKind`].
//!
//! The module is split so each concern stays readable and stays below the
//! repo's per-file line budget.

mod config;
mod publish;
mod registry;
mod service;

pub use config::{OutboxWorkerConfig, WorkerRunReport};
pub use publish::{HttpOutboxPublisher, OutboxPublisher, PublishError, PublishErrorKind};
pub use service::{OutboxWorkerService, build_outbox_worker_service};
