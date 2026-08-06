use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

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
    domain::auth::{AuthFlowId, AuthProgress, ChallengeInput, TokenBundle},
    error::AppError,
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
use tower::ServiceExt;
use url::Url;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, method, path},
};

const PHONE: &str = "+79990000000";
const OTP: &str = "123456";

struct NoSso;

#[async_trait]
impl SsoTransport for NoSso {
    async fn start(&self) -> Result<AuthProgress, AppError> {
        unreachable!()
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
    async fn logout(&self, _: &SecretString, _: Option<&SecretString>) -> Result<(), AppError> {
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
    _dir: TempDir,
}

async fn fixture(server: &MockServer) -> Fixture {
    let dir = TempDir::new().unwrap();
    let store = SecretStore::open(dir.path(), Some([93; 32])).await.unwrap();
    let provider = Arc::new(NoProvider {
        web_sso_calls: AtomicUsize::new(0),
    });
    let coordinator = AuthCoordinator::restore_for_test(
        store.clone(),
        Arc::new(NoSso),
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
    let state =
        AppState::with_dependencies(store.clone(), coordinator, client).with_legacy_sms_auth(sms);
    Fixture {
        app: build_router_with_config(state, &config),
        store,
        provider,
        _dir: dir,
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
    assert_eq!(fixture.provider.web_sso_calls.load(Ordering::SeqCst), 0);
}
