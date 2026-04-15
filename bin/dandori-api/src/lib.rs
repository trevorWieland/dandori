use std::sync::Arc;

use axum::{
    Json, Router,
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
        let token = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or((StatusCode::UNAUTHORIZED, "missing bearer token".to_owned()))?;

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
    (StatusCode::UNAUTHORIZED, error.to_string())
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
    Json(request): Json<dandori_contract::CreateIssueRequest>,
) -> (StatusCode, Json<Envelope<CreateIssueResponse>>) {
    let auth = match state.auth_context(&headers) {
        Ok(auth) => auth,
        Err((status, message)) => {
            return (
                status,
                Json(Envelope::Err {
                    error: dandori_contract::ErrorEnvelope {
                        code: "unauthorized".to_owned(),
                        message,
                    },
                }),
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
    Path(issue_id): Path<Uuid>,
) -> (StatusCode, Json<Envelope<GetIssueResponse>>) {
    let auth = match state.auth_context(&headers) {
        Ok(auth) => auth,
        Err((status, message)) => {
            return (
                status,
                Json(Envelope::Err {
                    error: dandori_contract::ErrorEnvelope {
                        code: "unauthorized".to_owned(),
                        message,
                    },
                }),
            );
        }
    };

    let (status_code, envelope) = handle_rest_get_issue(&state.service, &auth, issue_id).await;
    (
        StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(envelope),
    )
}
