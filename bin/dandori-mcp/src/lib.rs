use dandori_app_services::AuthContext;
use dandori_app_services::{
    IssueAppService, build_issue_service, handle_mcp_create_issue, handle_mcp_get_issue,
};
use dandori_contract::{Envelope, ErrorEnvelope};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct McpState {
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

impl McpState {
    pub async fn new(database_url: &str, run_migrations: bool) -> Result<Self, anyhow::Error> {
        let service = build_issue_service(database_url, run_migrations)
            .await
            .map_err(anyhow::Error::from)?;
        Ok(Self {
            service,
            jwt: JwtValidator::from_env("dandori-mcp"),
        })
    }

    #[must_use]
    pub fn from_service(service: IssueAppService) -> Self {
        Self {
            service,
            jwt: JwtValidator::from_env("dandori-mcp"),
        }
    }

    pub async fn handle_tool_call(
        &self,
        tool_name: &str,
        token: &str,
        params: Value,
    ) -> Envelope<Value> {
        let auth = match self.jwt.auth_context(token) {
            Ok(auth) => auth,
            Err(message) => {
                return Envelope::Err {
                    error: ErrorEnvelope {
                        code: "unauthorized".to_owned(),
                        message,
                    },
                };
            }
        };

        match tool_name {
            "issue.create" => handle_mcp_create_issue(&self.service, &auth, params).await,
            "issue.get" => handle_mcp_get_issue(&self.service, &auth, params).await,
            _ => Envelope::Err {
                error: ErrorEnvelope {
                    code: "unknown_tool".to_owned(),
                    message: format!("unknown MCP tool '{tool_name}'"),
                },
            },
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

    fn auth_context(&self, token: &str) -> Result<AuthContext, String> {
        let data = decode::<JwtClaims>(token, &self.decoding_key, &self.validation)
            .map_err(|error| format!("invalid token: {error}"))?;

        let actor_id = Uuid::parse_str(data.claims.sub.as_str())
            .map_err(|error| format!("invalid sub claim for actor id: {error}"))?;
        let workspace_id = Uuid::parse_str(data.claims.workspace_id.as_str())
            .map_err(|error| format!("invalid workspace_id claim: {error}"))?;

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

impl std::fmt::Debug for JwtValidator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JwtValidator").finish_non_exhaustive()
    }
}
