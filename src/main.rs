use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use domru_mts::{
    app::build_router,
    auth::AuthCoordinator,
    calls::runtime::SipCallRuntime,
    config::AppConfig,
    provider::mts::{
        MtsAuthApi, MtsClient, TokenSource,
        call::{MtsCallActionProvider, MtsCallOpenProvider},
        device::MtsDeviceDescriptor,
        fcm::{FcmSecretStore, FirebaseConfig, IncomingCallBootstrap, VendoredFcmTransport},
        sms::MtsLegacySmsAuth,
        sso::MtsSsoTransport,
    },
    service::{
        calls::CallService,
        push::{IncomingCallHandler, PushSupervisor, PushSupervisorConfig},
    },
    state::AppState,
    storage::SecretStore,
};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

const SUPERVISOR_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(20);

struct RuntimeIncomingCallHandler {
    runtime: Arc<SipCallRuntime>,
}

#[async_trait]
impl IncomingCallHandler for RuntimeIncomingCallHandler {
    async fn handle(&self, bootstrap: IncomingCallBootstrap, cancellation: CancellationToken) {
        if let Err(error) = self.runtime.handle_bootstrap(bootstrap, cancellation).await {
            tracing::warn!(%error, "incoming call handling stopped");
        }
    }
}

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
    let legacy_sms_auth = Arc::new(MtsLegacySmsAuth::new()?);

    let call_runtime = Arc::new(SipCallRuntime::production(Arc::new(
        MtsCallActionProvider::new((*mts_client).clone()),
    )));
    let call_service = Arc::new(CallService::new(
        call_runtime.clone(),
        Arc::new(MtsCallOpenProvider::new((*mts_client).clone())),
    ));
    call_runtime.attach_service(Arc::downgrade(&call_service))?;

    let push_store: Arc<dyn FcmSecretStore> = Arc::new(secret_store.clone());
    let push_supervisor = PushSupervisor::new(
        auth_coordinator.clone(),
        mts_client.clone(),
        Arc::new(RuntimeIncomingCallHandler {
            runtime: call_runtime,
        }),
        Arc::new(VendoredFcmTransport::new()?),
        push_store,
        FirebaseConfig::vdome(),
        PushSupervisorConfig::default(),
    );
    let state = AppState::with_dependencies(secret_store, auth_coordinator, mts_client)
        .with_legacy_sms_auth(legacy_sms_auth)
        .with_call_service(call_service);
    let live_sessions = state.live_session_manager().cloned();

    let listener = TcpListener::bind(config.bind).await?;
    tracing::info!(bind = %config.bind, "server listening");

    let cancellation = CancellationToken::new();
    let supervisor_cancellation = cancellation.clone();
    let mut supervisor_task = tokio::spawn(async move {
        if let Err(error) = push_supervisor.run(supervisor_cancellation).await {
            tracing::error!(%error, "push supervisor stopped");
        }
    });

    let shutdown_cancellation = cancellation.clone();
    let server_result = axum::serve(listener, build_router(state))
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            shutdown_cancellation.cancel();
        })
        .await;

    cancellation.cancel();
    if let Some(manager) = live_sessions {
        manager.stop_all().await;
    }
    if tokio::time::timeout(SUPERVISOR_SHUTDOWN_TIMEOUT, &mut supervisor_task)
        .await
        .is_err()
    {
        tracing::warn!("push supervisor did not stop gracefully; aborting task");
        supervisor_task.abort();
        let _ = supervisor_task.await;
    }
    server_result?;

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
