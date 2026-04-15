#[derive(Debug, thiserror::Error)]
pub enum AppServiceError {
    #[error("not implemented")]
    NotImplemented,
}

#[must_use]
pub fn health_banner() -> &'static str {
    "dandori-app-services"
}
