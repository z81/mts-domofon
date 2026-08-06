use std::{
    io::Write,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use chrono::{DateTime, TimeZone, Utc};
use domru_mts::{
    app::build_router_with_config,
    auth::{AuthClock, AuthCoordinator},
    config::AppConfig,
    domain::auth::{AuthFlavor, AuthProgress},
    error::{AppError, ErrorCode},
    provider::mts::{
        KeycloakRotation, MtsClient, ProviderAuthApi, TokenSource, sso::MtsSsoTransport,
    },
    state::AppState,
    storage::SecretStore,
};
use http_body_util::BodyExt;
use secrecy::{ExposeSecret, SecretString};
use tempfile::TempDir;
use tower::ServiceExt;
use tracing_subscriber::fmt::MakeWriter;

const IMPORT_JSON: &str = r#"{
  "webSsoAccessToken":"manual-web-sso",
  "idToken":"manual-id-token-discarded",
  "keycloakTokenPair":{
    "accessToken":"manual-access",
    "refreshToken":"manual-refresh",
    "expiresIn":"600"
  }
}"#;

#[derive(Clone)]
struct FixedClock(DateTime<Utc>);

impl AuthClock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

struct IdleProvider;

#[async_trait]
impl ProviderAuthApi for IdleProvider {
    async fn rotate_keycloak(
        &self,
        _refresh_token: &SecretString,
        _device_token: Option<&SecretString>,
    ) -> Result<KeycloakRotation, AppError> {
        Err(AppError::new(
            ErrorCode::Internal,
            "unused test provider",
            false,
        ))
    }

    async fn rotate_web_sso(
        &self,
        _access_token: &SecretString,
        _device_token: Option<&SecretString>,
    ) -> Result<SecretString, AppError> {
        Err(AppError::new(
            ErrorCode::Internal,
            "unused test provider",
            false,
        ))
    }

    async fn logout(
        &self,
        _refresh_token: &SecretString,
        _device_token: Option<&SecretString>,
        _auth_flavor: AuthFlavor,
    ) -> Result<(), AppError> {
        Ok(())
    }
}

async fn coordinator_fixture() -> (Arc<AuthCoordinator>, SecretStore, TempDir) {
    let data_dir = TempDir::new().unwrap();
    let store = SecretStore::open(data_dir.path(), Some([31; 32]))
        .await
        .unwrap();
    let coordinator = AuthCoordinator::restore_for_test(
        store.clone(),
        Arc::new(MtsSsoTransport::new("manual-import-device")),
        Arc::new(IdleProvider),
        Arc::new(FixedClock(
            Utc.with_ymd_and_hms(2026, 8, 6, 12, 0, 0).unwrap(),
        )),
    )
    .await
    .unwrap();
    (coordinator, store, data_dir)
}

async fn api_fixture() -> (Router, Arc<AuthCoordinator>, SecretStore, TempDir) {
    let (coordinator, store, data_dir) = coordinator_fixture().await;
    let config = AppConfig::from_iter(std::iter::empty::<(&str, &str)>()).unwrap();
    let token_source: Arc<dyn TokenSource> = coordinator.clone();
    let client = Arc::new(MtsClient::new(&config, token_source).unwrap());
    let state = AppState::with_dependencies(store.clone(), coordinator.clone(), client);
    (
        build_router_with_config(state, &config),
        coordinator,
        store,
        data_dir,
    )
}

async fn response_body(app: &Router, request: Request<Body>) -> (StatusCode, String) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

fn import_request(response: &str) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/web-sso/import")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({ "response": response }).to_string(),
        ))
        .unwrap()
}

#[tokio::test]
async fn direct_import_installs_retained_tokens_discards_id_token_and_preserves_session_on_rejection()
 {
    let (coordinator, store, data_dir) = coordinator_fixture().await;

    let progress = coordinator
        .import_web_sso_response(SecretString::from(IMPORT_JSON))
        .await
        .unwrap();
    assert!(matches!(progress, AuthProgress::Authenticated { .. }));

    let persisted = store.load_session().await.unwrap().unwrap();
    assert_eq!(
        persisted.tokens.access_token.expose_secret(),
        "manual-access"
    );
    assert_eq!(
        persisted.tokens.refresh_token.expose_secret(),
        "manual-refresh"
    );
    assert_eq!(
        persisted
            .tokens
            .web_sso_access_token
            .as_ref()
            .unwrap()
            .expose_secret(),
        "manual-web-sso"
    );
    assert!(persisted.tokens.device_token.is_none());
    let encrypted = tokio::fs::read(data_dir.path().join("session.enc"))
        .await
        .unwrap();
    let encrypted_text = String::from_utf8_lossy(&encrypted);
    for marker in [
        "manual-access",
        "manual-refresh",
        "manual-web-sso",
        "manual-id-token-discarded",
    ] {
        assert!(!encrypted_text.contains(marker));
    }

    for rejected in [
        "{not json}",
        r#"{"webSsoAccessToken":"manual-web-sso","idToken":"manual-id-token-discarded","keycloakTokenPair":{"accessToken":"","refreshToken":"manual-refresh"}}"#,
        r#"{"webSsoAccessToken":"manual-web-sso","idToken":"manual-id-token-discarded"}"#,
    ] {
        let error = coordinator
            .import_web_sso_response(SecretString::from(rejected))
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::UpstreamSchemaChanged);
        assert_eq!(
            tokio::fs::read(data_dir.path().join("session.enc"))
                .await
                .unwrap(),
            encrypted
        );
    }
}

#[tokio::test]
async fn import_api_enforces_the_outer_schema_and_never_echoes_or_logs_callback_secrets() {
    let (app, _coordinator, _store, data_dir) = api_fixture().await;
    let log_buffer = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(BufferWriter(log_buffer.clone()))
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .expect("import test installs one process-wide tracing subscriber");

    let (status, body) = response_body(&app, import_request(IMPORT_JSON)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("authenticated"));
    let encrypted = tokio::fs::read(data_dir.path().join("session.enc"))
        .await
        .unwrap();

    let malformed = "manual-malformed-secret";
    let cases = [
        import_request(malformed),
        Request::builder()
            .method(Method::POST)
            .uri("/api/v1/auth/web-sso/import")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::json!({ "response": IMPORT_JSON, "unexpected": "manual-unknown-field-secret" })
                    .to_string(),
            ))
            .unwrap(),
        import_request(&"x".repeat(64 * 1024 + 1)),
        Request::builder()
            .method(Method::POST)
            .uri("/api/v1/auth/web-sso/import")
            .header(header::CONTENT_TYPE, "text/plain")
            .body(Body::from("manual-content-type-secret"))
            .unwrap(),
    ];
    for request in cases {
        let (status, body) = response_body(&app, request).await;
        assert!(matches!(
            status,
            StatusCode::BAD_REQUEST
                | StatusCode::BAD_GATEWAY
                | StatusCode::PAYLOAD_TOO_LARGE
                | StatusCode::UNSUPPORTED_MEDIA_TYPE
        ));
        for marker in [
            "manual-web-sso",
            "manual-id-token-discarded",
            "manual-access",
            "manual-refresh",
            "manual-malformed-secret",
            "manual-unknown-field-secret",
            "manual-content-type-secret",
        ] {
            assert!(!body.contains(marker));
        }
        assert_eq!(
            tokio::fs::read(data_dir.path().join("session.enc"))
                .await
                .unwrap(),
            encrypted
        );
    }
    let logs = String::from_utf8(log_buffer.lock().unwrap().clone()).unwrap();
    for marker in [
        "manual-web-sso",
        "manual-id-token-discarded",
        "manual-access",
        "manual-refresh",
        "manual-malformed-secret",
        "manual-unknown-field-secret",
        "manual-content-type-secret",
    ] {
        assert!(!logs.contains(marker));
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
