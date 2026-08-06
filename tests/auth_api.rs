use std::{
    io::Write,
    str::FromStr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{Method, Request, Response, StatusCode, header},
};
use chrono::{DateTime, TimeZone, Utc};
use domru_mts::{
    app::build_router_with_config,
    auth::{AuthClock, AuthCoordinator, SsoTransport},
    config::AppConfig,
    domain::auth::{
        AuthFlavor, AuthFlowId, AuthProgress, ChallengeInput, PersistedSession, TokenBundle,
    },
    error::{AppError, ErrorCode},
    provider::mts::{KeycloakRotation, MtsClient, ProviderAuthApi, TokenSource},
    state::AppState,
    storage::SecretStore,
};
use http_body_util::BodyExt;
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;
use tracing_subscriber::fmt::MakeWriter;
use url::Url;

const FLOW_ID_HYPHENATED: &str = "00000000-0000-4000-8000-000000000001";
const FLOW_ID_WIRE: &str = "00000000000040008000000000000001";
const CALLBACK_SECRET: &str = "https://gateway.vdome.mts.ru/user-service/api/auth/callback?code=callback-code-secret&state=callback-state-secret";
const ACCESS_SECRET: &str = "api-access-secret";
const REFRESH_SECRET: &str = "api-refresh-secret";
const WEB_SSO_SECRET: &str = "api-websso-secret";
const DEVICE_SECRET: &str = "api-device-secret";

static TRACING_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn instant(hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 6, hour, 0, 0).unwrap()
}

fn flow_id() -> AuthFlowId {
    AuthFlowId::from_str(FLOW_ID_HYPHENATED).unwrap()
}

fn token_bundle() -> TokenBundle {
    TokenBundle {
        access_token: SecretString::from(ACCESS_SECRET),
        refresh_token: SecretString::from(REFRESH_SECRET),
        web_sso_access_token: Some(SecretString::from(WEB_SSO_SECRET)),
        device_token: Some(SecretString::from(DEVICE_SECRET)),
    }
}

#[derive(Clone)]
struct FixedClock(DateTime<Utc>);

impl AuthClock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

struct FakeSsoTransport {
    active: AtomicBool,
    callback_calls: AtomicUsize,
    submit_calls: AtomicUsize,
}

impl FakeSsoTransport {
    fn new() -> Self {
        Self {
            active: AtomicBool::new(false),
            callback_calls: AtomicUsize::new(0),
            submit_calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl SsoTransport for FakeSsoTransport {
    async fn start(&self) -> Result<AuthProgress, AppError> {
        self.active.store(true, Ordering::SeqCst);
        Ok(AuthProgress::BrowserRedirect {
            flow_id: flow_id(),
            authorization_url: Url::parse(
                "https://login.mts.ru/amserver/oauth2/authorize?client_id=VDome",
            )
            .unwrap(),
        })
    }

    async fn complete_callback(
        &self,
        requested_flow: &AuthFlowId,
        raw_callback: SecretString,
    ) -> Result<TokenBundle, AppError> {
        self.callback_calls.fetch_add(1, Ordering::SeqCst);
        if requested_flow != &flow_id() || !self.active.swap(false, Ordering::SeqCst) {
            return Err(inactive_flow());
        }
        if raw_callback.expose_secret() != CALLBACK_SECRET {
            return Err(AppError::invalid_request("Synthetic callback was invalid."));
        }
        Ok(token_bundle())
    }

    async fn submit(
        &self,
        requested_flow: &AuthFlowId,
        _input: ChallengeInput,
    ) -> Result<AuthProgress, AppError> {
        self.submit_calls.fetch_add(1, Ordering::SeqCst);
        if requested_flow != &flow_id() || !self.active.load(Ordering::SeqCst) {
            return Err(inactive_flow());
        }
        Err(AppError::new(
            ErrorCode::UpstreamSchemaChanged,
            "MTS browser challenge capture is required before challenge submission is supported.",
            false,
        ))
    }

    fn clear(&self) {
        self.active.store(false, Ordering::SeqCst);
    }
}

struct FakeProviderAuthApi {
    keycloak_calls: AtomicUsize,
    web_sso_calls: AtomicUsize,
    logout_calls: AtomicUsize,
}

impl FakeProviderAuthApi {
    fn new() -> Self {
        Self {
            keycloak_calls: AtomicUsize::new(0),
            web_sso_calls: AtomicUsize::new(0),
            logout_calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl ProviderAuthApi for FakeProviderAuthApi {
    async fn rotate_keycloak(
        &self,
        refresh_token: &SecretString,
        device_token: Option<&SecretString>,
    ) -> Result<KeycloakRotation, AppError> {
        assert_eq!(refresh_token.expose_secret(), REFRESH_SECRET);
        assert_eq!(
            device_token.map(ExposeSecret::expose_secret),
            Some(DEVICE_SECRET)
        );
        self.keycloak_calls.fetch_add(1, Ordering::SeqCst);
        Ok(KeycloakRotation {
            access_token: SecretString::from("rotated-api-access-secret"),
            refresh_token: SecretString::from("rotated-api-refresh-secret"),
        })
    }

    async fn rotate_web_sso(
        &self,
        access_token: &SecretString,
        device_token: Option<&SecretString>,
    ) -> Result<SecretString, AppError> {
        assert_eq!(access_token.expose_secret(), "rotated-api-access-secret");
        assert_eq!(
            device_token.map(ExposeSecret::expose_secret),
            Some(DEVICE_SECRET)
        );
        self.web_sso_calls.fetch_add(1, Ordering::SeqCst);
        Ok(SecretString::from("rotated-api-websso-secret"))
    }

    async fn logout(
        &self,
        refresh_token: &SecretString,
        device_token: Option<&SecretString>,
        auth_flavor: AuthFlavor,
    ) -> Result<(), AppError> {
        assert!(
            [REFRESH_SECRET, "rotated-api-refresh-secret"].contains(&refresh_token.expose_secret())
        );
        assert_eq!(
            device_token.map(ExposeSecret::expose_secret),
            Some(DEVICE_SECRET)
        );
        assert_eq!(auth_flavor, AuthFlavor::WebSso);
        self.logout_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn inactive_flow() -> AppError {
    AppError::new(
        ErrorCode::SessionExpired,
        "Synthetic flow is unavailable.",
        false,
    )
}

struct TestFixture {
    app: Router,
    sso: Arc<FakeSsoTransport>,
    provider: Arc<FakeProviderAuthApi>,
    _data_dir: TempDir,
}

async fn fixture(active_session: bool) -> TestFixture {
    let data_dir = TempDir::new().unwrap();
    let store = SecretStore::open(data_dir.path(), Some([81; 32]))
        .await
        .unwrap();
    if active_session {
        store
            .save_session(&PersistedSession {
                tokens: token_bundle(),
                auth_flavor: AuthFlavor::WebSso,
                authenticated_at: instant(9),
            })
            .await
            .unwrap();
    }
    let sso = Arc::new(FakeSsoTransport::new());
    let provider = Arc::new(FakeProviderAuthApi::new());
    let coordinator = AuthCoordinator::restore_for_test(
        store.clone(),
        sso.clone(),
        provider.clone(),
        Arc::new(FixedClock(instant(10))),
    )
    .await
    .unwrap();
    let config = AppConfig::from_iter(std::iter::empty::<(&str, &str)>()).unwrap();
    let token_source: Arc<dyn TokenSource> = coordinator.clone();
    let client = Arc::new(MtsClient::new(&config, token_source).unwrap());
    let state = AppState::with_dependencies(store, coordinator, client);
    let app = build_router_with_config(state, &config);

    TestFixture {
        app,
        sso,
        provider,
        _data_dir: data_dir,
    }
}

fn request(method: Method, uri: &str, body: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    builder
        .body(Body::from(body.unwrap_or_default().to_owned()))
        .unwrap()
}

async fn response_json(response: Response<Body>) -> (StatusCode, http::HeaderMap, Value, String) {
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let raw = String::from_utf8(bytes.to_vec()).unwrap();
    let value = serde_json::from_str(&raw).unwrap_or_else(|error| {
        panic!("response was not JSON: {error}; status={status}; body={raw:?}")
    });
    (status, headers, value, raw)
}

async fn start(app: &Router) -> Value {
    let response = app
        .clone()
        .oneshot(request(Method::POST, "/api/v1/auth/start", Some("{}")))
        .await
        .unwrap();
    let (status, _, body, _) = response_json(response).await;
    assert_eq!(status, StatusCode::OK);
    body
}

#[tokio::test]
async fn canonical_start_returns_the_exact_browser_redirect_contract() {
    let fixture = fixture(false).await;

    assert_eq!(
        start(&fixture.app).await,
        json!({
            "status": "browser_redirect",
            "flow_id": FLOW_ID_WIRE,
            "authorization_url": "https://login.mts.ru/amserver/oauth2/authorize?client_id=VDome"
        })
    );
}

#[tokio::test]
async fn callback_authenticates_once_and_session_exposes_only_public_metadata() {
    let fixture = fixture(false).await;
    start(&fixture.app).await;
    let uri = format!("/api/v1/auth/flows/{FLOW_ID_HYPHENATED}/callback");
    let body = serde_json::to_string(&json!({ "callback_url": CALLBACK_SECRET })).unwrap();

    let response = fixture
        .app
        .clone()
        .oneshot(request(Method::POST, &uri, Some(&body)))
        .await
        .unwrap();
    let (status, _, callback, raw) = response_json(response).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        callback,
        json!({
            "status": "authenticated",
            "session": {
                "authenticated": true,
                "status": "active",
                "authenticated_at": "2026-08-06T10:00:00Z",
                "expires_at": null
            }
        })
    );
    for secret in [
        CALLBACK_SECRET,
        ACCESS_SECRET,
        REFRESH_SECRET,
        WEB_SSO_SECRET,
        DEVICE_SECRET,
    ] {
        assert!(!raw.contains(secret));
    }

    let session = fixture
        .app
        .clone()
        .oneshot(request(Method::GET, "/api/v1/session", None))
        .await
        .unwrap();
    let (status, _, session, raw) = response_json(session).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(session, callback["session"]);
    assert!(!raw.contains("token"));

    let used = fixture
        .app
        .clone()
        .oneshot(request(Method::POST, &uri, Some(&body)))
        .await
        .unwrap();
    let (status, _, used, raw) = response_json(used).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(used["code"], "auth_flow_not_found");
    assert!(!raw.contains("callback-code-secret"));
    assert_eq!(fixture.sso.callback_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn refresh_and_logout_return_exact_public_json_and_update_session() {
    let fixture = fixture(true).await;

    let refresh = fixture
        .app
        .clone()
        .oneshot(request(Method::POST, "/api/v1/session/refresh", None))
        .await
        .unwrap();
    let (status, _, refresh, raw) = response_json(refresh).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        refresh,
        json!({
            "authenticated": true,
            "status": "active",
            "authenticated_at": "2026-08-06T09:00:00Z",
            "expires_at": null
        })
    );
    assert!(!raw.contains("rotated-api"));
    assert_eq!(fixture.provider.keycloak_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.provider.web_sso_calls.load(Ordering::SeqCst), 1);

    let logout = fixture
        .app
        .clone()
        .oneshot(request(Method::DELETE, "/api/v1/session", None))
        .await
        .unwrap();
    let (status, _, logout, raw) = response_json(logout).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(logout, json!({ "warning": null }));
    assert!(!raw.contains("secret"));
    assert_eq!(fixture.provider.logout_calls.load(Ordering::SeqCst), 1);

    let session = fixture
        .app
        .clone()
        .oneshot(request(Method::GET, "/api/v1/session", None))
        .await
        .unwrap();
    let (status, _, session, _) = response_json(session).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        session,
        json!({
            "authenticated": false,
            "status": "anonymous",
            "authenticated_at": null,
            "expires_at": null
        })
    );
}

#[tokio::test]
async fn unknown_and_malformed_flow_ids_have_distinct_stable_errors() {
    let fixture = fixture(false).await;
    let callback_body = serde_json::to_string(&json!({
        "callback_url": CALLBACK_SECRET
    }))
    .unwrap();

    let unknown = fixture
        .app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/auth/flows/00000000-0000-4000-8000-000000000099/callback",
            Some(&callback_body),
        ))
        .await
        .unwrap();
    let (status, _, unknown, _) = response_json(unknown).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(unknown["code"], "auth_flow_not_found");

    let malformed = fixture
        .app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/auth/flows/not-a-uuid/callback",
            Some(&callback_body),
        ))
        .await
        .unwrap();
    let (status, _, malformed, _) = response_json(malformed).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(malformed["code"], "invalid_request");
}

#[tokio::test]
async fn challenge_route_is_capture_required_and_never_calls_provider_auth() {
    let fixture = fixture(false).await;
    start(&fixture.app).await;

    for input in [
        json!({ "type": "phone", "phone": "+7 synthetic phone secret" }),
        json!({ "type": "sms_otp", "code": "synthetic otp secret" }),
    ] {
        let body = serde_json::to_string(&input).unwrap();
        let response = fixture
            .app
            .clone()
            .oneshot(request(
                Method::POST,
                &format!("/api/v1/auth/flows/{FLOW_ID_HYPHENATED}/challenge"),
                Some(&body),
            ))
            .await
            .unwrap();
        let (status, _, error, raw) = response_json(response).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(error["code"], "authentication_challenge_required");
        assert!(!raw.contains("synthetic"));
    }

    assert_eq!(fixture.sso.submit_calls.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.provider.keycloak_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.provider.web_sso_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.provider.logout_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn json_posts_require_content_type_and_enforce_body_and_callback_bounds() {
    let fixture = fixture(false).await;

    let missing_content_type = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/auth/start")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, _, error, _) = response_json(missing_content_type).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(error["code"], "unsupported_media_type");

    let malformed = fixture
        .app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/auth/start",
            Some("not-json"),
        ))
        .await
        .unwrap();
    let (status, _, error, _) = response_json(malformed).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "invalid_request");

    let oversized = serde_json::to_string(&json!({ "padding": "x".repeat(17 * 1024) })).unwrap();
    let oversized = fixture
        .app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/auth/start",
            Some(&oversized),
        ))
        .await
        .unwrap();
    let (status, _, error, _) = response_json(oversized).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(error["code"], "payload_too_large");

    let callback = "c".repeat(8 * 1024 + 1);
    let callback_body = serde_json::to_string(&json!({ "callback_url": callback })).unwrap();
    let callback = fixture
        .app
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/api/v1/auth/flows/{FLOW_ID_HYPHENATED}/callback"),
            Some(&callback_body),
        ))
        .await
        .unwrap();
    let (status, _, error, raw) = response_json(callback).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "invalid_request");
    assert!(!raw.contains(&"c".repeat(256)));
    assert_eq!(fixture.sso.callback_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn method_mismatches_use_the_stable_error_envelope() {
    let fixture = fixture(false).await;
    let response = fixture
        .app
        .clone()
        .oneshot(request(Method::GET, "/api/v1/auth/start", None))
        .await
        .unwrap();
    let (status, headers, error, _) = response_json(response).await;

    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(error["code"], "method_not_allowed");
    assert_eq!(
        error["requestId"],
        headers["x-request-id"].to_str().unwrap()
    );
}

#[tokio::test]
async fn compatibility_aliases_keep_only_safe_browser_flow_semantics() {
    let fixture = fixture(false).await;
    let login_secret = "legacy-phone-secret";
    let login_body = serde_json::to_string(&json!({ "phone": login_secret })).unwrap();
    let login = fixture
        .app
        .clone()
        .oneshot(request(Method::POST, "/api/login", Some(&login_body)))
        .await
        .unwrap();
    let (status, _, login, raw) = response_json(login).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(login["status"], "browser_redirect");
    assert!(!raw.contains(login_secret));

    let confirm_body = serde_json::to_string(&json!({
        "phone": login_secret,
        "code": "legacy-otp-secret"
    }))
    .unwrap();
    let confirm = fixture
        .app
        .clone()
        .oneshot(request(Method::POST, "/api/confirm", Some(&confirm_body)))
        .await
        .unwrap();
    let (status, _, confirm, raw) = response_json(confirm).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(confirm["code"], "authentication_challenge_required");
    assert!(!raw.contains("legacy"));
    assert_eq!(fixture.sso.submit_calls.load(Ordering::SeqCst), 0);

    let forbidden_callback = serde_json::to_string(&json!({
        "callback_url": CALLBACK_SECRET
    }))
    .unwrap();
    let confirm = fixture
        .app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/confirm",
            Some(&forbidden_callback),
        ))
        .await
        .unwrap();
    let (status, _, confirm, raw) = response_json(confirm).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(confirm["code"], "invalid_request");
    assert!(!raw.contains("callback-code-secret"));

    let session = fixture
        .app
        .clone()
        .oneshot(request(Method::GET, "/api/session", None))
        .await
        .unwrap();
    let (status, _, session, _) = response_json(session).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(session["authenticated"], false);

    let refresh = fixture
        .app
        .clone()
        .oneshot(request(Method::POST, "/api/refresh", None))
        .await
        .unwrap();
    let (status, _, refresh, _) = response_json(refresh).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(refresh["code"], "session_expired");

    let logout = fixture
        .app
        .clone()
        .oneshot(request(Method::POST, "/api/logout", None))
        .await
        .unwrap();
    let (status, _, logout, _) = response_json(logout).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(logout, json!({ "warning": null }));
}

#[tokio::test]
async fn errors_match_request_ids_and_never_log_or_echo_callback_material() {
    let _guard = TRACING_TEST_LOCK.lock().await;
    let fixture = fixture(false).await;
    let log_buffer = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(BufferWriter(log_buffer.clone()))
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .expect("auth API test installs one process-wide tracing subscriber");
    let request_id = "e7411f58-d5e9-4a43-b25d-88ddf2014713";
    let callback_body = serde_json::to_string(&json!({
        "callback_url": CALLBACK_SECRET
    }))
    .unwrap();

    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/v1/auth/flows/{FLOW_ID_HYPHENATED}/callback?code=query-code-secret&state=query-state-secret&callback_url=query-callback-secret&phone=query-phone-secret&password=query-password-secret&answer=query-answer-secret&future=query-future-secret"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-request-id", request_id)
                .body(Body::from(callback_body))
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, headers, error, raw) = response_json(response).await;

    let compatibility = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/login?phone=compat-phone-secret&password=compat-password-secret&answer=compat-answer-secret&future=compat-future-secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let logs = String::from_utf8(log_buffer.lock().unwrap().clone()).unwrap();

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(headers["x-request-id"], request_id);
    assert_eq!(error["requestId"], request_id);
    assert_eq!(error["code"], "auth_flow_not_found");
    assert_eq!(compatibility.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert!(logs.contains("/api/v1/auth/flows/"));
    assert!(logs.contains("/api/login"));
    assert!(logs.contains("[REDACTED]") || logs.contains("%5BREDACTED%5D"));
    for output in [&raw, &logs] {
        assert!(!output.contains("callback-code-secret"));
        assert!(!output.contains("callback-state-secret"));
        assert!(!output.contains(CALLBACK_SECRET));
        assert!(!output.contains("query-code-secret"));
        assert!(!output.contains("query-state-secret"));
        assert!(!output.contains("query-callback-secret"));
        assert!(!output.contains("query-phone-secret"));
        assert!(!output.contains("query-password-secret"));
        assert!(!output.contains("query-answer-secret"));
        assert!(!output.contains("query-future-secret"));
        assert!(!output.contains("compat-phone-secret"));
        assert!(!output.contains("compat-password-secret"));
        assert!(!output.contains("compat-answer-secret"));
        assert!(!output.contains("compat-future-secret"));
    }
}

#[derive(Clone)]
struct BufferWriter(Arc<Mutex<Vec<u8>>>);

impl Write for BufferWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'writer> MakeWriter<'writer> for BufferWriter {
    type Writer = Self;

    fn make_writer(&'writer self) -> Self::Writer {
        self.clone()
    }
}
