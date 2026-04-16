use anyhow::Context;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let database_url =
        std::env::var("DANDORI_DATABASE_URL").context("DANDORI_DATABASE_URL is required")?;
    dandori_store::migrate_database(&database_url).await?;
    Ok(())
}
