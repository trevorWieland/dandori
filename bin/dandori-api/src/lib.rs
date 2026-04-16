use std::sync::Arc;

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use dandori_app_services::AuthContext;
use dandori_app_services::{
    IssueAppService, build_issue_service, handle_rest_create_issue, handle_rest_get_issue,
};
use dandori_auth::{AuthError, JwtAuthenticator};
use dandori_contract::{CreateIssueResponse, Envelope, GetIssueResponse};
use secrecy::SecretString;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ApiState {
    service: IssueAppService,
    jwt: JwtAuthenticator,
}

impl ApiState {
    pub async fn new(database_url: &str, run_migrations: bool) -> Result<Self, anyhow::Error> {
        let service = build_issue_service(database_url, run_migrations)
            .await
            .map_err(anyhow::Error::from)?;
        let jwt = JwtAuthenticator::from_env().await?;
        Ok(Self { service, jwt })
    }

    pub async fn from_service(service: IssueAppService) -> Result<Self, anyhow::Error> {
        let jwt = JwtAuthenticator::from_env().await?;
        Ok(Self { service, jwt })
    }

    #[must_use]
    pub fn from_service_with_auth(service: IssueAppService, jwt: JwtAuthenticator) -> Self {
        Self { service, jwt }
    }
}

impl ApiState {
    fn auth_context(&self, headers: &HeaderMap) -> Result<AuthContext, (StatusCode, String)> {
        const UNAUTHORIZED_MESSAGE: &str = "authentication failed";

        let token = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or_else(|| {
                tracing::warn!("request missing bearer token");
                (StatusCode::UNAUTHORIZED, UNAUTHORIZED_MESSAGE.to_owned())
            })?;

        let claims = self
            .jwt
            .authenticate_token(&SecretString::from(token.to_owned()))
            .map_err(map_auth_error)?;

        Ok(AuthContext {
            workspace_id: claims.workspace_id.into(),
            actor_id: claims.actor_id,
        })
    }
}

fn map_auth_error(error: AuthError) -> (StatusCode, String) {
    tracing::warn!(error = ?error, "request authentication failed");
    (StatusCode::UNAUTHORIZED, "authentication failed".to_owned())
}

pub fn build_router(state: ApiState) -> Router {
    Router::new()
        .route("/v1/issues", post(rest_create_issue))
        .route("/v1/issues/{issue_id}", get(rest_get_issue))
        .with_state(Arc::new(state))
}

async fn rest_create_issue(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, Json<Envelope<CreateIssueResponse>>) {
    let auth = match state.auth_context(&headers) {
        Ok(auth) => auth,
        Err((status, message)) => {
            return transport_err(status, "unauthorized", message);
        }
    };

    let request = match serde_json::from_slice::<dandori_contract::CreateIssueRequest>(&body) {
        Ok(request) => request,
        Err(error) => {
            return transport_err(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_payload",
                format!("invalid request payload: {error}"),
            );
        }
    };

    let (status_code, envelope) = handle_rest_create_issue(&state.service, &auth, request).await;
    (
        StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(envelope),
    )
}

async fn rest_get_issue(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(issue_id): Path<String>,
) -> (StatusCode, Json<Envelope<GetIssueResponse>>) {
    let auth = match state.auth_context(&headers) {
        Ok(auth) => auth,
        Err((status, message)) => {
            return transport_err(status, "unauthorized", message);
        }
    };

    let issue_id = match Uuid::parse_str(issue_id.as_str()) {
        Ok(issue_id) => issue_id,
        Err(error) => {
            return transport_err(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_issue_id",
                format!("issue_id must be a valid UUID: {error}"),
            );
        }
    };

    let (status_code, envelope) = handle_rest_get_issue(&state.service, &auth, issue_id).await;
    (
        StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(envelope),
    )
}

fn transport_err<T>(
    status: StatusCode,
    code: &str,
    message: String,
) -> (StatusCode, Json<Envelope<T>>) {
    (
        status,
        Json(Envelope::Err {
            error: dandori_contract::ErrorEnvelope {
                code: code.to_owned(),
                message,
            },
        }),
    )
}
