#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let _banner = dandori_app_services::health_banner();
    Ok(())
}
