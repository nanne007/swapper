use metamatch_backend::{app::create_app, config::load_config_from_env};
use std::net::SocketAddr;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = load_config_from_env()?;
    let address: SocketAddr = format!("{}:{}", config.host, config.port).parse()?;
    let app = create_app(config);
    let listener = tokio::net::TcpListener::bind(address).await?;
    println!("MetaMatch Rust backend listening on http://{address}");
    axum::serve(listener, app.router.clone())
        .with_graceful_shutdown(shutdown_signal(app))
        .await?;
    Ok(())
}

async fn shutdown_signal(app: metamatch_backend::app::App) {
    let _ = tokio::signal::ctrl_c().await;
    app.close().await;
}
