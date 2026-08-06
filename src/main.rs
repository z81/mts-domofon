use std::sync::Arc;

use domru_mts::{
    app::build_router,
    auth::AuthCoordinator,
    config::AppConfig,
    provider::mts::{
        MtsAuthApi, MtsClient, TokenSource, device::MtsDeviceDescriptor, sso::MtsSsoTransport,
    },
    state::AppState,
    storage::SecretStore,
};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = AppConfig::from_env()?;
    init_tracing();
    let secret_store = SecretStore::open_from_env(&config.data_dir).await?;
    let device_identity = secret_store.load_or_create_device().await?;
    let device_descriptor = MtsDeviceDescriptor::from_identity(&device_identity).encode()?;
    let sso_transport = Arc::new(MtsSsoTransport::new(device_descriptor));
    let provider_auth = Arc::new(MtsAuthApi::new(&config)?);
    let auth_coordinator =
        AuthCoordinator::restore(secret_store.clone(), sso_transport, provider_auth).await?;
    let token_source: Arc<dyn TokenSource> = auth_coordinator.clone();
    let mts_client = Arc::new(MtsClient::new(&config, token_source)?);
    let state = AppState::with_dependencies(secret_store, auth_coordinator, mts_client);

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
