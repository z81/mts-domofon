use std::{
    fmt,
    sync::{Arc, Weak},
};

use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use secrecy::SecretString;
use tokio::sync::{Mutex, RwLock};

use crate::{
    domain::auth::{
        AuthFlavor, AuthFlowId, AuthProgress, ChallengeInput, PersistedSession, SessionStatus,
        SessionView, TokenBundle,
    },
    error::{AppError, ErrorCode},
    provider::mts::{ProviderAuthApi, TokenSource, sms::MtsLegacySmsAuth, sso::MtsSsoTransport},
    storage::SecretStore,
};

const REFRESH_RETRY_INITIAL_DELAY_SECONDS: i64 = 5;
const REFRESH_RETRY_MAX_DELAY_SECONDS: i64 = 5 * 60;

pub trait AuthClock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

struct SystemClock;

impl AuthClock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[async_trait]
pub trait SsoTransport: Send + Sync {
    async fn start(&self) -> Result<AuthProgress, AppError>;

    async fn complete_callback(
        &self,
        flow_id: &AuthFlowId,
        raw_callback: SecretString,
    ) -> Result<TokenBundle, AppError>;

    fn import_callback_response(
        &self,
        _raw_response: SecretString,
    ) -> Result<TokenBundle, AppError> {
        Err(AppError::new(
            ErrorCode::UpstreamSchemaChanged,
            "MTS callback response was invalid.",
            false,
        ))
    }

    async fn submit(
        &self,
        flow_id: &AuthFlowId,
        input: ChallengeInput,
    ) -> Result<AuthProgress, AppError>;

    fn clear(&self);
}

#[async_trait]
impl SsoTransport for MtsSsoTransport {
    async fn start(&self) -> Result<AuthProgress, AppError> {
        self.begin().await
    }

    async fn complete_callback(
        &self,
        flow_id: &AuthFlowId,
        raw_callback: SecretString,
    ) -> Result<TokenBundle, AppError> {
        self.complete_callback(flow_id, raw_callback).await
    }

    fn import_callback_response(
        &self,
        raw_response: SecretString,
    ) -> Result<TokenBundle, AppError> {
        self.import_callback_response(raw_response)
    }

    async fn submit(
        &self,
        flow_id: &AuthFlowId,
        input: ChallengeInput,
    ) -> Result<AuthProgress, AppError> {
        self.submit(flow_id, input).await
    }

    fn clear(&self) {
        self.clear();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogoutWarning {
    UpstreamLogoutFailed,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct LogoutResult {
    pub warning: Option<LogoutWarning>,
}

pub struct AuthCoordinator {
    this: Weak<Self>,
    state: RwLock<CoordinatorState>,
    operation: Mutex<()>,
    transport: Arc<dyn SsoTransport>,
    provider: Arc<dyn ProviderAuthApi>,
    store: SecretStore,
    clock: Arc<dyn AuthClock>,
}

#[derive(Clone, Copy)]
pub struct LegacyConfirmation {
    observed_generation: u64,
}

struct CoordinatorState {
    generation: u64,
    session: SessionState,
    last_refresh: Option<RefreshOutcome>,
    refresh_backoff: Option<RefreshBackoff>,
}

enum SessionState {
    Anonymous,
    Active(PersistedSession),
    Expired(ExpiredSession),
}

struct ExpiredSession {
    refresh_token: SecretString,
    web_sso_access_token: Option<SecretString>,
    device_token: Option<SecretString>,
    auth_flavor: AuthFlavor,
    authenticated_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

struct RefreshMaterial {
    generation: u64,
    refresh_token: SecretString,
    web_sso_access_token: Option<SecretString>,
    device_token: Option<SecretString>,
    auth_flavor: AuthFlavor,
    authenticated_at: DateTime<Utc>,
}

struct RefreshOutcome {
    observed_generation: u64,
    result: Result<SessionView, AppError>,
}

struct RefreshBackoff {
    consecutive_failures: u32,
    failed_at: DateTime<Utc>,
    retry_at: DateTime<Utc>,
    error: AppError,
}

impl AuthCoordinator {
    pub async fn restore(
        store: SecretStore,
        transport: Arc<dyn SsoTransport>,
        provider: Arc<dyn ProviderAuthApi>,
    ) -> Result<Arc<Self>, AppError> {
        Self::restore_with_clock(store, transport, provider, Arc::new(SystemClock)).await
    }

    #[cfg(feature = "test-support")]
    pub async fn restore_for_test(
        store: SecretStore,
        transport: Arc<dyn SsoTransport>,
        provider: Arc<dyn ProviderAuthApi>,
        clock: Arc<dyn AuthClock>,
    ) -> Result<Arc<Self>, AppError> {
        Self::restore_with_clock(store, transport, provider, clock).await
    }

    async fn restore_with_clock(
        store: SecretStore,
        transport: Arc<dyn SsoTransport>,
        provider: Arc<dyn ProviderAuthApi>,
        clock: Arc<dyn AuthClock>,
    ) -> Result<Arc<Self>, AppError> {
        let restored = store.load_session().await?;
        let (generation, session) = match restored {
            Some(session) => (1, SessionState::Active(session)),
            None => (0, SessionState::Anonymous),
        };
        Ok(Arc::new_cyclic(|this| Self {
            this: this.clone(),
            state: RwLock::new(CoordinatorState {
                generation,
                session,
                last_refresh: None,
                refresh_backoff: None,
            }),
            operation: Mutex::new(()),
            transport,
            provider,
            store,
            clock,
        }))
    }

    pub async fn start(&self) -> Result<AuthProgress, AppError> {
        let _operation = self.operation.lock().await;
        let progress = self.transport.start().await?;
        let mut state = self.state.write().await;
        state.generation = next_generation(state.generation)?;
        state.last_refresh = None;
        Ok(progress)
    }

    pub async fn complete_callback(
        &self,
        flow_id: &AuthFlowId,
        raw_callback: SecretString,
    ) -> Result<AuthProgress, AppError> {
        let _operation = self.operation.lock().await;
        let tokens = self
            .transport
            .complete_callback(flow_id, raw_callback)
            .await?;
        let view = self.install_web_sso_session(tokens).await?;
        Ok(AuthProgress::Authenticated { session: view })
    }

    pub async fn import_web_sso_response(
        &self,
        raw_response: SecretString,
    ) -> Result<AuthProgress, AppError> {
        let _operation = self.operation.lock().await;
        let tokens = self.transport.import_callback_response(raw_response)?;
        let view = self.install_web_sso_session(tokens).await?;
        self.transport.clear();
        Ok(AuthProgress::Authenticated { session: view })
    }

    pub async fn prepare_legacy_confirmation(&self) -> LegacyConfirmation {
        LegacyConfirmation {
            observed_generation: self.state.read().await.generation,
        }
    }

    pub async fn complete_legacy_sms(
        &self,
        confirmation: LegacyConfirmation,
        legacy: &MtsLegacySmsAuth,
        flow_id: &AuthFlowId,
        code: SecretString,
    ) -> Result<AuthProgress, AppError> {
        let _operation = self.operation.lock().await;
        if self.state.read().await.generation != confirmation.observed_generation {
            return Err(stale_legacy_confirmation_error());
        }
        let tokens = legacy.confirm(flow_id, code).await?;
        let session = PersistedSession {
            tokens: TokenBundle {
                access_token: tokens.access_token,
                refresh_token: tokens.refresh_token,
                web_sso_access_token: None,
                device_token: None,
            },
            auth_flavor: AuthFlavor::LegacySms,
            authenticated_at: self.clock.now(),
        };
        self.store.save_session(&session).await?;
        self.transport.clear();
        let view = active_view(&session);
        let mut state = self.state.write().await;
        state.generation = next_generation(state.generation)?;
        state.session = SessionState::Active(session);
        state.last_refresh = None;
        state.refresh_backoff = None;
        Ok(AuthProgress::Authenticated { session: view })
    }

    pub async fn submit(
        &self,
        flow_id: &AuthFlowId,
        input: ChallengeInput,
    ) -> Result<AuthProgress, AppError> {
        let _operation = self.operation.lock().await;
        self.transport.submit(flow_id, input).await
    }

    pub async fn session(&self) -> SessionView {
        session_view(&self.state.read().await.session)
    }

    pub async fn refresh(&self) -> Result<SessionView, AppError> {
        let observed_generation = self.state.read().await.generation;
        // Rotation runs detached so a dropped caller (client disconnect) cannot lose rotated tokens.
        let Some(coordinator) = self.this.upgrade() else {
            return self.refresh_observed(observed_generation).await;
        };
        tokio::spawn(async move { coordinator.refresh_observed(observed_generation).await })
            .await
            .unwrap_or_else(|_| Err(interrupted_refresh_error()))
    }

    async fn refresh_observed(&self, observed_generation: u64) -> Result<SessionView, AppError> {
        let _operation = self.operation.lock().await;
        let material = {
            let state = self.state.read().await;
            if state.generation != observed_generation {
                return coalesced_refresh_result(&state, observed_generation);
            }
            // A clock that moved backwards ends the backoff instead of extending it.
            if let Some(backoff) = &state.refresh_backoff
                && (backoff.failed_at..backoff.retry_at).contains(&self.clock.now())
            {
                return Err(backoff.error.clone());
            }
            refresh_material(&state)?
        };
        let replacement_generation = next_generation(material.generation)?;

        let keycloak = match self
            .provider
            .rotate_keycloak(&material.refresh_token, material.device_token.as_ref())
            .await
        {
            Ok(tokens) => tokens,
            Err(error) if error.code == ErrorCode::UpstreamAuthenticationFailed => {
                tracing::warn!(
                    code = ?error.code,
                    "MTS rejected the session refresh; session expired"
                );
                self.install_expired(material, replacement_generation, error.clone())
                    .await;
                return Err(error);
            }
            Err(error) => {
                self.start_refresh_backoff(&error).await;
                return Err(error);
            }
        };
        let web_sso_access_token = if material.auth_flavor == AuthFlavor::WebSso {
            match self
                .provider
                .rotate_web_sso(&keycloak.access_token, material.device_token.as_ref())
                .await
            {
                Ok(token) => Some(token),
                Err(error) => {
                    tracing::warn!(
                        code = ?error.code,
                        retryable = error.retryable,
                        "MTS web SSO refresh failed; keeping the previous web SSO token"
                    );
                    material.web_sso_access_token
                }
            }
        } else {
            None
        };
        let replacement = PersistedSession {
            tokens: TokenBundle {
                access_token: keycloak.access_token,
                refresh_token: keycloak.refresh_token,
                web_sso_access_token,
                device_token: material.device_token,
            },
            auth_flavor: material.auth_flavor,
            authenticated_at: material.authenticated_at,
        };
        if let Err(error) = self.store.save_session(&replacement).await {
            tracing::error!(
                code = ?error.code,
                "refreshed MTS session could not be persisted; keeping it in memory"
            );
        }

        let view = active_view(&replacement);
        let mut state = self.state.write().await;
        state.generation = replacement_generation;
        state.session = SessionState::Active(replacement);
        state.last_refresh = Some(RefreshOutcome {
            observed_generation,
            result: Ok(view.clone()),
        });
        let previous_failures = state
            .refresh_backoff
            .take()
            .map_or(0, |backoff| backoff.consecutive_failures);
        drop(state);
        tracing::info!(previous_failures, "MTS session refreshed");
        Ok(view)
    }

    pub async fn logout(&self) -> Result<LogoutResult, AppError> {
        let _operation = self.operation.lock().await;
        let logout_material = {
            let mut state = self.state.write().await;
            let logout_material = match &state.session {
                SessionState::Active(session) => Some((
                    session.tokens.refresh_token.clone(),
                    session.tokens.device_token.clone(),
                    session.auth_flavor,
                )),
                SessionState::Expired(session) => Some((
                    session.refresh_token.clone(),
                    session.device_token.clone(),
                    session.auth_flavor,
                )),
                SessionState::Anonymous => None,
            };
            state.generation = next_generation(state.generation)?;
            state.session = SessionState::Anonymous;
            state.last_refresh = None;
            state.refresh_backoff = None;
            logout_material
        };
        self.transport.clear();

        let storage_result = self.store.clear_session().await;
        let warning = if let Some((refresh_token, device_token, auth_flavor)) = logout_material {
            self.provider
                .logout(&refresh_token, device_token.as_ref(), auth_flavor)
                .await
                .err()
                .map(|_| LogoutWarning::UpstreamLogoutFailed)
        } else {
            None
        };
        storage_result?;
        Ok(LogoutResult { warning })
    }

    async fn install_expired(&self, material: RefreshMaterial, generation: u64, error: AppError) {
        let mut state = self.state.write().await;
        state.generation = generation;
        state.session = SessionState::Expired(ExpiredSession {
            refresh_token: material.refresh_token,
            web_sso_access_token: material.web_sso_access_token,
            device_token: material.device_token,
            auth_flavor: material.auth_flavor,
            authenticated_at: material.authenticated_at,
            expires_at: self.clock.now(),
        });
        state.last_refresh = Some(RefreshOutcome {
            observed_generation: material.generation,
            result: Err(error),
        });
        state.refresh_backoff = None;
    }

    async fn start_refresh_backoff(&self, error: &AppError) {
        let failed_at = self.clock.now();
        let mut state = self.state.write().await;
        let consecutive_failures = state
            .refresh_backoff
            .as_ref()
            .map_or(1, |backoff| backoff.consecutive_failures.saturating_add(1));
        let retry_after_seconds = error.retry_after_seconds.map_or(0, |seconds| {
            i64::try_from(seconds)
                .unwrap_or(i64::MAX)
                .min(REFRESH_RETRY_MAX_DELAY_SECONDS)
        });
        let retry_delay_seconds =
            refresh_retry_delay_seconds(consecutive_failures).max(retry_after_seconds);
        state.refresh_backoff = Some(RefreshBackoff {
            consecutive_failures,
            failed_at,
            retry_at: failed_at + TimeDelta::seconds(retry_delay_seconds),
            error: error.clone(),
        });
        drop(state);
        tracing::warn!(
            code = ?error.code,
            retryable = error.retryable,
            consecutive_failures,
            retry_delay_seconds,
            "MTS session refresh failed; keeping the current session"
        );
    }

    async fn install_web_sso_session(&self, tokens: TokenBundle) -> Result<SessionView, AppError> {
        let session = PersistedSession {
            tokens,
            auth_flavor: AuthFlavor::WebSso,
            authenticated_at: self.clock.now(),
        };
        self.store.save_session(&session).await?;
        let view = active_view(&session);
        let mut state = self.state.write().await;
        state.generation = next_generation(state.generation)?;
        state.session = SessionState::Active(session);
        state.last_refresh = None;
        state.refresh_backoff = None;
        Ok(view)
    }
}

impl fmt::Debug for AuthCoordinator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthCoordinator")
            .field("session", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl TokenSource for AuthCoordinator {
    async fn access_token(&self) -> Option<SecretString> {
        let state = self.state.read().await;
        match &state.session {
            SessionState::Active(session) => Some(session.tokens.access_token.clone()),
            SessionState::Anonymous | SessionState::Expired(_) => None,
        }
    }

    async fn device_token(&self) -> Option<SecretString> {
        let state = self.state.read().await;
        match &state.session {
            SessionState::Active(session) => session.tokens.device_token.clone(),
            SessionState::Anonymous | SessionState::Expired(_) => None,
        }
    }

    async fn refresh_after_unauthorized(&self) -> Result<(), AppError> {
        self.refresh().await.map(|_| ())
    }
}

fn session_view(session: &SessionState) -> SessionView {
    match session {
        SessionState::Anonymous => SessionView::anonymous(),
        SessionState::Active(session) => active_view(session),
        SessionState::Expired(session) => SessionView {
            authenticated: false,
            status: SessionStatus::Expired,
            authenticated_at: Some(session.authenticated_at),
            expires_at: Some(session.expires_at),
        },
    }
}

fn refresh_material(state: &CoordinatorState) -> Result<RefreshMaterial, AppError> {
    match &state.session {
        SessionState::Active(session) => Ok(RefreshMaterial {
            generation: state.generation,
            refresh_token: session.tokens.refresh_token.clone(),
            web_sso_access_token: session.tokens.web_sso_access_token.clone(),
            device_token: session.tokens.device_token.clone(),
            auth_flavor: session.auth_flavor,
            authenticated_at: session.authenticated_at,
        }),
        SessionState::Expired(session) => Ok(RefreshMaterial {
            generation: state.generation,
            refresh_token: session.refresh_token.clone(),
            web_sso_access_token: session.web_sso_access_token.clone(),
            device_token: session.device_token.clone(),
            auth_flavor: session.auth_flavor,
            authenticated_at: session.authenticated_at,
        }),
        SessionState::Anonymous => Err(session_expired_error()),
    }
}

fn coalesced_refresh_result(
    state: &CoordinatorState,
    observed_generation: u64,
) -> Result<SessionView, AppError> {
    if let Some(outcome) = &state.last_refresh
        && outcome.observed_generation == observed_generation
    {
        return outcome.result.clone();
    }
    match &state.session {
        SessionState::Active(session) => Ok(active_view(session)),
        SessionState::Anonymous | SessionState::Expired(_) => Err(session_expired_error()),
    }
}

fn active_view(session: &PersistedSession) -> SessionView {
    SessionView {
        authenticated: true,
        status: SessionStatus::Active,
        authenticated_at: Some(session.authenticated_at),
        expires_at: None,
    }
}

fn refresh_retry_delay_seconds(consecutive_failures: u32) -> i64 {
    REFRESH_RETRY_INITIAL_DELAY_SECONDS
        .saturating_mul(2_i64.saturating_pow(consecutive_failures.saturating_sub(1)))
        .min(REFRESH_RETRY_MAX_DELAY_SECONDS)
}

fn next_generation(generation: u64) -> Result<u64, AppError> {
    generation.checked_add(1).ok_or_else(|| {
        AppError::new(
            ErrorCode::Internal,
            "Authentication session generation was exhausted.",
            false,
        )
    })
}

fn session_expired_error() -> AppError {
    AppError::new(
        ErrorCode::SessionExpired,
        "Provider session expired.",
        false,
    )
}

fn interrupted_refresh_error() -> AppError {
    AppError::new(
        ErrorCode::Internal,
        "Provider session refresh was interrupted.",
        true,
    )
}

fn stale_legacy_confirmation_error() -> AppError {
    AppError::new(
        ErrorCode::AuthFlowNotFound,
        "Legacy SMS authentication flow is no longer current.",
        false,
    )
}
