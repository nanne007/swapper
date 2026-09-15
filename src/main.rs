use anyhow::Context as _;
use metamatch_backend::{app::create_app, config::load_config_file, error::ErrorKind};
use std::net::SocketAddr;

fn main() -> anyhow::Result<()> {
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

async fn run() -> anyhow::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let path = args.next().unwrap_or_else(|| "config.json".into());
    if args.next().is_some() {
        anyhow::bail!(ErrorKind::InvalidConfig);
    }
    let config = load_config_file(path)?;
    let address = SocketAddr::new(config.host, config.port);
    let app = create_app(config).await?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    println!("MetaMatch Rust backend listening on http://{address}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
