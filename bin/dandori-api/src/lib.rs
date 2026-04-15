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
use dandori_contract::{CreateIssueRequest, CreateIssueResponse, Envelope, GetIssueResponse};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use serde::Deserialize;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ApiState {
    service: IssueAppService,
    jwt: JwtValidator,
}

#[derive(Clone)]
struct JwtValidator {
    decoding_key: DecodingKey,
    validation: Validation,
}

#[derive(Debug, Clone, Deserialize)]
struct JwtClaims {
    sub: String,
    workspace_id: String,
    exp: usize,
    nbf: usize,
    iss: String,
    aud: String,
}

impl ApiState {
    pub async fn new(database_url: &str, run_migrations: bool) -> Result<Self, anyhow::Error> {
        let service = build_issue_service(database_url, run_migrations)
            .await
            .map_err(anyhow::Error::from)?;
        let jwt = JwtValidator::from_env("dandori-api");
        Ok(Self { service, jwt })
    }

    #[must_use]
    pub fn from_service(service: IssueAppService) -> Self {
        Self {
            service,
            jwt: JwtValidator::from_env("dandori-api"),
        }
    }
}

impl JwtValidator {
    fn from_env(audience: &str) -> Self {
        let secret =
            std::env::var("DANDORI_JWT_SECRET").unwrap_or_else(|_| "dandori-dev-secret".to_owned());
        let issuer =
            std::env::var("DANDORI_JWT_ISSUER").unwrap_or_else(|_| "dandori-local".to_owned());

        let mut validation = Validation::new(Algorithm::HS256);
        validation.set_audience(&[audience]);
        validation.set_issuer(&[issuer]);

        Self {
            decoding_key: DecodingKey::from_secret(secret.as_bytes()),
            validation,
        }
    }

    fn auth_context(&self, headers: &HeaderMap) -> Result<AuthContext, (StatusCode, String)> {
        let token = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or((StatusCode::UNAUTHORIZED, "missing bearer token".to_owned()))?;

        let data = decode::<JwtClaims>(token, &self.decoding_key, &self.validation)
            .map_err(|error| (StatusCode::UNAUTHORIZED, format!("invalid token: {error}")))?;

        let actor_id = Uuid::parse_str(data.claims.sub.as_str()).map_err(|error| {
            (
                StatusCode::UNAUTHORIZED,
                format!("invalid sub claim for actor id: {error}"),
            )
        })?;

        let workspace_id = Uuid::parse_str(data.claims.workspace_id.as_str()).map_err(|error| {
            (
                StatusCode::UNAUTHORIZED,
                format!("invalid workspace_id claim: {error}"),
            )
        })?;

        let _exp = data.claims.exp;
        let _nbf = data.claims.nbf;
        let _iss = data.claims.iss;
        let _aud = data.claims.aud;

        Ok(AuthContext {
            workspace_id: workspace_id.into(),
            actor_id,
        })
    }
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
    Json(request): Json<CreateIssueRequest>,
) -> (StatusCode, Json<Envelope<CreateIssueResponse>>) {
    let auth = match state.jwt.auth_context(&headers) {
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
    let auth = match state.jwt.auth_context(&headers) {
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

impl std::fmt::Debug for JwtValidator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JwtValidator").finish_non_exhaustive()
    }
}
