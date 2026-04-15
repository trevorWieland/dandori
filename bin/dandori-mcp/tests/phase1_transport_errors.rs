use std::path::PathBuf;
use std::process::Stdio;

use chrono::Utc;
use dandori_api::{ApiState, build_router};
use dandori_app_services::build_issue_service;
use dandori_auth::JwtAuthenticator;
use dandori_contract::{CreateIssueRequest, IssuePriorityDto};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use uuid::Uuid;

const TEST_JWKS: &str = r#"{"keys":[{"kty":"oct","k":"c2VjcmV0","alg":"HS256","kid":"test-key"}]}"#;
const TEST_ISSUER: &str = "https://issuer.example";
const TEST_AUDIENCE: &str = "dandori";

#[derive(Serialize)]
struct Claims {
    sub: String,
    workspace_id: String,
    iss: String,
    aud: String,
    exp: usize,
    nbf: usize,
}

struct McpSession {
    child: Child,
    stdin: ChildStdin,
    stdout: tokio::io::Lines<BufReader<ChildStdout>>,
}

impl McpSession {
    async fn send(&mut self, request: Value) -> Value {
        let payload = serde_json::to_string(&request).expect("serialize request");
        self.stdin
            .write_all(payload.as_bytes())
            .await
            .expect("write mcp request");
        self.stdin.write_all(b"\n").await.expect("write newline");
        self.stdin.flush().await.expect("flush stdin");

        let line = self
            .stdout
            .next_line()
            .await
            .expect("read mcp output")
            .expect("mcp returned response line");
        serde_json::from_str(&line).expect("parse mcp response")
    }

    async fn shutdown(mut self) {
        let _ = self.child.kill().await;
    }
}

#[tokio::test]
async fn rest_and_mcp_wire_paths_have_parity_for_retry_and_error_semantics() {
    let container = Postgres::default().start().await.expect("start postgres");
    let host = container.get_host().await.expect("host");
    let port = container.get_host_port_ipv4(5432).await.expect("port");

    let admin_url = format!("postgres://postgres:postgres@{host}:{port}/postgres");
    let _ = build_issue_service(&admin_url, true)
        .await
        .expect("migrate");

    let admin_pool = PgPool::connect(&admin_url).await.expect("connect admin");

    sqlx::query(
        "DO $$
            BEGIN
                IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'dandori_app') THEN
                    CREATE ROLE dandori_app
                        LOGIN
                        PASSWORD 'dandori_app'
                        NOSUPERUSER
                        NOCREATEDB
                        NOCREATEROLE
                        NOBYPASSRLS;
                END IF;
            END
        $$;",
    )
    .execute(&admin_pool)
    .await
    .expect("create app role");

    sqlx::query("GRANT USAGE ON SCHEMA public TO dandori_app")
        .execute(&admin_pool)
        .await
        .expect("grant schema usage");

    sqlx::query(
        "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO dandori_app",
    )
    .execute(&admin_pool)
    .await
    .expect("grant table perms");

    let workspace_id = Uuid::now_v7();
    let workflow_id = Uuid::now_v7();
    let project_id = Uuid::now_v7();

    sqlx::query("INSERT INTO workspace (id, name) VALUES ($1, 'ws-a')")
        .bind(workspace_id)
        .execute(&admin_pool)
        .await
        .expect("seed workspace");

    sqlx::query(
        "INSERT INTO workflow_version (id, workspace_id, name, version, checksum, states, transitions)
         VALUES ($1, $2, 'default', 1, 'sha256:a', '[]'::jsonb, '[]'::jsonb)",
    )
    .bind(workflow_id)
    .bind(workspace_id)
    .execute(&admin_pool)
    .await
    .expect("seed workflow");

    sqlx::query(
        "INSERT INTO project (id, workspace_id, name, workflow_version_id)
         VALUES ($1, $2, 'project-a', $3)",
    )
    .bind(project_id)
    .bind(workspace_id)
    .bind(workflow_id)
    .execute(&admin_pool)
    .await
    .expect("seed project");

    let app_url = format!("postgres://dandori_app:dandori_app@{host}:{port}/postgres");

    let service = build_issue_service(&app_url, false)
        .await
        .expect("connect app");
    let auth = JwtAuthenticator::from_jwks_json(
        TEST_ISSUER.to_owned(),
        TEST_AUDIENCE.to_owned(),
        TEST_JWKS,
    )
    .expect("build test auth");

    let api_state = ApiState::from_service_with_auth(service, auth);
    let router = build_router(api_state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind api listener");
    let rest_base = format!("http://{}", listener.local_addr().expect("listener addr"));
    let rest_server = tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve api");
    });

    let jwks_path = write_jwks_file();
    let mut mcp = spawn_mcp(&app_url, &jwks_path).await;

    let initialize = mcp
        .send(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        }))
        .await;
    assert!(initialize.get("result").is_some());

    let rest_client = reqwest::Client::new();

    let rest_retry_request = CreateIssueRequest {
        idempotency_key: "rest-wire-retry".to_owned(),
        project_id,
        milestone_id: None,
        title: "Rest wire retry".to_owned(),
        description: Some("same payload".to_owned()),
        priority: IssuePriorityDto::Medium,
    };

    let rest_retry_first = rest_client
        .post(format!("{rest_base}/v1/issues"))
        .bearer_auth(build_token(workspace_id))
        .json(&rest_retry_request)
        .send()
        .await
        .expect("rest retry first");
    let rest_retry_first_envelope: Value = rest_retry_first
        .json()
        .await
        .expect("rest retry first json");
    let rest_retry_first_id = rest_retry_first_envelope
        .get("data")
        .and_then(|data| data.get("issue"))
        .and_then(|issue| issue.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();

    let rest_retry_second = rest_client
        .post(format!("{rest_base}/v1/issues"))
        .bearer_auth(build_token(workspace_id))
        .json(&rest_retry_request)
        .send()
        .await
        .expect("rest retry second");
    let rest_retry_second_envelope: Value = rest_retry_second
        .json()
        .await
        .expect("rest retry second json");
    let rest_retry_replay = rest_retry_second_envelope
        .get("data")
        .and_then(|data| data.get("idempotent_replay"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let rest_retry_second_id = rest_retry_second_envelope
        .get("data")
        .and_then(|data| data.get("issue"))
        .and_then(|issue| issue.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    assert!(rest_retry_replay);
    assert_eq!(rest_retry_first_id, rest_retry_second_id);

    let rest_retry_conflict = rest_client
        .post(format!("{rest_base}/v1/issues"))
        .bearer_auth(build_token(workspace_id))
        .json(&json!({
            "idempotency_key": "rest-wire-retry",
            "project_id": project_id,
            "milestone_id": null,
            "title": "Rest wire retry changed",
            "description": "different payload",
            "priority": "medium"
        }))
        .send()
        .await
        .expect("rest retry conflict");
    assert_eq!(rest_retry_conflict.status(), 409);
    let rest_retry_conflict_envelope: Value = rest_retry_conflict
        .json()
        .await
        .expect("rest retry conflict json");
    let rest_retry_conflict_code = rest_retry_conflict_envelope
        .get("error")
        .and_then(|value| value.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(rest_retry_conflict_code, "duplicate_issue_command");

    let mcp_retry_first = mcp
        .send(json!({
            "jsonrpc": "2.0",
            "id": 20,
            "method": "tools/call",
            "params": {
                "name": "issue.create",
                "token": build_token(workspace_id),
                "arguments": {
                    "idempotency_key": "mcp-wire-retry",
                    "project_id": project_id,
                    "milestone_id": null,
                    "title": "MCP wire retry",
                    "description": "same payload",
                    "priority": "medium"
                }
            }
        }))
        .await;
    let mcp_retry_first_envelope = mcp_retry_first
        .get("result")
        .and_then(|value| value.get("envelope"))
        .cloned()
        .expect("mcp retry first envelope");
    let mcp_retry_first_id = mcp_retry_first_envelope
        .get("data")
        .and_then(|data| data.get("issue"))
        .and_then(|issue| issue.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();

    let mcp_retry_second = mcp
        .send(json!({
            "jsonrpc": "2.0",
            "id": 21,
            "method": "tools/call",
            "params": {
                "name": "issue.create",
                "token": build_token(workspace_id),
                "arguments": {
                    "idempotency_key": "mcp-wire-retry",
                    "project_id": project_id,
                    "milestone_id": null,
                    "title": "MCP wire retry",
                    "description": "same payload",
                    "priority": "medium"
                }
            }
        }))
        .await;
    let mcp_retry_second_envelope = mcp_retry_second
        .get("result")
        .and_then(|value| value.get("envelope"))
        .cloned()
        .expect("mcp retry second envelope");
    let mcp_retry_replay = mcp_retry_second_envelope
        .get("data")
        .and_then(|data| data.get("idempotent_replay"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mcp_retry_second_id = mcp_retry_second_envelope
        .get("data")
        .and_then(|data| data.get("issue"))
        .and_then(|issue| issue.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    assert!(mcp_retry_replay);
    assert_eq!(mcp_retry_first_id, mcp_retry_second_id);

    let mcp_retry_conflict = mcp
        .send(json!({
            "jsonrpc": "2.0",
            "id": 22,
            "method": "tools/call",
            "params": {
                "name": "issue.create",
                "token": build_token(workspace_id),
                "arguments": {
                    "idempotency_key": "mcp-wire-retry",
                    "project_id": project_id,
                    "milestone_id": null,
                    "title": "MCP wire retry changed",
                    "description": "different payload",
                    "priority": "medium"
                }
            }
        }))
        .await;
    let mcp_retry_conflict_envelope = mcp_retry_conflict
        .get("result")
        .and_then(|value| value.get("envelope"))
        .cloned()
        .expect("mcp retry conflict envelope");
    let mcp_retry_conflict_code = mcp_retry_conflict_envelope
        .get("error")
        .and_then(|value| value.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(mcp_retry_conflict_code, "duplicate_issue_command");

    mcp.shutdown().await;
    rest_server.abort();
}

async fn spawn_mcp(database_url: &str, jwks_path: &PathBuf) -> McpSession {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dandori-mcp"))
        .env("DANDORI_DATABASE_URL", database_url)
        .env("DANDORI_OIDC_ISSUER", TEST_ISSUER)
        .env("DANDORI_OIDC_AUDIENCE", TEST_AUDIENCE)
        .env("DANDORI_OIDC_ALLOWED_ALGS", "HS256")
        .env("DANDORI_OIDC_JWKS_PATH", jwks_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mcp process");

    let stdin = child.stdin.take().expect("mcp stdin");
    let stdout = child.stdout.take().expect("mcp stdout");

    McpSession {
        child,
        stdin,
        stdout: BufReader::new(stdout).lines(),
    }
}

fn write_jwks_file() -> PathBuf {
    let path = std::env::temp_dir().join(format!("dandori-jwks-{}.json", Uuid::now_v7()));
    std::fs::write(&path, TEST_JWKS).expect("write jwks fixture");
    path
}

fn build_token(workspace_id: Uuid) -> String {
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some("test-key".to_owned());

    let now = Utc::now().timestamp() as usize;
    let claims = Claims {
        sub: Uuid::now_v7().to_string(),
        workspace_id: workspace_id.to_string(),
        iss: TEST_ISSUER.to_owned(),
        aud: TEST_AUDIENCE.to_owned(),
        exp: now + 3_600,
        nbf: now.saturating_sub(30),
    };

    encode(&header, &claims, &EncodingKey::from_secret(b"secret")).expect("encode test token")
}
