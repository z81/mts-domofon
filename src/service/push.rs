use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use secrecy::SecretString;
use tokio::{sync::mpsc, task::JoinSet};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use crate::{
    auth::AuthCoordinator,
    domain::auth::SessionStatus,
    error::AppError,
    provider::mts::{
        MtsClient,
        fcm::{
            FcmBootstrapService, FcmSecretState, FcmSecretStore, FcmServiceError, FcmTransport,
            FirebaseConfig, IncomingCallBootstrap,
        },
    },
    storage::SecretStore,
};

const MAX_PENDING_BOOTSTRAPS: usize = 8;
const MAX_SUPERVISOR_DELAY: Duration = Duration::from_secs(5 * 60);
const HANDLER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);

#[async_trait]
pub trait PushAuthState: Send + Sync {
    async fn is_authenticated(&self) -> bool;
}

#[async_trait]
impl PushAuthState for AuthCoordinator {
    async fn is_authenticated(&self) -> bool {
        self.session().await.status == SessionStatus::Active
    }
}

#[async_trait]
pub trait PushRegistrationClient: Send + Sync {
    async fn current_user_id(&self) -> Result<String, AppError>;

    async fn register_fcm_device_token(
        &self,
        user_id: &str,
        token: &SecretString,
    ) -> Result<(), AppError>;
}

#[async_trait]
impl PushRegistrationClient for MtsClient {
    async fn current_user_id(&self) -> Result<String, AppError> {
        MtsClient::current_user_id(self).await
    }

    async fn register_fcm_device_token(
        &self,
        user_id: &str,
        token: &SecretString,
    ) -> Result<(), AppError> {
        MtsClient::register_fcm_device_token(self, user_id, token).await
    }
}

#[async_trait]
pub trait IncomingCallHandler: Send + Sync {
    async fn handle(&self, bootstrap: IncomingCallBootstrap, cancellation: CancellationToken);
}

#[async_trait]
impl FcmSecretStore for SecretStore {
    async fn load(&self) -> Result<Option<FcmSecretState>, FcmServiceError> {
        let Some(serialized) = self
            .load_call_push_state()
            .await
            .map_err(|_| FcmServiceError::Persistence)?
        else {
            return Ok(None);
        };
        FcmSecretState::deserialize_secret(&serialized).map(Some)
    }

    async fn save(&self, state: &FcmSecretState) -> Result<(), FcmServiceError> {
        let serialized = Zeroizing::new(state.serialize_secret()?);
        self.save_call_push_state(serialized.as_slice())
            .await
            .map_err(|_| FcmServiceError::Persistence)
    }

    async fn clear(&self) -> Result<(), FcmServiceError> {
        self.clear_call_push_state()
            .await
            .map_err(|_| FcmServiceError::Persistence)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PushSupervisorConfig {
    auth_poll_interval: Duration,
    retry_initial_delay: Duration,
    retry_max_delay: Duration,
    fcm_reconnect_delay: Duration,
}

impl PushSupervisorConfig {
    pub fn new(
        auth_poll_interval: Duration,
        retry_initial_delay: Duration,
        retry_max_delay: Duration,
        fcm_reconnect_delay: Duration,
    ) -> Result<Self, PushSupervisorError> {
        if auth_poll_interval.is_zero()
            || retry_initial_delay.is_zero()
            || retry_initial_delay > retry_max_delay
            || retry_max_delay > MAX_SUPERVISOR_DELAY
            || fcm_reconnect_delay.is_zero()
            || fcm_reconnect_delay > MAX_SUPERVISOR_DELAY
        {
            return Err(PushSupervisorError::InvalidConfiguration);
        }
        Ok(Self {
            auth_poll_interval,
            retry_initial_delay,
            retry_max_delay,
            fcm_reconnect_delay,
        })
    }
}

impl Default for PushSupervisorConfig {
    fn default() -> Self {
        Self::new(
            Duration::from_secs(1),
            Duration::from_secs(1),
            Duration::from_secs(60),
            Duration::from_secs(5),
        )
        .expect("production push supervisor timing is valid")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PushSupervisorError {
    #[error("push supervisor configuration is invalid")]
    InvalidConfiguration,
}

pub struct PushSupervisor {
    auth: Arc<dyn PushAuthState>,
    registration_client: Arc<dyn PushRegistrationClient>,
    handler: Arc<dyn IncomingCallHandler>,
    store: Arc<dyn FcmSecretStore>,
    bootstrap_service: FcmBootstrapService,
    config: PushSupervisorConfig,
}

impl PushSupervisor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        auth: Arc<dyn PushAuthState>,
        registration_client: Arc<dyn PushRegistrationClient>,
        handler: Arc<dyn IncomingCallHandler>,
        transport: Arc<dyn FcmTransport>,
        store: Arc<dyn FcmSecretStore>,
        firebase: FirebaseConfig,
        config: PushSupervisorConfig,
    ) -> Self {
        let bootstrap_service = FcmBootstrapService::new(
            transport,
            store.clone(),
            firebase,
            config.fcm_reconnect_delay,
        );
        Self {
            auth,
            registration_client,
            handler,
            store,
            bootstrap_service,
            config,
        }
    }

    pub async fn run(&self, cancellation: CancellationToken) -> Result<(), PushSupervisorError> {
        let mut cleared_for_anonymous_session = false;
        let mut fresh_recovery_used = false;
        let mut retry_delay = self.config.retry_initial_delay;

        loop {
            let authenticated = tokio::select! {
                () = cancellation.cancelled() => return Ok(()),
                authenticated = self.auth.is_authenticated() => authenticated,
            };
            if !authenticated {
                if !cleared_for_anonymous_session {
                    match self.store.clear().await {
                        Ok(()) => {
                            cleared_for_anonymous_session = true;
                            retry_delay = self.config.retry_initial_delay;
                        }
                        Err(_) => {
                            if !wait_or_cancel(retry_delay, &cancellation).await {
                                return Ok(());
                            }
                            retry_delay = self.next_retry_delay(retry_delay);
                            continue;
                        }
                    }
                }
                if !wait_or_cancel(self.config.auth_poll_interval, &cancellation).await {
                    return Ok(());
                }
                continue;
            }

            cleared_for_anonymous_session = false;
            let active_cancellation = cancellation.child_token();
            let active = self.run_active(active_cancellation.clone());
            tokio::pin!(active);
            let outcome = loop {
                tokio::select! {
                    biased;
                    () = cancellation.cancelled() => {
                        active_cancellation.cancel();
                        let _ = active.await;
                        return Ok(());
                    }
                    result = &mut active => break ActiveOutcome::Finished(result),
                    () = tokio::time::sleep(self.config.auth_poll_interval) => {
                        if !self.auth.is_authenticated().await {
                            active_cancellation.cancel();
                            let _ = active.await;
                            break ActiveOutcome::LoggedOut;
                        }
                    }
                }
            };

            match outcome {
                ActiveOutcome::LoggedOut => {
                    if self.store.clear().await.is_ok() {
                        cleared_for_anonymous_session = true;
                    }
                    fresh_recovery_used = false;
                    retry_delay = self.config.retry_initial_delay;
                }
                ActiveOutcome::Finished(Err(ActiveError::Fcm(
                    FcmServiceError::RegistrationStale,
                ))) if !fresh_recovery_used => {
                    if self.store.clear().await.is_ok() {
                        fresh_recovery_used = true;
                    }
                    if !self
                        .wait_retry_while_authenticated(retry_delay, &cancellation)
                        .await
                    {
                        continue;
                    }
                    retry_delay = self.next_retry_delay(retry_delay);
                }
                ActiveOutcome::Finished(_) => {
                    if !self
                        .wait_retry_while_authenticated(retry_delay, &cancellation)
                        .await
                    {
                        continue;
                    }
                    retry_delay = self.next_retry_delay(retry_delay);
                }
            }
        }
    }

    async fn run_active(&self, cancellation: CancellationToken) -> Result<(), ActiveError> {
        let state = tokio::select! {
            () = cancellation.cancelled() => return Ok(()),
            result = self.bootstrap_service.ensure_registration() => result.map_err(ActiveError::Fcm)?,
        };
        let user_id = tokio::select! {
            () = cancellation.cancelled() => return Ok(()),
            result = self.registration_client.current_user_id() => {
                Zeroizing::new(result.map_err(|_| ActiveError::Provider)?)
            }
        };
        let token = SecretString::from(state.fcm_token().to_owned());
        tokio::select! {
            () = cancellation.cancelled() => return Ok(()),
            result = self.registration_client.register_fcm_device_token(&user_id, &token) => {
                result.map_err(|_| ActiveError::Provider)?;
            }
        }

        let (output, mut incoming) = mpsc::channel(MAX_PENDING_BOOTSTRAPS);
        let service_cancellation = cancellation.child_token();
        let service = self
            .bootstrap_service
            .run(output, service_cancellation.clone());
        tokio::pin!(service);
        let mut handlers = JoinSet::new();
        let mut handler_cancellation = None;
        let outcome = loop {
            tokio::select! {
                biased;
                () = cancellation.cancelled() => break Ok(()),
                bootstrap = incoming.recv() => {
                    let Some(bootstrap) = bootstrap else {
                        break service.await.map_err(ActiveError::Fcm);
                    };
                    let authenticated = tokio::select! {
                        () = cancellation.cancelled() => break Ok(()),
                        authenticated = self.auth.is_authenticated() => authenticated,
                    };
                    if !authenticated {
                        break Ok(());
                    }
                    if handlers.is_empty() {
                        let call_cancellation = cancellation.child_token();
                        let handler = self.handler.clone();
                        handler_cancellation = Some(call_cancellation.clone());
                        handlers.spawn(async move {
                            handler.handle(bootstrap, call_cancellation).await;
                        });
                    }
                }
                result = handlers.join_next(), if !handlers.is_empty() => {
                    handler_cancellation = None;
                    if result.is_some_and(|result| result.is_err()) {
                        break Err(ActiveError::Handler);
                    }
                }
                result = &mut service => break result.map_err(ActiveError::Fcm),
            }
        };
        service_cancellation.cancel();
        shutdown_handlers(&mut handlers, handler_cancellation).await;
        outcome
    }

    async fn wait_retry_while_authenticated(
        &self,
        delay: Duration,
        cancellation: &CancellationToken,
    ) -> bool {
        let retry = tokio::time::sleep(delay);
        tokio::pin!(retry);
        loop {
            tokio::select! {
                () = cancellation.cancelled() => return false,
                () = &mut retry => return true,
                () = tokio::time::sleep(self.config.auth_poll_interval) => {
                    if !self.auth.is_authenticated().await {
                        return false;
                    }
                }
            }
        }
    }

    fn next_retry_delay(&self, current: Duration) -> Duration {
        current
            .checked_mul(2)
            .unwrap_or(self.config.retry_max_delay)
            .min(self.config.retry_max_delay)
    }
}

enum ActiveOutcome {
    LoggedOut,
    Finished(Result<(), ActiveError>),
}

enum ActiveError {
    Provider,
    Handler,
    Fcm(FcmServiceError),
}

async fn wait_or_cancel(delay: Duration, cancellation: &CancellationToken) -> bool {
    tokio::select! {
        () = cancellation.cancelled() => false,
        () = tokio::time::sleep(delay) => true,
    }
}

async fn shutdown_handlers(handlers: &mut JoinSet<()>, cancellation: Option<CancellationToken>) {
    if let Some(cancellation) = cancellation {
        cancellation.cancel();
    }
    let graceful = async { while handlers.join_next().await.is_some() {} };
    if tokio::time::timeout(HANDLER_SHUTDOWN_TIMEOUT, graceful)
        .await
        .is_err()
    {
        handlers.abort_all();
        while handlers.join_next().await.is_some() {}
    }
}
