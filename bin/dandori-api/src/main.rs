use std::net::SocketAddr;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let database_url = std::env::var("DANDORI_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5432/postgres".to_owned());
    let bind = std::env::var("DANDORI_API_BIND").unwrap_or_else(|_| "127.0.0.1:3000".to_owned());

    let state = dandori_api::ApiState::new(&database_url, true).await?;
    let router = dandori_api::build_router(state);

    let listener = tokio::net::TcpListener::bind(bind.as_str()).await?;
    let _addr: SocketAddr = listener.local_addr()?;
    axum::serve(listener, router).await?;
    Ok(())
}
