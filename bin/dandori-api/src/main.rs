use anyhow::Context;
use std::net::SocketAddr;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let database_url =
        std::env::var("DANDORI_DATABASE_URL").context("DANDORI_DATABASE_URL is required")?;
    let bind = std::env::var("DANDORI_API_BIND").unwrap_or_else(|_| "127.0.0.1:3000".to_owned());
    let run_migrations = std::env::var("DANDORI_RUN_MIGRATIONS")
        .ok()
        .is_some_and(|value| value.eq_ignore_ascii_case("true"));

    let state = dandori_api::ApiState::new(&database_url, run_migrations).await?;
    let router = dandori_api::build_router(state);

    let listener = tokio::net::TcpListener::bind(bind.as_str()).await?;
    let _addr: SocketAddr = listener.local_addr()?;
    axum::serve(listener, router).await?;
    Ok(())
}
