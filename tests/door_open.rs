use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    extract::Extension,
    http::{Method, Request, StatusCode, header},
    routing::post,
};
use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use domru_mts::{
    app::{RedactedUri, build_router_with_config, with_http_middleware},
    config::AppConfig,
    domain::{
        AccessPoint, AccessPointKind, CallMode, Camera, Capability, CapabilitySet, Flat, FlatId,
        Lock,
        door::{DoorOpenIntentId, DoorOpenIntentView},
    },
    error::{AppError, ErrorCode},
    provider::mts::{
        AvailabilitySnapshot, MtsClient, PropertyProvider, ServiceSnapshot, TokenSource,
        door::{DoorOpenProvider, MtsDoorOpenProvider},
        intercom::DeviceProvider,
    },
    service::{
        devices::DeviceService,
        door::{DoorOpenClock, DoorOpenService},
        media::MediaService,
        property::PropertyService,
    },
    state::AppState,
    storage::SecretStore,
};
use http_body_util::BodyExt;
use secrecy::SecretString;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::Notify;
use tower::ServiceExt;
use url::Url;
use uuid::Uuid;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

const INTERCOM_PREFIX: &str = "/vdome-smart-facade/devices/api/v1/intercoms";

#[derive(Clone, Copy)]
struct TestTokens;

#[async_trait]
impl TokenSource for TestTokens {
    async fn access_token(&self) -> Option<SecretString> {
        Some(SecretString::from("door-access-secret"))
    }

    async fn device_token(&self) -> Option<SecretString> {
        Some(SecretString::from("door-device-secret"))
    }

    async fn refresh_after_unauthorized(&self) -> Result<(), AppError> {
        panic!("unsafe door mutations must never refresh and retry")
    }
}

fn mts_client(server: &MockServer) -> MtsClient {
    let base = Url::parse(&format!("{}/", server.uri())).unwrap();
    MtsClient::new_for_test(base.clone(), base, Arc::new(TestTokens)).unwrap()
}

async fn mount_open_response(server: &MockServer, status: u16) {
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(status).set_body_string("upstream-private-marker"))
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn provider_posts_the_exact_encoded_normal_lock_contract_with_common_headers() {
    // Catches a wrong MTS route, missing segment encoding, wrong JSON action, or missing auth/device
    // headers while keeping the actual HTTP request builder in the test.
    let server = MockServer::start().await;
    mount_open_response(&server, 204).await;
    let provider = MtsDoorOpenProvider::new(mts_client(&server));

    provider.open("entry /%?# юг", "lock /%?# 1").await.unwrap();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method.as_str(), "POST");
    assert_eq!(
        request.url.path(),
        format!(
            "{INTERCOM_PREFIX}/entry%20%2F%25%3F%23%20%D1%8E%D0%B3/lock/lock%20%2F%25%3F%23%201"
        )
    );
    assert!(request.url.query().is_none());
    assert_eq!(request.body.as_slice(), br#"{"action":"open"}"#);
    assert_eq!(
        request.headers.get("content-type").unwrap(),
        "application/json"
    );
    assert_eq!(request.headers.get("accept").unwrap(), "application/json");
    assert_eq!(
        request.headers.get("authorization").unwrap(),
        "Bearer door-access-secret"
    );
    assert_eq!(
        request.headers.get("x-device-token").unwrap(),
        "door-device-secret"
    );
    assert_eq!(request.headers.get("x-device-type").unwrap(), "ANDROID");
    assert!(Uuid::parse_str(request.headers.get("request-id").unwrap().to_str().unwrap()).is_ok());
    assert!(request.headers.get("x-auth-token").is_none());
}

#[tokio::test]
async fn provider_dispatches_a_mutation_only_once_on_redirect_unauthorized_and_server_failure() {
    // Catches enabling redirects, 401 refresh retries, or transport retries for physical actions.
    for status in [302, 401, 503] {
        let server = MockServer::start().await;
        mount_open_response(&server, status).await;
        let provider = MtsDoorOpenProvider::new(mts_client(&server));

        let error = provider.open("entry-safe", "1").await.unwrap_err();

        assert!(!format!("{error:?}").contains("upstream-private-marker"));
        assert!(!error.retryable);
        if matches!(status, 302 | 503) {
            assert_eq!(error.code, ErrorCode::MutationOutcomeUnknown);
        } else {
            assert_eq!(error.code, ErrorCode::UpstreamAuthenticationFailed);
        }
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn provider_rejects_unbounded_or_invalid_ids_before_dispatch() {
    // Catches passing attacker-controlled empty/control/unbounded identifiers to the URL builder.
    for (access_point_id, lock_number) in [
        ("", "1"),
        ("entry", ""),
        (".", "1"),
        ("entry", ".."),
        ("line\nbreak", "1"),
        ("entry", "line\nbreak"),
        (&"x".repeat(513), "1"),
        ("entry", &"x".repeat(513)),
    ] {
        let server = MockServer::start().await;
        let provider = MtsDoorOpenProvider::new(mts_client(&server));

        let error = provider
            .open(access_point_id, lock_number)
            .await
            .unwrap_err();

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}

#[derive(Clone)]
struct TestPropertyProvider;

#[async_trait]
impl PropertyProvider for TestPropertyProvider {
    async fn flats(&self) -> Result<Vec<Flat>, AppError> {
        Ok(vec![Flat {
            id: FlatId("flat-owned".to_owned()),
            label: "Owned flat".to_owned(),
            address: "Private address".to_owned(),
            capabilities: CapabilitySet {
                access_points: Capability::Available,
                cameras: unavailable(),
                archive: unavailable(),
                events: unavailable(),
                calls: unavailable(),
                call_settings: unavailable(),
            },
        }])
    }

    async fn availability(&self, _flat: &FlatId) -> Result<AvailabilitySnapshot, AppError> {
        Err(AppError::invalid_request("not used"))
    }

    async fn services(&self, _flat: &FlatId) -> Result<ServiceSnapshot, AppError> {
        Err(AppError::invalid_request("not used"))
    }

    async fn call_settings(&self) -> Result<CallMode, AppError> {
        Err(AppError::invalid_request("not used"))
    }

    async fn update_call_settings(&self, _mode: &CallMode) -> Result<(), AppError> {
        Err(AppError::invalid_request("not used"))
    }
}

struct TestDeviceProvider {
    access_calls: Arc<AtomicUsize>,
    available: bool,
}

#[async_trait]
impl DeviceProvider for TestDeviceProvider {
    async fn access_points(
        &self,
        flat: &FlatId,
        _category: Option<&str>,
    ) -> Result<Vec<AccessPoint>, AppError> {
        self.access_calls.fetch_add(1, Ordering::SeqCst);
        if flat.0 != "flat-owned" {
            return Ok(Vec::new());
        }
        Ok(vec![AccessPoint {
            id: "entry-owned".to_owned(),
            flat_id: flat.clone(),
            name: "Owned entry".to_owned(),
            kind: AccessPointKind::Intercom,
            locks: vec![
                Lock {
                    number: "1".to_owned(),
                    name: "Door one".to_owned(),
                    permanently_open: Some(false),
                },
                Lock {
                    number: "2".to_owned(),
                    name: "Door two".to_owned(),
                    permanently_open: Some(false),
                },
            ],
            camera_ids: Vec::new(),
            available: self.available,
        }])
    }

    async fn cameras(&self, _flat: &FlatId) -> Result<Vec<Camera>, AppError> {
        panic!("door ownership must not depend on the unrelated camera source")
    }
}

struct RecordingDoorProvider {
    calls: AtomicUsize,
    started: Notify,
    release: Notify,
    block: bool,
}

impl RecordingDoorProvider {
    fn immediate() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            started: Notify::new(),
            release: Notify::new(),
            block: false,
        }
    }

    fn blocking() -> Self {
        Self {
            block: true,
            ..Self::immediate()
        }
    }
}

#[async_trait]
impl DoorOpenProvider for RecordingDoorProvider {
    async fn open(&self, access_point_id: &str, lock_number: &str) -> Result<(), AppError> {
        assert_eq!(access_point_id, "entry-owned");
        assert!(matches!(lock_number, "1" | "2"));
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        if self.block {
            self.release.notified().await;
        }
        Ok(())
    }
}

#[derive(Clone)]
struct TestClock(Arc<Mutex<DateTime<Utc>>>);

impl TestClock {
    fn at(value: DateTime<Utc>) -> Self {
        Self(Arc::new(Mutex::new(value)))
    }

    fn advance(&self, duration: TimeDelta) {
        let mut value = self.0.lock().unwrap();
        *value += duration;
    }
}

impl DoorOpenClock for TestClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}

struct ServiceFixture {
    service: Arc<DoorOpenService>,
    provider: Arc<RecordingDoorProvider>,
    clock: TestClock,
    access_calls: Arc<AtomicUsize>,
    _directory: TempDir,
}

async fn service_fixture(block: bool, max_pending: usize) -> ServiceFixture {
    service_fixture_with_availability(block, max_pending, true).await
}

async fn service_fixture_with_availability(
    block: bool,
    max_pending: usize,
    available: bool,
) -> ServiceFixture {
    let directory = tempfile::tempdir().unwrap();
    let store = SecretStore::open(directory.path(), Some([42; 32]))
        .await
        .unwrap();
    let property = Arc::new(PropertyService::new(Arc::new(TestPropertyProvider), store));
    let access_calls = Arc::new(AtomicUsize::new(0));
    let devices = Arc::new(DeviceService::new(Arc::new(TestDeviceProvider {
        access_calls: Arc::clone(&access_calls),
        available,
    })));
    let provider = Arc::new(if block {
        RecordingDoorProvider::blocking()
    } else {
        RecordingDoorProvider::immediate()
    });
    let clock = TestClock::at(Utc.with_ymd_and_hms(2026, 8, 6, 12, 0, 0).unwrap());
    let service = Arc::new(
        DoorOpenService::new_for_test(
            property,
            devices,
            provider.clone(),
            Arc::new(clock.clone()),
            Duration::from_secs(30),
            max_pending,
        )
        .unwrap(),
    );
    ServiceFixture {
        service,
        provider,
        clock,
        access_calls,
        _directory: directory,
    }
}

#[tokio::test]
async fn unavailable_access_points_cannot_issue_a_physical_action_intent() {
    // Catches ownership alone authorizing an action against a currently unavailable device.
    let fixture = service_fixture_with_availability(false, 8, false).await;

    let error = fixture
        .service
        .create_intent(&FlatId("flat-owned".to_owned()), "entry-owned", "1")
        .await
        .unwrap_err();

    assert_eq!(error.code, ErrorCode::NotFound);
    assert_eq!(fixture.provider.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn creating_an_intent_proves_flat_access_point_and_lock_without_opening_the_door() {
    // Catches any upstream door mutation during create and any missing linked-inventory proof.
    let fixture = service_fixture(false, 8).await;

    let intent = fixture
        .service
        .create_intent(&FlatId("flat-owned".to_owned()), "entry-owned", "1")
        .await
        .unwrap();

    assert_eq!(
        intent.expires_at,
        Utc.with_ymd_and_hms(2026, 8, 6, 12, 0, 30).unwrap()
    );
    assert_eq!(fixture.provider.calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.access_calls.load(Ordering::SeqCst), 1);

    let unlinked = fixture
        .service
        .create_intent(&FlatId("flat-other".to_owned()), "entry-owned", "1")
        .await
        .unwrap_err();
    assert_eq!(unlinked.code, ErrorCode::NotFound);
    assert_eq!(fixture.access_calls.load(Ordering::SeqCst), 1);

    for (access_point_id, lock_number) in [("entry-other", "1"), ("entry-owned", "9")] {
        let error = fixture
            .service
            .create_intent(
                &FlatId("flat-owned".to_owned()),
                access_point_id,
                lock_number,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::NotFound);
    }
    assert_eq!(fixture.provider.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn confirmation_consumes_once_before_dispatch_and_replay_is_stable() {
    // Catches a replayable intent or consumption after the physical mutation.
    let fixture = service_fixture(false, 8).await;
    let intent = fixture
        .service
        .create_intent(&FlatId("flat-owned".to_owned()), "entry-owned", "1")
        .await
        .unwrap();

    fixture.service.confirm(&intent.intent_id).await.unwrap();
    let replay = fixture
        .service
        .confirm(&intent.intent_id)
        .await
        .unwrap_err();

    assert_eq!(replay.code, ErrorCode::DoorOpenIntentNotFound);
    assert_eq!(fixture.provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn expired_intents_fail_closed_and_pending_capacity_is_bounded() {
    // Catches confirming after the deadline or an unbounded local pending-intent map.
    let fixture = service_fixture(false, 1).await;
    let first = fixture
        .service
        .create_intent(&FlatId("flat-owned".to_owned()), "entry-owned", "1")
        .await
        .unwrap();
    let capacity = fixture
        .service
        .create_intent(&FlatId("flat-owned".to_owned()), "entry-owned", "2")
        .await
        .unwrap_err();
    assert_eq!(capacity.code, ErrorCode::RateLimited);
    assert_eq!(fixture.provider.calls.load(Ordering::SeqCst), 0);

    fixture.clock.advance(TimeDelta::seconds(31));
    let expired = fixture.service.confirm(&first.intent_id).await.unwrap_err();
    assert_eq!(expired.code, ErrorCode::DoorOpenIntentNotFound);
    assert_eq!(fixture.provider.calls.load(Ordering::SeqCst), 0);

    fixture
        .service
        .create_intent(&FlatId("flat-owned".to_owned()), "entry-owned", "2")
        .await
        .unwrap();
}

#[tokio::test]
async fn parallel_confirmation_allows_only_one_upstream_mutation() {
    // Catches removing the intent after await, which lets concurrent confirmations both dispatch.
    let fixture = service_fixture(true, 8).await;
    let intent = fixture
        .service
        .create_intent(&FlatId("flat-owned".to_owned()), "entry-owned", "1")
        .await
        .unwrap();
    let service = Arc::clone(&fixture.service);
    let intent_id = intent.intent_id.clone();
    let first = tokio::spawn(async move { service.confirm(&intent_id).await });
    fixture.provider.started.notified().await;

    let concurrent = fixture
        .service
        .confirm(&intent.intent_id)
        .await
        .unwrap_err();
    assert_eq!(concurrent.code, ErrorCode::DoorOpenIntentNotFound);
    assert_eq!(fixture.provider.calls.load(Ordering::SeqCst), 1);

    fixture.provider.release.notify_waiters();
    first.await.unwrap().unwrap();
}

async fn api_fixture() -> (Router, Arc<RecordingDoorProvider>, TempDir) {
    let directory = tempfile::tempdir().unwrap();
    let store = SecretStore::open(directory.path(), Some([43; 32]))
        .await
        .unwrap();
    let property = Arc::new(PropertyService::new(
        Arc::new(TestPropertyProvider),
        store.clone(),
    ));
    let devices = Arc::new(DeviceService::new(Arc::new(TestDeviceProvider {
        access_calls: Arc::new(AtomicUsize::new(0)),
        available: true,
    })));
    let media = Arc::new(MediaService::new(
        Arc::clone(&devices),
        Arc::new(NeverSnapshotProvider),
    ));
    let provider = Arc::new(RecordingDoorProvider::immediate());
    let door = Arc::new(
        DoorOpenService::new_for_test(
            Arc::clone(&property),
            Arc::clone(&devices),
            provider.clone(),
            Arc::new(TestClock::at(
                Utc.with_ymd_and_hms(2026, 8, 6, 12, 0, 0).unwrap(),
            )),
            Duration::from_secs(30),
            8,
        )
        .unwrap(),
    );
    let state = AppState::with_core_services(property, devices, media).with_door_open_service(door);
    let config = AppConfig::from_iter(std::iter::empty::<(&str, &str)>()).unwrap();
    (
        build_router_with_config(state, &config),
        provider,
        directory,
    )
}

struct NeverSnapshotProvider;

#[async_trait]
impl domru_mts::provider::mts::media::SnapshotProvider for NeverSnapshotProvider {
    async fn snapshot(
        &self,
        _camera_id: &str,
    ) -> Result<domru_mts::domain::ProxiedMedia, AppError> {
        panic!("not used by door API tests")
    }
}

fn json_request(method: Method, uri: &str, body: impl Into<Body>) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .body(body.into())
        .unwrap()
}

async fn response_json(response: axum::response::Response) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap();
    (status, value)
}

async fn create_api_intent(app: &Router) -> DoorOpenIntentView {
    create_api_intent_at(app, "/api/v1/door-open-intents").await
}

async fn create_api_intent_at(app: &Router, uri: &str) -> DoorOpenIntentView {
    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            uri,
            Body::from(
                json!({
                    "flatId": "flat-owned",
                    "accessPointId": "entry-owned",
                    "lockNumber": "1"
                })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

fn serialized_intent_id(intent: &DoorOpenIntentView) -> String {
    serde_json::to_value(intent).unwrap()["intentId"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn api_requires_two_explicit_steps_and_returns_204_only_after_success() {
    // Catches accidental one-step opening, a leaky response, or a replayable API confirmation.
    let (app, provider, _directory) = api_fixture().await;

    let intent = create_api_intent(&app).await;
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        intent.expires_at,
        Utc.with_ymd_and_hms(2026, 8, 6, 12, 0, 30).unwrap()
    );
    let intent_id = serde_json::to_string(&intent.intent_id).unwrap();
    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/door-open-intents/confirm",
            Body::from(format!(r#"{{"intentId":{intent_id}}}"#)),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .is_empty()
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);

    let replay = app
        .oneshot(json_request(
            Method::POST,
            "/api/v1/door-open-intents/confirm",
            Body::from(format!(r#"{{"intentId":{intent_id}}}"#)),
        ))
        .await
        .unwrap();
    let (status, replay) = response_json(replay).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(replay["code"], "door_open_intent_not_found");
    assert!(
        replay["requestId"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn canonical_access_alias_creates_then_executes_the_same_one_use_intent() {
    // Catches drift from the stable Rust API routes or an execute alias bypassing consumption.
    let (app, provider, _directory) = api_fixture().await;
    let intent = create_api_intent_at(&app, "/api/v1/access/intents").await;
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    let intent_id = serialized_intent_id(&intent);

    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/api/v1/access/intents/{intent_id}/execute"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);

    let replay = app
        .oneshot(
            Request::post(format!("/api/v1/access/intents/{intent_id}/execute"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, replay) = response_json(replay).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(replay["code"], "door_open_intent_not_found");
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn open_door_compatibility_alias_preserves_create_then_confirm_semantics() {
    // Catches the legacy alias regressing into a dangerous one-call open operation.
    let (app, provider, _directory) = api_fixture().await;
    let intent = create_api_intent_at(&app, "/api/open-door").await;
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    let intent_id = serialized_intent_id(&intent);

    let response = app
        .oneshot(json_request(
            Method::POST,
            "/api/open-door",
            Body::from(json!({ "intentId": intent_id }).to_string()),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn api_rejects_unknown_ownership_and_malformed_or_unbounded_json_without_dispatch() {
    // Catches skipped ownership, permissive fields/media types, and unbounded body buffering.
    let (app, provider, _directory) = api_fixture().await;
    let cases = [
        (
            json_request(
                Method::POST,
                "/api/v1/door-open-intents",
                Body::from(
                    json!({
                        "flatId": "flat-owned",
                        "accessPointId": "entry-owned",
                        "lockNumber": "99"
                    })
                    .to_string(),
                ),
            ),
            StatusCode::NOT_FOUND,
            "not_found",
        ),
        (
            json_request(
                Method::POST,
                "/api/v1/door-open-intents",
                Body::from(r#"{"flatId":"flat-owned","accessPointId":"entry-owned"}"#),
            ),
            StatusCode::BAD_REQUEST,
            "invalid_request",
        ),
        (
            json_request(
                Method::POST,
                "/api/v1/door-open-intents",
                Body::from(
                    r#"{"flatId":"flat-owned","accessPointId":"entry-owned","lockNumber":"1","extra":true}"#,
                ),
            ),
            StatusCode::BAD_REQUEST,
            "invalid_request",
        ),
        (
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/door-open-intents")
                .header(header::CONTENT_TYPE, "text/plain")
                .body(Body::from("{}"))
                .unwrap(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
        ),
        (
            json_request(
                Method::POST,
                "/api/v1/door-open-intents",
                Body::from("x".repeat(4097)),
            ),
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
        ),
    ];

    for (request, expected_status, expected_code) in cases {
        let response = app.clone().oneshot(request).await.unwrap();
        let (status, body) = response_json(response).await;
        assert_eq!(status, expected_status);
        assert_eq!(body["code"], expected_code);
        assert!(body["requestId"].as_str().is_some_and(|id| !id.is_empty()));
    }
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn door_routes_reject_wrong_methods_with_the_standard_error_envelope() {
    // Catches framework-default text/empty 405 responses that break the API error contract.
    let (app, provider, _directory) = api_fixture().await;
    let response = app
        .oneshot(
            Request::get("/api/v1/door-open-intents")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let (status, body) = response_json(response).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(body["code"], "method_not_allowed");
    assert!(body["requestId"].as_str().is_some_and(|id| !id.is_empty()));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn tracing_redacts_every_query_value_on_physical_action_routes() {
    // Catches resource IDs being copied into request tracing through ignored query parameters.
    async fn rendered(Extension(uri): Extension<RedactedUri>) -> String {
        uri.to_string()
    }
    let app =
        with_http_middleware(Router::new().route("/api/v1/door-open-intents", post(rendered)));

    let response = app
        .oneshot(
            Request::post(
                "/api/v1/door-open-intents?flatId=flat-private&accessPointId=entry-private",
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();

    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"/api/v1/door-open-intents?[REDACTED]");
}

#[tokio::test]
async fn tracing_redacts_the_canonical_execute_intent_path_and_query() {
    // Catches the public confirmation token entering structured request logs through the path.
    async fn rendered(Extension(uri): Extension<RedactedUri>) -> String {
        uri.to_string()
    }
    let app = with_http_middleware(
        Router::new().route("/api/v1/access/intents/{intent_id}/execute", post(rendered)),
    );
    let marker = "00000000-0000-4000-8000-000000000123";

    let response = app
        .oneshot(
            Request::post(format!(
                "/api/v1/access/intents/{marker}/execute?flatId=flat-private"
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();

    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        &body[..],
        b"/api/v1/access/intents/[REDACTED]/execute?[REDACTED]"
    );
    assert!(!String::from_utf8_lossy(&body).contains(marker));
}

#[test]
fn intent_debug_output_never_contains_public_or_target_identifiers() {
    // Catches accidental logging of intent IDs through derived Debug output.
    let intent_id: DoorOpenIntentId = "00000000-0000-4000-8000-000000000123".parse().unwrap();
    let intent = DoorOpenIntentView {
        intent_id,
        expires_at: Utc.with_ymd_and_hms(2026, 8, 6, 12, 0, 30).unwrap(),
    };

    let debug = format!("{intent:?}");
    assert!(!debug.contains("00000000"));
    assert!(!debug.contains("entry-owned"));
}

fn unavailable() -> Capability {
    Capability::Unavailable { reason: None }
}
