use anyhow::Context as _;
use metamatch_backend::{app::create_app, config::load_config_from_env, error::ErrorKind};
use std::net::SocketAddr;

fn main() -> anyhow::Result<()> {
    load_dotenv()?;
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "metamatch_backend=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("create Tokio runtime")?
        .block_on(run())
}

fn load_dotenv() -> anyhow::Result<()> {
    match dotenvy::dotenv() {
        Ok(_) => Ok(()),
        Err(error) if error.not_found() => Ok(()),
        Err(error) => Err(anyhow::Error::new(error)
            .context(ErrorKind::InvalidConfig)
            .context("loading .env")),
    }
}

async fn run() -> anyhow::Result<()> {
    let config = load_config_from_env()?;
    let address: SocketAddr = format!("{}:{}", config.host, config.port).parse()?;
    let app = create_app(config);
    let listener = tokio::net::TcpListener::bind(address).await?;
    println!("MetaMatch Rust backend listening on http://{address}");
    axum::serve(listener, app.router.clone())
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
