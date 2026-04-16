use dandori_app_services::AuthContext;
use dandori_app_services::{
    IssueAppService, build_issue_service, handle_mcp_create_issue, handle_mcp_get_issue,
};
use dandori_auth::JwtAuthenticator;
use dandori_contract::{Envelope, ErrorEnvelope};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct McpState {
    service: IssueAppService,
    jwt: JwtAuthenticator,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
struct ToolCallParams {
    name: String,
    token: String,
    #[serde(default)]
    arguments: Value,
}

impl McpState {
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

    pub async fn handle_tool_call(
        &self,
        tool_name: &str,
        token: &str,
        params: Value,
    ) -> Envelope<Value> {
        let auth = match self
            .jwt
            .authenticate_token(&SecretString::from(token.to_owned()))
        {
            Ok(claims) => AuthContext {
                workspace_id: claims.workspace_id.into(),
                actor_id: claims.actor_id,
            },
            Err(error) => {
                let correlation_id = Uuid::now_v7();
                tracing::warn!(
                    correlation_id = %correlation_id,
                    error = ?error,
                    "mcp authentication failed"
                );
                return Envelope::Err {
                    error: ErrorEnvelope {
                        code: "unauthorized".to_owned(),
                        message: "authentication failed".to_owned(),
                        correlation_id: Some(correlation_id),
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
                    correlation_id: Some(Uuid::now_v7()),
                },
            },
        }
    }

    pub async fn handle_json_rpc(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        if request.jsonrpc != "2.0" {
            return JsonRpcResponse::invalid_request(
                request.id,
                "jsonrpc must be exactly '2.0'".to_owned(),
            );
        }

        match request.method.as_str() {
            "initialize" => JsonRpcResponse::ok(
                request.id,
                json!({
                    "protocolVersion": "2026-04-15",
                    "serverInfo": {"name": "dandori-mcp", "version": "0.1.0"},
                    "capabilities": {"tools": {}}
                }),
            ),
            "tools/list" => JsonRpcResponse::ok(request.id, tools_list_payload()),
            "tools/call" => {
                let params: ToolCallParams = match serde_json::from_value(request.params) {
                    Ok(value) => value,
                    Err(error) => {
                        return JsonRpcResponse::invalid_params(
                            request.id,
                            format!("invalid tools/call params: {error}"),
                        );
                    }
                };

                let envelope = self
                    .handle_tool_call(
                        params.name.as_str(),
                        params.token.as_str(),
                        params.arguments,
                    )
                    .await;

                JsonRpcResponse::ok(request.id, json!({ "envelope": envelope }))
            }
            _ => JsonRpcResponse::method_not_found(request.id, request.method),
        }
    }
}

impl JsonRpcResponse {
    #[must_use]
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    #[must_use]
    pub fn invalid_request(id: Value, message: String) -> Self {
        Self::error(id, -32600, message)
    }

    #[must_use]
    pub fn method_not_found(id: Value, method: String) -> Self {
        Self::error(id, -32601, format!("method not found: {method}"))
    }

    #[must_use]
    pub fn invalid_params(id: Value, message: String) -> Self {
        Self::error(id, -32602, message)
    }

    fn error(id: Value, code: i32, message: String) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError { code, message }),
        }
    }
}

fn tools_list_payload() -> Value {
    json!({
        "tools": [
            {
                "name": "issue.create",
                "description": "Create an issue in the authenticated workspace",
                "inputSchema": {
                    "type": "object",
                    "required": ["idempotency_key", "project_id", "title", "priority"],
                    "properties": {
                        "idempotency_key": {"type": "string"},
                        "project_id": {"type": "string", "format": "uuid"},
                        "milestone_id": {"type": ["string", "null"], "format": "uuid"},
                        "title": {"type": "string"},
                        "description": {"type": ["string", "null"]},
                        "priority": {"type": "string", "enum": ["low", "medium", "high", "urgent"]}
                    }
                }
            },
            {
                "name": "issue.get",
                "description": "Get an issue from the authenticated workspace",
                "inputSchema": {
                    "type": "object",
                    "required": ["issue_id"],
                    "properties": {
                        "issue_id": {"type": "string", "format": "uuid"}
                    }
                }
            }
        ]
    })
}
