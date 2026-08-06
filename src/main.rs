use domru_mts::{app::build_router, config::AppConfig, state::AppState, storage::SecretStore};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = AppConfig::from_env()?;
    init_tracing();
    let secret_store = SecretStore::open_from_env(&config.data_dir).await?;
    let state = AppState::with_secret_store(secret_store);

    let listener = TcpListener::bind(config.bind).await?;
    tracing::info!(bind = %config.bind, "server listening");

    axum::serve(listener, build_router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .init();
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "failed to listen for Ctrl-C");
    }
}
