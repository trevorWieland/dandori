use std::path::PathBuf;
use std::process::Stdio;

use chrono::Utc;
use dandori_api::{ApiState, build_router};
use dandori_app_services::build_issue_service;
use dandori_auth::JwtAuthenticator;
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
async fn rest_and_mcp_wire_paths_have_parity_for_malformed_and_auth_errors() {
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

    let rest_invalid_payload = rest_client
        .post(format!("{rest_base}/v1/issues"))
        .bearer_auth(build_token(workspace_id))
        .header("content-type", "application/json")
        .body("{")
        .send()
        .await
        .expect("rest invalid payload request");
    assert_eq!(rest_invalid_payload.status(), 422);
    let rest_invalid_payload_envelope: Value = rest_invalid_payload
        .json()
        .await
        .expect("rest invalid payload json");

    let mcp_invalid_payload = mcp
        .send(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "issue.create",
                "token": build_token(workspace_id),
                "arguments": {
                    "idempotency_key": "invalid-payload",
                    "project_id": "not-a-uuid",
                    "title": "bad payload",
                    "priority": "medium"
                }
            }
        }))
        .await;
    let mcp_invalid_payload_envelope = mcp_invalid_payload
        .get("result")
        .and_then(|value| value.get("envelope"))
        .cloned()
        .expect("mcp invalid payload envelope");

    let rest_invalid_payload_code = rest_invalid_payload_envelope
        .get("error")
        .and_then(|value| value.get("code"))
        .and_then(Value::as_str)
        .expect("rest invalid payload code");
    let mcp_invalid_payload_code = mcp_invalid_payload_envelope
        .get("error")
        .and_then(|value| value.get("code"))
        .and_then(Value::as_str)
        .expect("mcp invalid payload code");
    assert_eq!(rest_invalid_payload_code, "invalid_payload");
    assert_eq!(mcp_invalid_payload_code, "invalid_payload");

    let rest_invalid_issue_id = rest_client
        .get(format!("{rest_base}/v1/issues/not-a-uuid"))
        .bearer_auth(build_token(workspace_id))
        .send()
        .await
        .expect("rest invalid issue id request");
    assert_eq!(rest_invalid_issue_id.status(), 422);
    let rest_invalid_issue_id_envelope: Value = rest_invalid_issue_id
        .json()
        .await
        .expect("rest invalid issue id json");

    let mcp_invalid_issue_id = mcp
        .send(json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "issue.get",
                "token": build_token(workspace_id),
                "arguments": {"issue_id": "not-a-uuid"}
            }
        }))
        .await;
    let mcp_invalid_issue_id_envelope = mcp_invalid_issue_id
        .get("result")
        .and_then(|value| value.get("envelope"))
        .cloned()
        .expect("mcp invalid issue id envelope");

    let rest_invalid_issue_code = rest_invalid_issue_id_envelope
        .get("error")
        .and_then(|value| value.get("code"))
        .and_then(Value::as_str)
        .expect("rest invalid issue code");
    let mcp_invalid_issue_code = mcp_invalid_issue_id_envelope
        .get("error")
        .and_then(|value| value.get("code"))
        .and_then(Value::as_str)
        .expect("mcp invalid issue code");
    assert_eq!(rest_invalid_issue_code, "invalid_issue_id");
    assert_eq!(mcp_invalid_issue_code, "invalid_issue_id");

    let rest_unauthorized = rest_client
        .post(format!("{rest_base}/v1/issues"))
        .json(&json!({
            "idempotency_key": "unauthorized",
            "project_id": project_id,
            "milestone_id": null,
            "title": "unauthorized",
            "description": null,
            "priority": "medium"
        }))
        .send()
        .await
        .expect("rest unauthorized request");
    assert_eq!(rest_unauthorized.status(), 401);
    let rest_unauthorized_envelope: Value = rest_unauthorized
        .json()
        .await
        .expect("rest unauthorized json");

    let mcp_unauthorized = mcp
        .send(json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "issue.create",
                "token": "bad.token.value",
                "arguments": {
                    "idempotency_key": "unauthorized",
                    "project_id": project_id,
                    "title": "unauthorized",
                    "priority": "medium"
                }
            }
        }))
        .await;
    let mcp_unauthorized_envelope = mcp_unauthorized
        .get("result")
        .and_then(|value| value.get("envelope"))
        .cloned()
        .expect("mcp unauthorized envelope");

    let rest_unauthorized_code = rest_unauthorized_envelope
        .get("error")
        .and_then(|value| value.get("code"))
        .and_then(Value::as_str)
        .expect("rest unauthorized code");
    let mcp_unauthorized_code = mcp_unauthorized_envelope
        .get("error")
        .and_then(|value| value.get("code"))
        .and_then(Value::as_str)
        .expect("mcp unauthorized code");
    assert_eq!(rest_unauthorized_code, "unauthorized");
    assert_eq!(mcp_unauthorized_code, "unauthorized");

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
