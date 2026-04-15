mod adapters;
mod service;

pub use adapters::{
    handle_mcp_create_issue, handle_mcp_get_issue, handle_rest_create_issue, handle_rest_get_issue,
};
pub use dandori_domain::AuthContext;

pub use service::{
    AppServiceError, ErrorKind, IssueAppService, build_issue_service, map_error_to_transport,
    validation_error,
};

#[must_use]
pub fn health_banner() -> &'static str {
    "dandori-app-services"
}
