use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use domru_mts::{
    app::build_router_with_config,
    auth::{AuthClock, AuthCoordinator, SsoTransport},
    config::AppConfig,
    domain::auth::{
        AuthFlavor, AuthFlowId, AuthProgress, ChallengeInput, PersistedSession, TokenBundle,
    },
    error::{AppError, ErrorCode},
    provider::mts::{
        KeycloakRotation, MtsClient, ProviderAuthApi, TokenSource,
        sms::{LegacySmsTestConfig, MtsLegacySmsAuth},
    },
    state::AppState,
    storage::SecretStore,
};
use http_body_util::BodyExt;
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::{Barrier, Notify};
use tower::ServiceExt;
use url::Url;
use wiremock::{
    Mock, MockServer, Respond, ResponseTemplate,
    matchers::{body_json, method, path},
};

const PHONE: &str = "+79990000000";
const OTP: &str = "123456";

struct TestSso {
    start_calls: AtomicUsize,
    start_entered: Option<Arc<Barrier>>,
    release_start: Option<Arc<Barrier>>,
}

#[async_trait]
impl SsoTransport for TestSso {
    async fn start(&self) -> Result<AuthProgress, AppError> {
        self.start_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(entered) = &self.start_entered {
            entered.wait().await;
        }
        if let Some(release) = &self.release_start {
            release.wait().await;
        }
        Ok(AuthProgress::BrowserRedirect {
            flow_id: "11111111-1111-4111-8111-111111111111".parse().unwrap(),
            authorization_url: Url::parse("https://login.mts.ru/authorize").unwrap(),
        })
    }
    async fn complete_callback(
        &self,
        _: &AuthFlowId,
        _: SecretString,
    ) -> Result<TokenBundle, AppError> {
        unreachable!()
    }
    async fn submit(&self, _: &AuthFlowId, _: ChallengeInput) -> Result<AuthProgress, AppError> {
        unreachable!()
    }
    fn clear(&self) {}
}

struct NoProvider {
    web_sso_calls: AtomicUsize,
    logout_entered: Option<Arc<Barrier>>,
    release_logout: Option<Arc<Barrier>>,
}

#[async_trait]
impl ProviderAuthApi for NoProvider {
    async fn rotate_keycloak(
        &self,
        _: &SecretString,
        _: Option<&SecretString>,
    ) -> Result<KeycloakRotation, AppError> {
        unreachable!()
    }
    async fn rotate_web_sso(
        &self,
        _: &SecretString,
        _: Option<&SecretString>,
    ) -> Result<SecretString, AppError> {
        self.web_sso_calls.fetch_add(1, Ordering::SeqCst);
        unreachable!()
    }
    async fn logout(
        &self,
        _: &SecretString,
        _: Option<&SecretString>,
        _: AuthFlavor,
    ) -> Result<(), AppError> {
        if let Some(entered) = &self.logout_entered {
            entered.wait().await;
        }
        if let Some(release) = &self.release_logout {
            release.wait().await;
        }
        Ok(())
    }
}

#[derive(Clone)]
struct FixedClock;

impl AuthClock for FixedClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-08-06T12:00:00Z")
            .unwrap()
            .to_utc()
    }
}

struct Fixture {
    app: Router,
    store: SecretStore,
    provider: Arc<NoProvider>,
    sso: Arc<TestSso>,
    coordinator: Arc<AuthCoordinator>,
    sms: Arc<MtsLegacySmsAuth>,
    _dir: TempDir,
}

async fn fixture(server: &MockServer) -> Fixture {
    fixture_with(
        server,
        Arc::new(TestSso {
            start_calls: AtomicUsize::new(0),
            start_entered: None,
            release_start: None,
        }),
        Arc::new(NoProvider {
            web_sso_calls: AtomicUsize::new(0),
            logout_entered: None,
            release_logout: None,
        }),
        None,
    )
    .await
}

async fn fixture_with(
    server: &MockServer,
    sso: Arc<TestSso>,
    provider: Arc<NoProvider>,
    initial_session: Option<PersistedSession>,
) -> Fixture {
    let dir = TempDir::new().unwrap();
    let store = SecretStore::open(dir.path(), Some([93; 32])).await.unwrap();
    if let Some(session) = initial_session {
        store.save_session(&session).await.unwrap();
    }
    let coordinator = AuthCoordinator::restore_for_test(
        store.clone(),
        sso.clone(),
        provider.clone(),
        Arc::new(FixedClock),
    )
    .await
    .unwrap();
    let sms = Arc::new(
        MtsLegacySmsAuth::new_for_test(LegacySmsTestConfig::new(
            Url::parse(&format!("{}/", server.uri())).unwrap(),
        ))
        .unwrap(),
    );
    let config = AppConfig::from_iter(std::iter::empty::<(&str, &str)>()).unwrap();
    let token_source: Arc<dyn TokenSource> = coordinator.clone();
    let client = Arc::new(MtsClient::new(&config, token_source).unwrap());
    let state = AppState::with_dependencies(store.clone(), coordinator.clone(), client)
        .with_legacy_sms_auth(sms.clone());
    Fixture {
        app: build_router_with_config(state, &config),
        store,
        provider,
        sso,
        coordinator,
        sms,
        _dir: dir,
    }
}

#[derive(Clone)]
struct DelayedResponse {
    entered: Arc<Notify>,
    response: ResponseTemplate,
    delay: Duration,
}

impl Respond for DelayedResponse {
    fn respond(&self, _request: &wiremock::Request) -> ResponseTemplate {
        self.entered.notify_one();
        self.response.clone().set_delay(self.delay)
    }
}

fn json_request(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

async fn json_response(response: axum::response::Response) -> (StatusCode, Value, String) {
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let raw = String::from_utf8(bytes.to_vec()).unwrap();
    (status, serde_json::from_str(&raw).unwrap(), raw)
}

async fn mount_successful_init(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/user-service/api/v2/auth/init"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "result": {
                "status": "SMS_SENT",
                "canSkipSms": false,
                "refreshToken": {"token": "init-secret", "expiredAt": 123, "timeZone": "Europe/Moscow"}
            }
        })))
        .mount(server)
        .await;
}

async fn confirm_flow(fixture: &Fixture, flow_id: &str) -> (StatusCode, Value, String) {
    let response = fixture
        .app
        .clone()
        .oneshot(json_request(
            &format!("/api/v1/auth/legacy-sms/flows/{flow_id}/confirm"),
            json!({"code": OTP}),
        ))
        .await
        .unwrap();
    json_response(response).await
}

async fn begin_legacy_flow(app: &Router) -> String {
    let started = app
        .clone()
        .oneshot(json_request(
            "/api/v1/auth/legacy-sms/start",
            json!({"phone": PHONE, "accept_legacy_sms": true}),
        ))
        .await
        .unwrap();
    let (status, body, _) = json_response(started).await;
    assert_eq!(status, StatusCode::OK);
    body["flow_id"].as_str().unwrap().to_owned()
}

fn seeded_web_sso_session() -> PersistedSession {
    PersistedSession {
        tokens: TokenBundle {
            access_token: SecretString::from("seed-access-secret"),
            refresh_token: SecretString::from("seed-refresh-secret"),
            web_sso_access_token: Some(SecretString::from("seed-websso-secret")),
            device_token: Some(SecretString::from("seed-device-secret")),
        },
        auth_flavor: AuthFlavor::WebSso,
        authenticated_at: FixedClock.now(),
    }
}

async fn login_request_count(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| request.url.path() == "/user-service/api/v2/auth/login")
        .count()
}

#[tokio::test]
async fn legacy_sms_is_explicit_and_init_uses_the_exact_packaged_contract() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/user-service/api/v2/auth/init"))
        .and(body_json(json!({"phone": PHONE})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "result": {
                "status": "SMS_SENT",
                "canSkipSms": false,
                "refreshToken": {"token": "init-secret", "expiredAt": 123, "timeZone": "Europe/Moscow"}
            }
        })))
        .expect(1)
        .mount(&server).await;
    let fixture = fixture(&server).await;

    let declined = fixture
        .app
        .clone()
        .oneshot(json_request(
            "/api/v1/auth/legacy-sms/start",
            json!({"phone": PHONE, "accept_legacy_sms": false}),
        ))
        .await
        .unwrap();
    assert_eq!(declined.status(), StatusCode::BAD_REQUEST);

    let started = fixture
        .app
        .clone()
        .oneshot(json_request(
            "/api/v1/auth/legacy-sms/start",
            json!({"phone": PHONE, "accept_legacy_sms": true}),
        ))
        .await
        .unwrap();
    let (status, body, raw) = json_response(started).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "challenge");
    assert_eq!(
        body["challenge"],
        json!({"type": "sms_otp", "resend_after_seconds": 60})
    );
    assert!(!raw.contains(PHONE));
    assert!(!raw.contains("init-secret"));

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].headers.get("authorization").is_none());
    assert!(requests[0].headers.get("x-device-token").is_none());
    assert_eq!(
        requests[0]
            .headers
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "application/json"
    );
}

#[tokio::test]
async fn six_digit_confirmation_installs_legacy_tokens_without_fabricating_web_sso() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/user-service/api/v2/auth/init"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "result": {
                "status": "SMS_SENT",
                "canSkipSms": false,
                "refreshToken": {"token": "init-secret", "expiredAt": 123, "timeZone": "Europe/Moscow"}
            }
        })))
        .expect(1)
        .mount(&server).await;
    Mock::given(method("POST"))
        .and(path("/user-service/api/v2/auth/login"))
        .and(body_json(json!({"phone": PHONE, "code": OTP})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "result": {"accessToken": "legacy-access-secret", "refreshToken": "legacy-refresh-secret"}
        })))
        .expect(1)
        .mount(&server).await;
    let fixture = fixture(&server).await;

    let started = fixture
        .app
        .clone()
        .oneshot(json_request(
            "/api/v1/auth/legacy-sms/start",
            json!({"phone": PHONE, "accept_legacy_sms": true}),
        ))
        .await
        .unwrap();
    let (_, body, _) = json_response(started).await;
    let flow_id = body["flow_id"].as_str().unwrap();
    let confirmed = fixture
        .app
        .clone()
        .oneshot(json_request(
            &format!("/api/v1/auth/legacy-sms/flows/{flow_id}/confirm"),
            json!({"code": OTP}),
        ))
        .await
        .unwrap();
    let (status, body, raw) = json_response(confirmed).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "authenticated");
    assert!(!raw.contains("secret"));

    let persisted = fixture.store.load_session().await.unwrap().unwrap();
    assert_eq!(
        persisted.tokens.access_token.expose_secret(),
        "legacy-access-secret"
    );
    assert_eq!(
        persisted.tokens.refresh_token.expose_secret(),
        "legacy-refresh-secret"
    );
    assert!(persisted.tokens.web_sso_access_token.is_none());
    assert_eq!(persisted.auth_flavor, AuthFlavor::LegacySms);
    assert_eq!(fixture.provider.web_sso_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn logout_waits_for_in_flight_legacy_confirmation_then_clears_the_winning_session() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/user-service/api/v2/auth/init"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "result": {
                "status": "SMS_SENT",
                "canSkipSms": false,
                "refreshToken": {"token": "init-secret", "expiredAt": 123, "timeZone": "Europe/Moscow"}
            }
        })))
        .mount(&server)
        .await;
    let login_entered = Arc::new(Notify::new());
    Mock::given(method("POST"))
        .and(path("/user-service/api/v2/auth/login"))
        .respond_with(DelayedResponse {
            entered: Arc::clone(&login_entered),
            response: ResponseTemplate::new(200).set_body_json(json!({
                "result": {
                    "accessToken": "legacy-access-secret",
                    "refreshToken": "legacy-refresh-secret"
                }
            })),
            delay: Duration::from_millis(200),
        })
        .mount(&server)
        .await;
    let fixture = fixture(&server).await;
    let flow_id = begin_legacy_flow(&fixture.app).await;

    let confirm_app = fixture.app.clone();
    let confirm = tokio::spawn(async move {
        confirm_app
            .oneshot(json_request(
                &format!("/api/v1/auth/legacy-sms/flows/{flow_id}/confirm"),
                json!({"code": OTP}),
            ))
            .await
            .unwrap()
    });
    login_entered.notified().await;

    let logout_app = fixture.app.clone();
    let mut logout = tokio::spawn(async move {
        logout_app
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/api/v1/session")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut logout)
            .await
            .is_err(),
        "logout must serialize behind the in-flight SMS verification"
    );

    assert_eq!(confirm.await.unwrap().status(), StatusCode::OK);
    assert_eq!(logout.await.unwrap().status(), StatusCode::OK);
    assert!(fixture.store.load_session().await.unwrap().is_none());
}

#[tokio::test]
async fn browser_auth_start_waits_for_in_flight_legacy_confirmation() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/user-service/api/v2/auth/init"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "result": {
                "status": "SMS_SENT",
                "canSkipSms": false,
                "refreshToken": {"token": "init-secret", "expiredAt": 123, "timeZone": "Europe/Moscow"}
            }
        })))
        .mount(&server)
        .await;
    let login_entered = Arc::new(Notify::new());
    Mock::given(method("POST"))
        .and(path("/user-service/api/v2/auth/login"))
        .respond_with(DelayedResponse {
            entered: Arc::clone(&login_entered),
            response: ResponseTemplate::new(200).set_body_json(json!({
                "result": {
                    "accessToken": "legacy-access-secret",
                    "refreshToken": "legacy-refresh-secret"
                }
            })),
            delay: Duration::from_millis(200),
        })
        .mount(&server)
        .await;
    let fixture = fixture(&server).await;
    let flow_id = begin_legacy_flow(&fixture.app).await;

    let confirm_app = fixture.app.clone();
    let confirm = tokio::spawn(async move {
        confirm_app
            .oneshot(json_request(
                &format!("/api/v1/auth/legacy-sms/flows/{flow_id}/confirm"),
                json!({"code": OTP}),
            ))
            .await
            .unwrap()
    });
    login_entered.notified().await;

    let start_app = fixture.app.clone();
    let mut start = tokio::spawn(async move {
        start_app
            .oneshot(json_request("/api/v1/auth/start", json!({})))
            .await
            .unwrap()
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut start)
            .await
            .is_err(),
        "browser auth must serialize behind the in-flight SMS verification"
    );
    assert_eq!(fixture.sso.start_calls.load(Ordering::SeqCst), 0);

    assert_eq!(confirm.await.unwrap().status(), StatusCode::OK);
    assert_eq!(start.await.unwrap().status(), StatusCode::OK);
    assert_eq!(fixture.sso.start_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn stale_legacy_confirmation_cannot_install_after_browser_auth_wins_the_lock() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/user-service/api/v2/auth/init"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "result": {
                "status": "SMS_SENT",
                "canSkipSms": false,
                "refreshToken": {"token": "init-secret", "expiredAt": 123, "timeZone": "Europe/Moscow"}
            }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/user-service/api/v2/auth/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "result": {
                "accessToken": "stale-access-secret",
                "refreshToken": "stale-refresh-secret"
            }
        })))
        .mount(&server)
        .await;
    let start_entered = Arc::new(Barrier::new(2));
    let release_start = Arc::new(Barrier::new(2));
    let fixture = fixture_with(
        &server,
        Arc::new(TestSso {
            start_calls: AtomicUsize::new(0),
            start_entered: Some(Arc::clone(&start_entered)),
            release_start: Some(Arc::clone(&release_start)),
        }),
        Arc::new(NoProvider {
            web_sso_calls: AtomicUsize::new(0),
            logout_entered: None,
            release_logout: None,
        }),
        None,
    )
    .await;
    let flow_id: AuthFlowId = begin_legacy_flow(&fixture.app).await.parse().unwrap();
    let confirmation = fixture.coordinator.prepare_legacy_confirmation().await;

    let start_coordinator = Arc::clone(&fixture.coordinator);
    let start = tokio::spawn(async move { start_coordinator.start().await });
    start_entered.wait().await;
    let confirm_coordinator = Arc::clone(&fixture.coordinator);
    let sms = Arc::clone(&fixture.sms);
    let confirm = tokio::spawn(async move {
        confirm_coordinator
            .complete_legacy_sms(confirmation, &sms, &flow_id, SecretString::from(OTP))
            .await
    });
    release_start.wait().await;

    start.await.unwrap().unwrap();
    let error = confirm.await.unwrap().unwrap_err();
    assert_eq!(error.code, ErrorCode::AuthFlowNotFound);
    assert_eq!(login_request_count(&server).await, 0);
    assert!(fixture.store.load_session().await.unwrap().is_none());
}

#[tokio::test]
async fn stale_legacy_confirmation_cannot_resurrect_a_logged_out_session() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/user-service/api/v2/auth/init"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "result": {
                "status": "SMS_SENT",
                "canSkipSms": false,
                "refreshToken": {"token": "init-secret", "expiredAt": 123, "timeZone": "Europe/Moscow"}
            }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/user-service/api/v2/auth/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "result": {
                "accessToken": "stale-access-secret",
                "refreshToken": "stale-refresh-secret"
            }
        })))
        .mount(&server)
        .await;
    let logout_entered = Arc::new(Barrier::new(2));
    let release_logout = Arc::new(Barrier::new(2));
    let fixture = fixture_with(
        &server,
        Arc::new(TestSso {
            start_calls: AtomicUsize::new(0),
            start_entered: None,
            release_start: None,
        }),
        Arc::new(NoProvider {
            web_sso_calls: AtomicUsize::new(0),
            logout_entered: Some(Arc::clone(&logout_entered)),
            release_logout: Some(Arc::clone(&release_logout)),
        }),
        Some(seeded_web_sso_session()),
    )
    .await;
    let flow_id: AuthFlowId = begin_legacy_flow(&fixture.app).await.parse().unwrap();
    let confirmation = fixture.coordinator.prepare_legacy_confirmation().await;

    let logout_coordinator = Arc::clone(&fixture.coordinator);
    let logout = tokio::spawn(async move { logout_coordinator.logout().await });
    logout_entered.wait().await;
    let confirm_coordinator = Arc::clone(&fixture.coordinator);
    let sms = Arc::clone(&fixture.sms);
    let confirm = tokio::spawn(async move {
        confirm_coordinator
            .complete_legacy_sms(confirmation, &sms, &flow_id, SecretString::from(OTP))
            .await
    });
    release_logout.wait().await;

    logout.await.unwrap().unwrap();
    let error = confirm.await.unwrap().unwrap_err();
    assert_eq!(error.code, ErrorCode::AuthFlowNotFound);
    assert_eq!(login_request_count(&server).await, 0);
    assert!(fixture.store.load_session().await.unwrap().is_none());
}

#[tokio::test]
async fn otp_dispatch_is_limited_to_three_attempts_even_when_upstream_is_unavailable() {
    let server = MockServer::start().await;
    mount_successful_init(&server).await;
    Mock::given(method("POST"))
        .and(path("/user-service/api/v2/auth/login"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let fixture = fixture(&server).await;
    let flow_id = begin_legacy_flow(&fixture.app).await;

    for _ in 0..3 {
        let (status, body, _) = confirm_flow(&fixture, &flow_id).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["code"], "upstream_unavailable");
    }
    let (status, body, _) = confirm_flow(&fixture, &flow_id).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "auth_flow_not_found");
    assert_eq!(login_request_count(&server).await, 3);
}

#[tokio::test]
async fn otp_retry_after_is_kept_in_the_flow_and_blocks_an_immediate_repeat() {
    let server = MockServer::start().await;
    mount_successful_init(&server).await;
    Mock::given(method("POST"))
        .and(path("/user-service/api/v2/auth/login"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "17"))
        .mount(&server)
        .await;
    let fixture = fixture(&server).await;
    let flow_id = begin_legacy_flow(&fixture.app).await;

    let (first_status, first_body, _) = confirm_flow(&fixture, &flow_id).await;
    assert_eq!(first_status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(first_body["code"], "rate_limited");
    assert_eq!(first_body["retryAfterSeconds"], 17);

    let (second_status, second_body, _) = confirm_flow(&fixture, &flow_id).await;
    assert_eq!(second_status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(second_body["code"], "rate_limited");
    assert_eq!(second_body["retryAfterSeconds"], 17);
    assert_eq!(login_request_count(&server).await, 1);
}

#[tokio::test]
async fn legacy_sms_maps_only_auth_statuses_to_authentication_failure() {
    let cases = [
        (
            400,
            StatusCode::UNAUTHORIZED,
            "upstream_authentication_failed",
            None,
        ),
        (
            401,
            StatusCode::UNAUTHORIZED,
            "upstream_authentication_failed",
            None,
        ),
        (
            403,
            StatusCode::UNAUTHORIZED,
            "upstream_authentication_failed",
            None,
        ),
        (
            404,
            StatusCode::SERVICE_UNAVAILABLE,
            "upstream_unavailable",
            None,
        ),
        (
            405,
            StatusCode::SERVICE_UNAVAILABLE,
            "upstream_unavailable",
            None,
        ),
        (
            410,
            StatusCode::SERVICE_UNAVAILABLE,
            "upstream_unavailable",
            None,
        ),
        (
            422,
            StatusCode::SERVICE_UNAVAILABLE,
            "upstream_unavailable",
            None,
        ),
        (429, StatusCode::TOO_MANY_REQUESTS, "rate_limited", Some(23)),
    ];

    for (upstream_status, expected_status, expected_code, retry_after) in cases {
        let server = MockServer::start().await;
        let mut response = ResponseTemplate::new(upstream_status);
        if let Some(seconds) = retry_after {
            response = response.insert_header("Retry-After", seconds.to_string());
        }
        Mock::given(method("POST"))
            .and(path("/user-service/api/v2/auth/init"))
            .respond_with(response)
            .mount(&server)
            .await;
        let fixture = fixture(&server).await;

        let response = fixture
            .app
            .clone()
            .oneshot(json_request(
                "/api/v1/auth/legacy-sms/start",
                json!({"phone": PHONE, "accept_legacy_sms": true}),
            ))
            .await
            .unwrap();
        let (status, body, _) = json_response(response).await;

        assert_eq!(status, expected_status, "upstream status {upstream_status}");
        assert_eq!(
            body["code"], expected_code,
            "upstream status {upstream_status}"
        );
        assert_eq!(
            body.get("retryAfterSeconds").and_then(Value::as_u64),
            retry_after,
            "upstream status {upstream_status}"
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn failed_init_post_is_dispatched_exactly_once() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/user-service/api/v2/auth/init"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let fixture = fixture(&server).await;

    let response = fixture
        .app
        .clone()
        .oneshot(json_request(
            "/api/v1/auth/legacy-sms/start",
            json!({"phone": PHONE, "accept_legacy_sms": true}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}
