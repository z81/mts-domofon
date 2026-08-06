use std::{
    collections::VecDeque,
    str::FromStr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use domru_mts::auth::{AuthClock, AuthCoordinator, LogoutWarning, SsoTransport};
use domru_mts::domain::auth::{
    AuthFlowId, AuthProgress, ChallengeInput, PersistedSession, SessionStatus, SessionView,
    TokenBundle,
};
use domru_mts::{
    error::{AppError, ErrorCode},
    provider::mts::{KeycloakRotation, MtsAuthApi, MtsClient, ProviderAuthApi, TokenSource},
    state::AppState,
    storage::SecretStore,
};
use secrecy::{ExposeSecret, SecretString};
use tempfile::TempDir;
use url::Url;
use uuid::Uuid;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

const ACCESS_OLD: &str = "coordinator-access-old-secret";
const REFRESH_OLD: &str = "coordinator-refresh-old-secret";
const WEB_SSO_OLD: &str = "coordinator-websso-old-secret";
const DEVICE_OLD: &str = "coordinator-device-old-secret";
const CALLBACK_SECRET: &str = "coordinator-callback-secret";

fn instant(hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 6, hour, 0, 0).unwrap()
}

fn bundle(access: &str, refresh: &str, web_sso: &str) -> TokenBundle {
    TokenBundle {
        access_token: SecretString::from(access.to_owned()),
        refresh_token: SecretString::from(refresh.to_owned()),
        web_sso_access_token: SecretString::from(web_sso.to_owned()),
        device_token: Some(SecretString::from(DEVICE_OLD)),
    }
}

fn persisted_session() -> PersistedSession {
    PersistedSession {
        tokens: bundle(ACCESS_OLD, REFRESH_OLD, WEB_SSO_OLD),
        authenticated_at: instant(9),
    }
}

fn flow_id() -> AuthFlowId {
    AuthFlowId::from_str("00000000-0000-4000-8000-000000000001").unwrap()
}

#[derive(Clone)]
struct FixedClock(DateTime<Utc>);

impl AuthClock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

struct FakeSsoTransport {
    callback_tokens: Mutex<Option<TokenBundle>>,
    cleared: AtomicBool,
    active_flows: AtomicUsize,
}

impl FakeSsoTransport {
    fn with_callback(tokens: TokenBundle) -> Self {
        Self {
            callback_tokens: Mutex::new(Some(tokens)),
            cleared: AtomicBool::new(false),
            active_flows: AtomicUsize::new(0),
        }
    }

    fn without_callback() -> Self {
        Self {
            callback_tokens: Mutex::new(None),
            cleared: AtomicBool::new(false),
            active_flows: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl SsoTransport for FakeSsoTransport {
    async fn start(&self) -> Result<AuthProgress, AppError> {
        self.active_flows.fetch_add(1, Ordering::SeqCst);
        Ok(AuthProgress::BrowserRedirect {
            flow_id: flow_id(),
            authorization_url: Url::parse("https://login.mts.ru/amserver/oauth2/authorize")
                .unwrap(),
        })
    }

    async fn complete_callback(
        &self,
        _flow_id: &AuthFlowId,
        _raw_callback: SecretString,
    ) -> Result<TokenBundle, AppError> {
        self.callback_tokens
            .lock()
            .unwrap()
            .take()
            .ok_or_else(session_expired_error)
    }

    async fn submit(
        &self,
        _flow_id: &AuthFlowId,
        _input: ChallengeInput,
    ) -> Result<AuthProgress, AppError> {
        Err(AppError::new(
            ErrorCode::UpstreamSchemaChanged,
            "MTS browser challenge capture is required.",
            false,
        ))
    }

    fn clear(&self) {
        self.cleared.store(true, Ordering::SeqCst);
        self.active_flows.store(0, Ordering::SeqCst);
        let _ = self.callback_tokens.lock().unwrap().take();
    }
}

enum KeycloakAnswer {
    Success {
        access_token: &'static str,
        refresh_token: &'static str,
    },
    Failure,
}

enum WebSsoAnswer {
    Success(&'static str),
    Failure,
}

struct FakeProviderAuthApi {
    keycloak_answers: Mutex<VecDeque<KeycloakAnswer>>,
    web_sso_answers: Mutex<VecDeque<WebSsoAnswer>>,
    logout_fails: AtomicBool,
    keycloak_calls: AtomicUsize,
    web_sso_calls: AtomicUsize,
    logout_calls: AtomicUsize,
    seen_refresh_tokens: Mutex<Vec<String>>,
    seen_keycloak_device_tokens: Mutex<Vec<Option<String>>>,
    seen_web_access_tokens: Mutex<Vec<String>>,
    seen_web_device_tokens: Mutex<Vec<Option<String>>>,
    seen_logout_refresh_tokens: Mutex<Vec<String>>,
    seen_logout_device_tokens: Mutex<Vec<Option<String>>>,
    first_keycloak_entered: Option<Arc<tokio::sync::Barrier>>,
    release_first_keycloak: Option<Arc<tokio::sync::Barrier>>,
    first_logout_entered: Option<Arc<tokio::sync::Barrier>>,
    release_first_logout: Option<Arc<tokio::sync::Barrier>>,
}

impl FakeProviderAuthApi {
    fn new(
        keycloak_answers: impl IntoIterator<Item = KeycloakAnswer>,
        web_sso_answers: impl IntoIterator<Item = WebSsoAnswer>,
    ) -> Self {
        Self {
            keycloak_answers: Mutex::new(keycloak_answers.into_iter().collect()),
            web_sso_answers: Mutex::new(web_sso_answers.into_iter().collect()),
            logout_fails: AtomicBool::new(false),
            keycloak_calls: AtomicUsize::new(0),
            web_sso_calls: AtomicUsize::new(0),
            logout_calls: AtomicUsize::new(0),
            seen_refresh_tokens: Mutex::new(Vec::new()),
            seen_keycloak_device_tokens: Mutex::new(Vec::new()),
            seen_web_access_tokens: Mutex::new(Vec::new()),
            seen_web_device_tokens: Mutex::new(Vec::new()),
            seen_logout_refresh_tokens: Mutex::new(Vec::new()),
            seen_logout_device_tokens: Mutex::new(Vec::new()),
            first_keycloak_entered: None,
            release_first_keycloak: None,
            first_logout_entered: None,
            release_first_logout: None,
        }
    }

    fn idle() -> Self {
        Self::new([], [])
    }

    fn with_first_keycloak_barriers(
        mut self,
        entered: Arc<tokio::sync::Barrier>,
        release: Arc<tokio::sync::Barrier>,
    ) -> Self {
        self.first_keycloak_entered = Some(entered);
        self.release_first_keycloak = Some(release);
        self
    }

    fn with_first_logout_barriers(
        mut self,
        entered: Arc<tokio::sync::Barrier>,
        release: Arc<tokio::sync::Barrier>,
    ) -> Self {
        self.first_logout_entered = Some(entered);
        self.release_first_logout = Some(release);
        self
    }
}

#[async_trait]
impl ProviderAuthApi for FakeProviderAuthApi {
    async fn rotate_keycloak(
        &self,
        refresh_token: &SecretString,
        device_token: Option<&SecretString>,
    ) -> Result<KeycloakRotation, AppError> {
        self.seen_refresh_tokens
            .lock()
            .unwrap()
            .push(refresh_token.expose_secret().to_owned());
        self.seen_keycloak_device_tokens
            .lock()
            .unwrap()
            .push(device_token.map(|token| token.expose_secret().to_owned()));
        let call = self.keycloak_calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            if let Some(entered) = &self.first_keycloak_entered {
                entered.wait().await;
            }
            if let Some(release) = &self.release_first_keycloak {
                release.wait().await;
            }
        }
        match self.keycloak_answers.lock().unwrap().pop_front() {
            Some(KeycloakAnswer::Success {
                access_token,
                refresh_token,
            }) => Ok(KeycloakRotation {
                access_token: SecretString::from(access_token),
                refresh_token: SecretString::from(refresh_token),
            }),
            Some(KeycloakAnswer::Failure) => Err(upstream_error()),
            None => panic!("unexpected Keycloak rotation"),
        }
    }

    async fn rotate_web_sso(
        &self,
        access_token: &SecretString,
        device_token: Option<&SecretString>,
    ) -> Result<SecretString, AppError> {
        self.seen_web_access_tokens
            .lock()
            .unwrap()
            .push(access_token.expose_secret().to_owned());
        self.seen_web_device_tokens
            .lock()
            .unwrap()
            .push(device_token.map(|token| token.expose_secret().to_owned()));
        self.web_sso_calls.fetch_add(1, Ordering::SeqCst);
        match self.web_sso_answers.lock().unwrap().pop_front() {
            Some(WebSsoAnswer::Success(token)) => Ok(SecretString::from(token)),
            Some(WebSsoAnswer::Failure) => Err(upstream_error()),
            None => panic!("unexpected WebSSO rotation"),
        }
    }

    async fn logout(
        &self,
        refresh_token: &SecretString,
        device_token: Option<&SecretString>,
    ) -> Result<(), AppError> {
        self.seen_logout_refresh_tokens
            .lock()
            .unwrap()
            .push(refresh_token.expose_secret().to_owned());
        self.seen_logout_device_tokens
            .lock()
            .unwrap()
            .push(device_token.map(|token| token.expose_secret().to_owned()));
        let call = self.logout_calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            if let Some(entered) = &self.first_logout_entered {
                entered.wait().await;
            }
            if let Some(release) = &self.release_first_logout {
                release.wait().await;
            }
        }
        if self.logout_fails.load(Ordering::SeqCst) {
            Err(upstream_error())
        } else {
            Ok(())
        }
    }
}

fn upstream_error() -> AppError {
    AppError::new(
        ErrorCode::UpstreamUnavailable,
        "Synthetic upstream failure.",
        true,
    )
}

fn session_expired_error() -> AppError {
    AppError::new(
        ErrorCode::SessionExpired,
        "Synthetic session expired.",
        false,
    )
}

async fn open_store(data_dir: &TempDir) -> SecretStore {
    SecretStore::open(data_dir.path(), Some([7_u8; 32]))
        .await
        .unwrap()
}

async fn restore_with(
    store: SecretStore,
    sso: Arc<FakeSsoTransport>,
    provider: Arc<FakeProviderAuthApi>,
) -> Arc<AuthCoordinator> {
    AuthCoordinator::restore_for_test(store, sso, provider, Arc::new(FixedClock(instant(10))))
        .await
        .unwrap()
}

#[tokio::test]
async fn restore_distinguishes_an_absent_session_from_a_persisted_session() {
    let anonymous_dir = TempDir::new().unwrap();
    let anonymous_store = open_store(&anonymous_dir).await;
    let anonymous = restore_with(
        anonymous_store,
        Arc::new(FakeSsoTransport::without_callback()),
        Arc::new(FakeProviderAuthApi::idle()),
    )
    .await;
    assert_eq!(anonymous.session().await, SessionView::anonymous());
    assert!(
        TokenSource::access_token(anonymous.as_ref())
            .await
            .is_none()
    );
    assert!(
        TokenSource::device_token(anonymous.as_ref())
            .await
            .is_none()
    );

    let restored_dir = TempDir::new().unwrap();
    let restored_store = open_store(&restored_dir).await;
    restored_store
        .save_session(&persisted_session())
        .await
        .unwrap();
    let restored = restore_with(
        restored_store,
        Arc::new(FakeSsoTransport::without_callback()),
        Arc::new(FakeProviderAuthApi::idle()),
    )
    .await;

    assert_eq!(
        restored.session().await,
        SessionView {
            authenticated: true,
            status: SessionStatus::Active,
            authenticated_at: Some(instant(9)),
            expires_at: None,
        }
    );
    assert_eq!(
        TokenSource::access_token(restored.as_ref())
            .await
            .unwrap()
            .expose_secret(),
        ACCESS_OLD
    );
    assert_eq!(
        TokenSource::device_token(restored.as_ref())
            .await
            .unwrap()
            .expose_secret(),
        DEVICE_OLD
    );
}

#[tokio::test]
async fn restore_propagates_corrupt_encrypted_session_errors() {
    let data_dir = TempDir::new().unwrap();
    let store = open_store(&data_dir).await;
    tokio::fs::write(
        data_dir.path().join("session.enc"),
        b"not-an-encrypted-session",
    )
    .await
    .unwrap();

    let error = AuthCoordinator::restore_for_test(
        store,
        Arc::new(FakeSsoTransport::without_callback()),
        Arc::new(FakeProviderAuthApi::idle()),
        Arc::new(FixedClock(instant(10))),
    )
    .await
    .unwrap_err();

    assert_eq!(error.code, ErrorCode::StorageFailure);
}

#[tokio::test]
async fn callback_completion_persists_then_installs_one_complete_bundle() {
    let data_dir = TempDir::new().unwrap();
    let store = open_store(&data_dir).await;
    let sso = Arc::new(FakeSsoTransport::with_callback(bundle(
        "callback-access-secret",
        "callback-refresh-secret",
        "callback-websso-secret",
    )));
    let coordinator = restore_with(store.clone(), sso, Arc::new(FakeProviderAuthApi::idle())).await;

    let progress = coordinator
        .complete_callback(&flow_id(), SecretString::from(CALLBACK_SECRET))
        .await
        .unwrap();

    assert_eq!(
        serde_json::to_value(&progress).unwrap(),
        serde_json::json!({
            "status": "authenticated",
            "session": {
                "authenticated": true,
                "status": "active",
                "authenticated_at": "2026-08-06T10:00:00Z",
                "expires_at": null
            }
        })
    );
    let persisted = store.load_session().await.unwrap().unwrap();
    assert_eq!(
        persisted.tokens.access_token.expose_secret(),
        "callback-access-secret"
    );
    assert_eq!(
        persisted.tokens.refresh_token.expose_secret(),
        "callback-refresh-secret"
    );
    assert_eq!(
        persisted.tokens.web_sso_access_token.expose_secret(),
        "callback-websso-secret"
    );
    assert_eq!(persisted.authenticated_at, instant(10));
    assert_eq!(
        TokenSource::access_token(coordinator.as_ref())
            .await
            .unwrap()
            .expose_secret(),
        "callback-access-secret"
    );
    let second = coordinator
        .complete_callback(&flow_id(), SecretString::from(CALLBACK_SECRET))
        .await
        .unwrap_err();
    assert_eq!(second.code, ErrorCode::SessionExpired);

    let output = format!("{coordinator:?} {progress:?}");
    for secret in [
        "callback-access-secret",
        "callback-refresh-secret",
        "callback-websso-secret",
        CALLBACK_SECRET,
    ] {
        assert!(!output.contains(secret));
    }
}

#[tokio::test]
async fn callback_persistence_failure_keeps_the_in_memory_session_anonymous() {
    let data_dir = TempDir::new().unwrap();
    let store = open_store(&data_dir).await;
    let coordinator = restore_with(
        store,
        Arc::new(FakeSsoTransport::with_callback(bundle(
            "must-not-install-access",
            "must-not-install-refresh",
            "must-not-install-websso",
        ))),
        Arc::new(FakeProviderAuthApi::idle()),
    )
    .await;
    tokio::fs::create_dir(data_dir.path().join("session.enc"))
        .await
        .unwrap();

    let error = coordinator
        .complete_callback(&flow_id(), SecretString::from(CALLBACK_SECRET))
        .await
        .unwrap_err();

    assert_eq!(error.code, ErrorCode::StorageFailure);
    assert_eq!(coordinator.session().await, SessionView::anonymous());
    assert!(
        TokenSource::access_token(coordinator.as_ref())
            .await
            .is_none()
    );
}

#[tokio::test]
async fn start_and_submit_preserve_the_sso_transport_boundary() {
    let data_dir = TempDir::new().unwrap();
    let coordinator = restore_with(
        open_store(&data_dir).await,
        Arc::new(FakeSsoTransport::without_callback()),
        Arc::new(FakeProviderAuthApi::idle()),
    )
    .await;

    let start = coordinator.start().await.unwrap();
    assert!(matches!(start, AuthProgress::BrowserRedirect { .. }));
    let error = coordinator
        .submit(
            &flow_id(),
            ChallengeInput::SmsOtp {
                code: SecretString::from("otp-must-remain-secret"),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::UpstreamSchemaChanged);
    assert!(!format!("{error:?}").contains("otp-must-remain-secret"));
}

#[tokio::test(flavor = "current_thread")]
async fn twenty_refreshes_of_one_generation_share_one_complete_rotation() {
    let data_dir = TempDir::new().unwrap();
    let store = open_store(&data_dir).await;
    store.save_session(&persisted_session()).await.unwrap();
    let entered = Arc::new(tokio::sync::Barrier::new(2));
    let release = Arc::new(tokio::sync::Barrier::new(2));
    let provider = Arc::new(
        FakeProviderAuthApi::new(
            [
                KeycloakAnswer::Success {
                    access_token: "access-generation-one",
                    refresh_token: "refresh-generation-one",
                },
                KeycloakAnswer::Success {
                    access_token: "access-generation-two",
                    refresh_token: "refresh-generation-two",
                },
            ],
            [
                WebSsoAnswer::Success("websso-generation-one"),
                WebSsoAnswer::Success("websso-generation-two"),
            ],
        )
        .with_first_keycloak_barriers(Arc::clone(&entered), Arc::clone(&release)),
    );
    let coordinator = restore_with(
        store.clone(),
        Arc::new(FakeSsoTransport::without_callback()),
        Arc::clone(&provider),
    )
    .await;
    let start = Arc::new(tokio::sync::Barrier::new(21));
    let mut refreshes = Vec::new();
    for _ in 0..20 {
        let start = Arc::clone(&start);
        let coordinator = Arc::clone(&coordinator);
        refreshes.push(tokio::spawn(async move {
            start.wait().await;
            coordinator.refresh().await
        }));
    }
    start.wait().await;
    entered.wait().await;
    tokio::task::yield_now().await;
    release.wait().await;

    for refresh in refreshes {
        let view = refresh.await.unwrap().unwrap();
        assert_eq!(view.status, SessionStatus::Active);
        assert_eq!(view.authenticated_at, Some(instant(9)));
    }
    assert_eq!(provider.keycloak_calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.web_sso_calls.load(Ordering::SeqCst), 1);
    let persisted = store.load_session().await.unwrap().unwrap();
    assert_eq!(
        persisted.tokens.access_token.expose_secret(),
        "access-generation-one"
    );
    assert_eq!(
        persisted.tokens.refresh_token.expose_secret(),
        "refresh-generation-one"
    );
    assert_eq!(
        persisted.tokens.web_sso_access_token.expose_secret(),
        "websso-generation-one"
    );
    assert_eq!(
        persisted
            .tokens
            .device_token
            .as_ref()
            .unwrap()
            .expose_secret(),
        DEVICE_OLD
    );

    coordinator.refresh().await.unwrap();
    assert_eq!(provider.keycloak_calls.load(Ordering::SeqCst), 2);
    assert_eq!(provider.web_sso_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        TokenSource::access_token(coordinator.as_ref())
            .await
            .unwrap()
            .expose_secret(),
        "access-generation-two"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn concurrent_failed_refreshes_share_the_same_redacted_failure() {
    let data_dir = TempDir::new().unwrap();
    let store = open_store(&data_dir).await;
    store.save_session(&persisted_session()).await.unwrap();
    let entered = Arc::new(tokio::sync::Barrier::new(2));
    let release = Arc::new(tokio::sync::Barrier::new(2));
    let provider = Arc::new(
        FakeProviderAuthApi::new([KeycloakAnswer::Failure], [])
            .with_first_keycloak_barriers(Arc::clone(&entered), Arc::clone(&release)),
    );
    let coordinator = restore_with(
        store,
        Arc::new(FakeSsoTransport::without_callback()),
        Arc::clone(&provider),
    )
    .await;
    let start = Arc::new(tokio::sync::Barrier::new(21));
    let mut refreshes = Vec::new();
    for _ in 0..20 {
        let start = Arc::clone(&start);
        let coordinator = Arc::clone(&coordinator);
        refreshes.push(tokio::spawn(async move {
            start.wait().await;
            coordinator.refresh().await
        }));
    }
    start.wait().await;
    entered.wait().await;
    tokio::task::yield_now().await;
    release.wait().await;

    for refresh in refreshes {
        let error = refresh.await.unwrap().unwrap_err();
        assert_eq!(error.code, ErrorCode::UpstreamUnavailable);
        assert!(!format!("{error:?}").contains(REFRESH_OLD));
    }
    assert_eq!(provider.keycloak_calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.web_sso_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn failed_keycloak_refresh_expires_access_and_retry_uses_retained_refresh_material() {
    let data_dir = TempDir::new().unwrap();
    let store = open_store(&data_dir).await;
    store.save_session(&persisted_session()).await.unwrap();
    let provider = Arc::new(FakeProviderAuthApi::new(
        [
            KeycloakAnswer::Failure,
            KeycloakAnswer::Success {
                access_token: "access-after-keycloak-retry",
                refresh_token: "refresh-after-keycloak-retry",
            },
        ],
        [WebSsoAnswer::Success("websso-after-keycloak-retry")],
    ));
    let coordinator = restore_with(
        store.clone(),
        Arc::new(FakeSsoTransport::without_callback()),
        Arc::clone(&provider),
    )
    .await;

    let error = coordinator.refresh().await.unwrap_err();

    assert_eq!(error.code, ErrorCode::UpstreamUnavailable);
    assert_eq!(
        coordinator.session().await,
        SessionView {
            authenticated: false,
            status: SessionStatus::Expired,
            authenticated_at: Some(instant(9)),
            expires_at: Some(instant(10)),
        }
    );
    assert!(
        TokenSource::access_token(coordinator.as_ref())
            .await
            .is_none()
    );
    assert!(
        TokenSource::device_token(coordinator.as_ref())
            .await
            .is_none()
    );
    let still_persisted = store.load_session().await.unwrap().unwrap();
    assert_eq!(
        still_persisted.tokens.access_token.expose_secret(),
        ACCESS_OLD
    );
    assert_eq!(
        still_persisted.tokens.refresh_token.expose_secret(),
        REFRESH_OLD
    );

    TokenSource::refresh_after_unauthorized(coordinator.as_ref())
        .await
        .unwrap();

    assert_eq!(
        provider.seen_refresh_tokens.lock().unwrap().as_slice(),
        [REFRESH_OLD, REFRESH_OLD]
    );
    assert_eq!(coordinator.session().await.status, SessionStatus::Active);
    assert_eq!(
        TokenSource::access_token(coordinator.as_ref())
            .await
            .unwrap()
            .expose_secret(),
        "access-after-keycloak-retry"
    );
}

#[tokio::test]
async fn failed_websso_refresh_never_installs_partial_tokens_and_retry_uses_rotated_refresh() {
    let data_dir = TempDir::new().unwrap();
    let store = open_store(&data_dir).await;
    store.save_session(&persisted_session()).await.unwrap();
    let provider = Arc::new(FakeProviderAuthApi::new(
        [
            KeycloakAnswer::Success {
                access_token: "partial-keycloak-access",
                refresh_token: "partial-keycloak-refresh",
            },
            KeycloakAnswer::Success {
                access_token: "complete-keycloak-access",
                refresh_token: "complete-keycloak-refresh",
            },
        ],
        [
            WebSsoAnswer::Failure,
            WebSsoAnswer::Success("complete-websso-access"),
        ],
    ));
    let coordinator = restore_with(
        store.clone(),
        Arc::new(FakeSsoTransport::without_callback()),
        Arc::clone(&provider),
    )
    .await;

    let error = coordinator.refresh().await.unwrap_err();

    assert_eq!(error.code, ErrorCode::UpstreamUnavailable);
    assert_eq!(coordinator.session().await.status, SessionStatus::Expired);
    assert!(
        TokenSource::access_token(coordinator.as_ref())
            .await
            .is_none()
    );
    let still_persisted = store.load_session().await.unwrap().unwrap();
    assert_eq!(
        still_persisted.tokens.access_token.expose_secret(),
        ACCESS_OLD
    );
    assert_eq!(
        still_persisted.tokens.refresh_token.expose_secret(),
        REFRESH_OLD
    );

    coordinator.refresh().await.unwrap();

    assert_eq!(
        provider.seen_refresh_tokens.lock().unwrap().as_slice(),
        [REFRESH_OLD, "partial-keycloak-refresh"]
    );
    assert_eq!(
        provider
            .seen_keycloak_device_tokens
            .lock()
            .unwrap()
            .as_slice(),
        [Some(DEVICE_OLD.to_owned()), Some(DEVICE_OLD.to_owned())]
    );
    assert_eq!(
        provider.seen_web_access_tokens.lock().unwrap().as_slice(),
        ["partial-keycloak-access", "complete-keycloak-access"]
    );
    assert_eq!(
        provider.seen_web_device_tokens.lock().unwrap().as_slice(),
        [Some(DEVICE_OLD.to_owned()), Some(DEVICE_OLD.to_owned())]
    );
    let complete = store.load_session().await.unwrap().unwrap();
    assert_eq!(
        complete.tokens.access_token.expose_secret(),
        "complete-keycloak-access"
    );
    assert_eq!(
        complete.tokens.refresh_token.expose_secret(),
        "complete-keycloak-refresh"
    );
    assert_eq!(
        complete.tokens.web_sso_access_token.expose_secret(),
        "complete-websso-access"
    );
}

#[tokio::test]
async fn logout_clears_memory_storage_and_browser_state_even_when_upstream_fails() {
    let data_dir = TempDir::new().unwrap();
    let store = open_store(&data_dir).await;
    store.save_session(&persisted_session()).await.unwrap();
    let provider = Arc::new(FakeProviderAuthApi::idle());
    provider.logout_fails.store(true, Ordering::SeqCst);
    let sso = Arc::new(FakeSsoTransport::with_callback(bundle(
        "pending-access-secret",
        "pending-refresh-secret",
        "pending-websso-secret",
    )));
    let coordinator = restore_with(store.clone(), Arc::clone(&sso), Arc::clone(&provider)).await;

    let result = coordinator.logout().await.unwrap();

    assert_eq!(result.warning, Some(LogoutWarning::UpstreamLogoutFailed));
    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        serde_json::json!({"warning": "upstream_logout_failed"})
    );
    assert_eq!(coordinator.session().await, SessionView::anonymous());
    assert!(
        TokenSource::access_token(coordinator.as_ref())
            .await
            .is_none()
    );
    assert!(store.load_session().await.unwrap().is_none());
    assert!(sso.cleared.load(Ordering::SeqCst));
    assert_eq!(provider.logout_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        provider
            .seen_logout_refresh_tokens
            .lock()
            .unwrap()
            .as_slice(),
        [REFRESH_OLD]
    );
    assert_eq!(
        provider
            .seen_logout_device_tokens
            .lock()
            .unwrap()
            .as_slice(),
        [Some(DEVICE_OLD.to_owned())]
    );
    let output = format!("{coordinator:?} {result:?}");
    for secret in [
        ACCESS_OLD,
        REFRESH_OLD,
        WEB_SSO_OLD,
        DEVICE_OLD,
        "pending-access-secret",
        "pending-refresh-secret",
        "pending-websso-secret",
    ] {
        assert!(!output.contains(secret));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn auth_start_waits_for_in_progress_logout_cleanup_to_finish() {
    let data_dir = TempDir::new().unwrap();
    let store = open_store(&data_dir).await;
    store.save_session(&persisted_session()).await.unwrap();
    let entered = Arc::new(tokio::sync::Barrier::new(2));
    let release = Arc::new(tokio::sync::Barrier::new(2));
    let provider = Arc::new(
        FakeProviderAuthApi::idle()
            .with_first_logout_barriers(Arc::clone(&entered), Arc::clone(&release)),
    );
    let sso = Arc::new(FakeSsoTransport::without_callback());
    let coordinator = restore_with(store, Arc::clone(&sso), provider).await;
    let logout_coordinator = Arc::clone(&coordinator);
    let logout = tokio::spawn(async move { logout_coordinator.logout().await });
    entered.wait().await;

    let start_coordinator = Arc::clone(&coordinator);
    let start = tokio::spawn(async move { start_coordinator.start().await });
    tokio::task::yield_now().await;

    assert_eq!(sso.active_flows.load(Ordering::SeqCst), 0);
    release.wait().await;
    logout.await.unwrap().unwrap();
    start.await.unwrap().unwrap();
    assert_eq!(sso.active_flows.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn app_state_retains_the_coordinator_and_the_client_constructed_after_it() {
    let data_dir = TempDir::new().unwrap();
    let store = open_store(&data_dir).await;
    let coordinator = restore_with(
        store.clone(),
        Arc::new(FakeSsoTransport::without_callback()),
        Arc::new(FakeProviderAuthApi::idle()),
    )
    .await;
    let token_source: Arc<dyn TokenSource> = coordinator.clone();
    let base = Url::parse("http://127.0.0.1:1/").unwrap();
    let client = Arc::new(MtsClient::new_for_test(base.clone(), base, token_source).unwrap());

    let state = AppState::with_dependencies(store, coordinator.clone(), client.clone());

    assert!(Arc::ptr_eq(state.auth_coordinator().unwrap(), &coordinator));
    assert!(Arc::ptr_eq(state.mts_client().unwrap(), &client));
}

#[test]
fn progress_and_session_use_the_exact_provider_neutral_json_contract() {
    let authenticated_at = Utc.with_ymd_and_hms(2026, 8, 6, 9, 0, 0).unwrap();
    let session = SessionView {
        authenticated: true,
        status: SessionStatus::Active,
        authenticated_at: Some(authenticated_at),
        expires_at: None,
    };

    assert_eq!(
        serde_json::to_value(AuthProgress::Authenticated { session }).unwrap(),
        serde_json::json!({
            "status": "authenticated",
            "session": {
                "authenticated": true,
                "status": "active",
                "authenticated_at": "2026-08-06T09:00:00Z",
                "expires_at": null
            }
        })
    );

    assert_eq!(
        serde_json::to_value(SessionView::anonymous()).unwrap(),
        serde_json::json!({
            "authenticated": false,
            "status": "anonymous",
            "authenticated_at": null,
            "expires_at": null
        })
    );
    assert_eq!(
        serde_json::to_value(SessionView {
            authenticated: false,
            status: SessionStatus::Expired,
            authenticated_at: Some(authenticated_at),
            expires_at: Some(instant(10)),
        })
        .unwrap(),
        serde_json::json!({
            "authenticated": false,
            "status": "expired",
            "authenticated_at": "2026-08-06T09:00:00Z",
            "expires_at": "2026-08-06T10:00:00Z"
        })
    );
}

#[tokio::test]
async fn mts_auth_api_uses_exact_rotation_and_logout_contracts() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/user-service/api/v2/auth/refresh"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "accessToken": "access-fresh",
            "refreshToken": "refresh-fresh"
        })))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/user-service/api/auth/websso/refresh"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "accessToken": "websso-fresh"
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/user-service/api/v2/auth/logout"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    let api = MtsAuthApi::new_for_test(Url::parse(&format!("{}/", server.uri())).unwrap()).unwrap();

    let keycloak = api
        .rotate_keycloak(
            &SecretString::from("refresh-old"),
            Some(&SecretString::from("device-secret")),
        )
        .await
        .unwrap();
    let web_sso = api
        .rotate_web_sso(
            &keycloak.access_token,
            Some(&SecretString::from("device-secret")),
        )
        .await
        .unwrap();
    api.logout(
        &keycloak.refresh_token,
        Some(&SecretString::from("device-secret")),
    )
    .await
    .unwrap();

    assert_eq!(keycloak.access_token.expose_secret(), "access-fresh");
    assert_eq!(keycloak.refresh_token.expose_secret(), "refresh-fresh");
    assert_eq!(web_sso.expose_secret(), "websso-fresh");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&requests[0].body).unwrap(),
        serde_json::json!({"refreshToken": "refresh-old"})
    );
    assert_eq!(requests[1].body, b"");
    assert_eq!(requests[1].headers["authorization"], "Bearer access-fresh");
    for request in &requests {
        assert_eq!(request.headers["x-device-token"], "device-secret");
    }
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&requests[2].body).unwrap(),
        serde_json::json!({"refreshToken": "refresh-fresh"})
    );
    assert!(!requests[0].headers.contains_key("authorization"));
    assert!(!requests[2].headers.contains_key("authorization"));
    let request_ids = requests
        .iter()
        .map(|request| {
            assert_eq!(request.headers["x-device-type"], "ANDROID");
            Uuid::parse_str(request.headers["request-id"].to_str().unwrap()).unwrap()
        })
        .collect::<Vec<_>>();
    assert_ne!(request_ids[0], request_ids[1]);
    assert_ne!(request_ids[1], request_ids[2]);
}

#[tokio::test]
async fn mts_auth_api_rejects_partial_or_enveloped_rotation_responses() {
    for response in [
        serde_json::json!({"accessToken": "partial-secret"}),
        serde_json::json!({
            "data": {
                "accessToken": "enveloped-access-secret",
                "refreshToken": "enveloped-refresh-secret"
            }
        }),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/user-service/api/v2/auth/refresh"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .mount(&server)
            .await;
        let api =
            MtsAuthApi::new_for_test(Url::parse(&format!("{}/", server.uri())).unwrap()).unwrap();

        let error = match api
            .rotate_keycloak(&SecretString::from("request-refresh-secret"), None)
            .await
        {
            Ok(_) => panic!("partial rotation unexpectedly succeeded"),
            Err(error) => error,
        };

        assert_eq!(error.code, ErrorCode::UpstreamSchemaChanged);
        let output = format!("{error:?}");
        for secret in [
            "partial-secret",
            "enveloped-access-secret",
            "enveloped-refresh-secret",
            "request-refresh-secret",
        ] {
            assert!(!output.contains(secret));
        }
    }
}

#[tokio::test]
async fn mts_auth_api_rejects_enveloped_websso_responses_without_disclosing_them() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/user-service/api/auth/websso/refresh"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {"accessToken": "enveloped-websso-secret"}
        })))
        .mount(&server)
        .await;
    let api = MtsAuthApi::new_for_test(Url::parse(&format!("{}/", server.uri())).unwrap()).unwrap();

    let error = match api
        .rotate_web_sso(&SecretString::from("authorized-access-secret"), None)
        .await
    {
        Ok(_) => panic!("enveloped WebSSO rotation unexpectedly succeeded"),
        Err(error) => error,
    };

    assert_eq!(error.code, ErrorCode::UpstreamSchemaChanged);
    let output = format!("{error:?}");
    assert!(!output.contains("enveloped-websso-secret"));
    assert!(!output.contains("authorized-access-secret"));
}

#[tokio::test]
async fn mts_auth_api_bounds_responses_and_never_follows_redirects() {
    let redirect_server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/user-service/api/v2/auth/refresh"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Location", format!("{}/unexpected", redirect_server.uri())),
        )
        .mount(&redirect_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/unexpected"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "accessToken": "redirect-access-secret",
            "refreshToken": "redirect-refresh-secret"
        })))
        .mount(&redirect_server)
        .await;
    let api = MtsAuthApi::new_for_test(Url::parse(&format!("{}/", redirect_server.uri())).unwrap())
        .unwrap();
    let redirect_error = match api
        .rotate_keycloak(&SecretString::from("redirect-request-secret"), None)
        .await
    {
        Ok(_) => panic!("redirected rotation unexpectedly succeeded"),
        Err(error) => error,
    };
    assert_eq!(redirect_error.code, ErrorCode::UpstreamUnavailable);
    let requests = redirect_server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.path(), "/user-service/api/v2/auth/refresh");

    let bounded_server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/user-service/api/v2/auth/refresh"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            "oversized-response-secret{}",
            "x".repeat(70 * 1024)
        )))
        .mount(&bounded_server)
        .await;
    let bounded_api =
        MtsAuthApi::new_for_test(Url::parse(&format!("{}/", bounded_server.uri())).unwrap())
            .unwrap();
    let bounded_error = match bounded_api
        .rotate_keycloak(&SecretString::from("bounded-request-secret"), None)
        .await
    {
        Ok(_) => panic!("oversized rotation unexpectedly succeeded"),
        Err(error) => error,
    };
    assert_eq!(bounded_error.code, ErrorCode::UpstreamSchemaChanged);
    let output = format!("{bounded_error:?}");
    assert!(!output.contains("oversized-response-secret"));
    assert!(!output.contains("bounded-request-secret"));
}
