use anyhow::Context;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let database_url = std::env::var("DANDORI_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5432/postgres".to_owned());

    let state = dandori_mcp::McpState::new(&database_url, true).await?;

    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<dandori_mcp::JsonRpcRequest>(line.as_str()) {
            Ok(request) => state.handle_json_rpc(request).await,
            Err(error) => dandori_mcp::JsonRpcResponse::invalid_request(
                serde_json::Value::Null,
                format!("invalid JSON-RPC request: {error}"),
            ),
        };

        let payload = serde_json::to_vec(&response).context("serialize json-rpc response")?;
        stdout.write_all(&payload).await?;
        stdout.write_all(b"\n").await?;
        stdout.flush().await?;
    }

    Ok(())
}
