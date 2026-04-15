use dandori_contract::{
    CreateIssueRequest, CreateIssueResponse, Envelope, ErrorEnvelope, GetIssueResponse,
};
use dandori_domain::AuthContext;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    AppServiceError, ErrorKind, IssueAppService, map_error_to_transport, validation_error,
};

pub async fn handle_rest_create_issue(
    service: &IssueAppService,
    auth: &AuthContext,
    request: CreateIssueRequest,
) -> (u16, Envelope<CreateIssueResponse>) {
    match service.create_issue(auth, request).await {
        Ok(response) => (201, Envelope::Ok { data: response }),
        Err(error) => map_transport_error(error),
    }
}

pub async fn handle_rest_get_issue(
    service: &IssueAppService,
    auth: &AuthContext,
    issue_id: Uuid,
) -> (u16, Envelope<GetIssueResponse>) {
    match service.get_issue(auth, issue_id).await {
        Ok(response) => (200, Envelope::Ok { data: response }),
        Err(error) => map_transport_error(error),
    }
}

pub async fn handle_mcp_create_issue(
    service: &IssueAppService,
    auth: &AuthContext,
    params: Value,
) -> Envelope<Value> {
    let request: CreateIssueRequest = match serde_json::from_value(params) {
        Ok(request) => request,
        Err(error) => {
            return Envelope::Err {
                error: map_error_to_transport(validation_error(
                    "invalid_mcp_payload",
                    format!("invalid MCP create_issue payload: {error}"),
                )),
            };
        }
    };

    let (_, envelope) = handle_rest_create_issue(service, auth, request).await;
    match envelope {
        Envelope::Ok { data } => Envelope::Ok { data: json!(data) },
        Envelope::Err { error } => Envelope::Err { error },
    }
}

pub async fn handle_mcp_get_issue(
    service: &IssueAppService,
    auth: &AuthContext,
    params: Value,
) -> Envelope<Value> {
    let issue_id = params
        .get("issue_id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            validation_error(
                "invalid_mcp_payload",
                "issue_id is required in MCP get_issue payload".to_owned(),
            )
        })
        .and_then(|value| {
            Uuid::parse_str(value).map_err(|error| {
                validation_error(
                    "invalid_issue_id",
                    format!("issue_id must be a valid UUID: {error}"),
                )
            })
        });

    let issue_id = match issue_id {
        Ok(value) => value,
        Err(error) => {
            return Envelope::Err {
                error: map_error_to_transport(error),
            };
        }
    };

    let (_, envelope) = handle_rest_get_issue(service, auth, issue_id).await;
    match envelope {
        Envelope::Ok { data } => Envelope::Ok { data: json!(data) },
        Envelope::Err { error } => Envelope::Err { error },
    }
}

fn map_transport_error<T>(error: AppServiceError) -> (u16, Envelope<T>) {
    let status = match error.kind {
        ErrorKind::Validation => 422,
        ErrorKind::Precondition => 422,
        ErrorKind::Conflict => 409,
        ErrorKind::NotFound => 404,
        ErrorKind::Authz => 403,
        ErrorKind::TenantBoundary => 403,
        ErrorKind::Infrastructure => 500,
    };

    (
        status,
        Envelope::Err {
            error: ErrorEnvelope {
                code: error.code.to_owned(),
                message: error.message,
            },
        },
    )
}
