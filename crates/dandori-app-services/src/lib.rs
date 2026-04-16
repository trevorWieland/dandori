mod adapters;
mod service;
mod worker;

pub use adapters::{
    handle_mcp_create_issue, handle_mcp_get_issue, handle_rest_create_issue, handle_rest_get_issue,
};
pub use dandori_domain::AuthContext;

pub use dandori_store::ShardBucketRange;
pub use service::{
    AppServiceError, ErrorKind, IssueAppService, build_issue_service, map_error_to_transport,
    validation_error,
};
pub use worker::{
    HttpOutboxPublisher, OutboxPublisher, OutboxWorkerConfig, OutboxWorkerService, PublishError,
    PublishErrorKind, WorkerRunReport, build_outbox_worker_service,
};

#[must_use]
pub fn health_banner() -> &'static str {
    "dandori-app-services"
}
