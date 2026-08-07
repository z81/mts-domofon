use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Request, StatusCode, header},
};
use domru_mts::{
    api,
    app::with_http_middleware,
    domain::{
        AccessPoint, CallMode, Camera, CameraCapabilities, Capability, CapabilitySet, Flat, FlatId,
    },
    error::{AppError, ErrorCode},
    provider::mts::{
        AvailabilitySnapshot, PropertyProvider, ServiceSnapshot,
        intercom::{DeviceProvider, LiveSourceProvider},
    },
    service::{
        devices::DeviceService,
        live::{LiveProcess, LiveProcessRunner, LiveSessionManager},
        property::PropertyService,
    },
    state::AppState,
    storage::SecretStore,
};
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;
use uuid::Uuid;

use http_body_util::BodyExt;

const PRIVATE_SOURCE: &str = "rtsp://provider.invalid/private-source";

struct TestProperties {
    flats: Vec<Flat>,
}

#[async_trait]
impl PropertyProvider for TestProperties {
    async fn flats(&self) -> Result<Vec<Flat>, AppError> {
        Ok(self.flats.clone())
    }

    async fn availability(&self, _flat: &FlatId) -> Result<AvailabilitySnapshot, AppError> {
        Err(AppError::invalid_request("not used by stream fixture"))
    }

    async fn services(&self, _flat: &FlatId) -> Result<ServiceSnapshot, AppError> {
        Err(AppError::invalid_request("not used by stream fixture"))
    }

    async fn call_settings(&self) -> Result<CallMode, AppError> {
        Ok(CallMode::Both)
    }

    async fn update_call_settings(&self, _mode: &CallMode) -> Result<(), AppError> {
        Ok(())
    }
}

struct TestDevices {
    cameras: BTreeMap<FlatId, Vec<Camera>>,
}

#[async_trait]
impl DeviceProvider for TestDevices {
    async fn access_points(
        &self,
        _flat: &FlatId,
        _category: Option<&str>,
    ) -> Result<Vec<AccessPoint>, AppError> {
        Ok(Vec::new())
    }

    async fn cameras(&self, flat: &FlatId) -> Result<Vec<Camera>, AppError> {
        Ok(self.cameras.get(flat).cloned().unwrap_or_default())
    }
}

enum SourceResult {
    Ready,
    SessionExpired,
}

struct ExpectedLiveSource {
    expected_flat: FlatId,
    expected_camera: String,
    result: SourceResult,
    calls: AtomicUsize,
}

#[async_trait]
impl LiveSourceProvider for ExpectedLiveSource {
    async fn live_source(&self, flat: &FlatId, camera_id: &str) -> Result<SecretString, AppError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if flat != &self.expected_flat || camera_id != self.expected_camera {
            return Err(AppError::new(
                ErrorCode::Internal,
                "Default camera selection was incorrect.",
                false,
            ));
        }
        match self.result {
            SourceResult::Ready => Ok(SecretString::from(PRIVATE_SOURCE)),
            SourceResult::SessionExpired => Err(AppError::new(
                ErrorCode::SessionExpired,
                "Provider session expired.",
                false,
            )),
        }
    }
}

struct TestProcess;

#[async_trait]
impl LiveProcess for TestProcess {
    async fn stop(&mut self) {}

    async fn wait_for_exit(&mut self) {
        std::future::pending::<()>().await;
    }
}

#[derive(Default)]
struct TestRunner {
    starts: AtomicUsize,
}

#[async_trait]
impl LiveProcessRunner for TestRunner {
    async fn start(
        &self,
        source: &SecretString,
        output: &Path,
    ) -> Result<Box<dyn LiveProcess>, AppError> {
        if source.expose_secret() != PRIVATE_SOURCE {
            return Err(AppError::new(
                ErrorCode::Internal,
                "Live source was not kept inside the media pipeline.",
                false,
            ));
        }
        self.starts.fetch_add(1, Ordering::SeqCst);
        tokio::fs::write(
            output.join("index.m3u8"),
            b"#EXTM3U\n#EXTINF:1,\nsegment-000001.ts\n",
        )
        .await
        .map_err(|_| AppError::new(ErrorCode::Internal, "Fixture write failed.", false))?;
        tokio::fs::write(output.join("segment-000001.ts"), b"synthetic-ts")
            .await
            .map_err(|_| AppError::new(ErrorCode::Internal, "Fixture write failed.", false))?;
        Ok(Box::new(TestProcess))
    }
}

struct Fixture {
    app: Router,
    sessions: Arc<LiveSessionManager>,
    source: Arc<ExpectedLiveSource>,
    runner: Arc<TestRunner>,
    _directory: TempDir,
}

async fn fixture(
    flats: Vec<Flat>,
    cameras: BTreeMap<FlatId, Vec<Camera>>,
    selected_flat: Option<&str>,
    expected_flat: &str,
    expected_camera: &str,
    source_result: SourceResult,
) -> Fixture {
    let directory = tempfile::tempdir().expect("fixture directory");
    let store = SecretStore::open(directory.path(), Some([37_u8; 32]))
        .await
        .expect("fixture store");
    let properties = Arc::new(PropertyService::new(
        Arc::new(TestProperties { flats }),
        store,
    ));
    if let Some(flat_id) = selected_flat {
        properties
            .select_flat(FlatId(flat_id.to_owned()))
            .await
            .expect("selected fixture flat is linked");
    }
    let devices = Arc::new(DeviceService::new(Arc::new(TestDevices { cameras })));
    let source = Arc::new(ExpectedLiveSource {
        expected_flat: FlatId(expected_flat.to_owned()),
        expected_camera: expected_camera.to_owned(),
        result: source_result,
        calls: AtomicUsize::new(0),
    });
    let runner = Arc::new(TestRunner::default());
    let sessions = Arc::new(
        LiveSessionManager::new_for_test(runner.clone(), 2).expect("fixture live manager"),
    );
    let state = AppState::with_intercom_video_services(
        properties,
        devices,
        source.clone(),
        Arc::clone(&sessions),
    );

    Fixture {
        app: with_http_middleware(api::router().with_state(state)),
        sessions,
        source,
        runner,
        _directory: directory,
    }
}

fn flat(id: &str) -> Flat {
    Flat {
        id: FlatId(id.to_owned()),
        label: format!("Flat {id}"),
        address: "Fixture address".to_owned(),
        capabilities: CapabilitySet {
            access_points: Capability::Available,
            cameras: Capability::Available,
            archive: unavailable(),
            events: unavailable(),
            calls: unavailable(),
            call_settings: unavailable(),
        },
    }
}

fn camera(flat_id: &str, id: &str, live: Capability) -> Camera {
    Camera {
        id: id.to_owned(),
        flat_id: FlatId(flat_id.to_owned()),
        name: format!("Camera {id}"),
        access_point_id: None,
        capabilities: CameraCapabilities {
            snapshot: Capability::Available,
            live,
            archive: unavailable(),
        },
    }
}

fn unavailable() -> Capability {
    Capability::Unavailable { reason: None }
}

fn camera_map(entries: Vec<(&str, Vec<Camera>)>) -> BTreeMap<FlatId, Vec<Camera>> {
    entries
        .into_iter()
        .map(|(flat_id, cameras)| (FlatId(flat_id.to_owned()), cameras))
        .collect()
}

async fn response_bytes(response: axum::response::Response) -> bytes::Bytes {
    response
        .into_body()
        .collect()
        .await
        .expect("response body")
        .to_bytes()
}

async fn response_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(&response_bytes(response).await).expect("JSON response")
}

fn playlist_session_id(location: &str) -> Uuid {
    let raw = location
        .strip_prefix("/api/v1/live/")
        .and_then(|value| value.strip_suffix("/index.m3u8"))
        .expect("same-origin local playlist location");
    Uuid::parse_str(raw).expect("playlist session UUID")
}

fn assert_no_private_markers(headers: &HeaderMap, body: &[u8]) {
    let mut surface = String::from_utf8_lossy(body).into_owned();
    for value in headers.values() {
        if let Ok(value) = value.to_str() {
            surface.push_str(value);
        }
    }
    for marker in [
        PRIVATE_SOURCE,
        "provider-access-secret",
        "flat-a",
        "flat-b",
        "a-unavailable",
        "b-live",
        "z-live",
    ] {
        assert!(
            !surface.contains(marker),
            "response exposed private marker {marker}"
        );
    }
}

#[tokio::test]
async fn redirects_an_empty_request_to_the_first_flats_first_live_camera() {
    let fixture = fixture(
        vec![flat("flat-a"), flat("flat-b")],
        camera_map(vec![
            (
                "flat-a",
                vec![camera("flat-a", "first-live", Capability::Available)],
            ),
            (
                "flat-b",
                vec![camera("flat-b", "other-live", Capability::Available)],
            ),
        ]),
        None,
        "flat-a",
        "first-live",
        SourceResult::Ready,
    )
    .await;

    let first = fixture
        .app
        .clone()
        .oneshot(
            Request::get("/api/v1/cameras/stream")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("stream response");
    assert_eq!(first.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(
        first.headers().get(header::CACHE_CONTROL),
        Some(&"private, no-store".parse().expect("cache header"))
    );
    let first_headers = first.headers().clone();
    let first_location = first_headers
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .expect("redirect location")
        .to_owned();
    let first_session_id = playlist_session_id(&first_location);
    let first_body = response_bytes(first).await;
    assert!(first_body.is_empty());
    assert_no_private_markers(&first_headers, &first_body);

    let repeated = fixture
        .app
        .clone()
        .oneshot(
            Request::get("/api/v1/cameras/stream")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("repeated stream response");
    assert_eq!(repeated.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(
        repeated.headers().get(header::LOCATION),
        Some(&first_location.parse().expect("location header"))
    );
    assert_eq!(fixture.runner.starts.load(Ordering::SeqCst), 1);

    let playlist = fixture
        .app
        .clone()
        .oneshot(
            Request::get(&first_location)
                .body(Body::empty())
                .expect("playlist request"),
        )
        .await
        .expect("playlist response");
    assert_eq!(playlist.status(), StatusCode::OK);
    assert_eq!(
        playlist.headers().get(header::CACHE_CONTROL),
        Some(&"private, no-store".parse().expect("cache header"))
    );
    assert_eq!(
        response_bytes(playlist).await.as_ref(),
        b"#EXTM3U\n#EXTINF:1,\nsegment-000001.ts\n"
    );
    fixture
        .sessions
        .stop(first_session_id)
        .await
        .expect("stop live fixture");
}

#[tokio::test]
async fn selected_flat_wins_and_unavailable_cameras_are_skipped_in_id_order() {
    let fixture = fixture(
        vec![flat("flat-a"), flat("flat-b")],
        camera_map(vec![
            (
                "flat-a",
                vec![camera("flat-a", "other-live", Capability::Available)],
            ),
            (
                "flat-b",
                vec![
                    camera("flat-b", "z-live", Capability::Available),
                    camera("flat-b", "a-unavailable", unavailable()),
                    camera("flat-b", "b-live", Capability::Available),
                ],
            ),
        ]),
        Some("flat-b"),
        "flat-b",
        "b-live",
        SourceResult::Ready,
    )
    .await;

    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::get("/api/v1/cameras/stream")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("stream response");
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    let headers = response.headers().clone();
    let location = headers
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .expect("redirect location");
    let session_id = playlist_session_id(location);
    let body = response_bytes(response).await;
    assert_no_private_markers(&headers, &body);
    assert_eq!(fixture.source.calls.load(Ordering::SeqCst), 1);
    fixture
        .sessions
        .stop(session_id)
        .await
        .expect("stop live fixture");
}

#[tokio::test]
async fn selected_flat_without_live_video_does_not_fall_back_to_another_flat() {
    let fixture = fixture(
        vec![flat("flat-a"), flat("flat-b")],
        camera_map(vec![
            (
                "flat-a",
                vec![camera("flat-a", "other-live", Capability::Available)],
            ),
            (
                "flat-b",
                vec![camera("flat-b", "a-unavailable", unavailable())],
            ),
        ]),
        Some("flat-b"),
        "flat-b",
        "unused",
        SourceResult::Ready,
    )
    .await;

    let response = fixture
        .app
        .oneshot(
            Request::get("/api/v1/cameras/stream")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("stream response");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let error = response_json(response).await;
    assert_eq!(error["code"], "not_found");
    assert_eq!(error["message"], "Live video is unavailable.");
    assert_eq!(fixture.source.calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.runner.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn no_linked_flat_returns_the_stable_not_found_envelope() {
    let fixture = fixture(
        Vec::new(),
        BTreeMap::new(),
        None,
        "unused",
        "unused",
        SourceResult::Ready,
    )
    .await;

    let response = fixture
        .app
        .oneshot(
            Request::get("/api/v1/cameras/stream")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("stream response");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let error = response_json(response).await;
    assert_eq!(error["code"], "not_found");
    assert_eq!(error["message"], "Linked flat was not found.");
    assert_eq!(fixture.source.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn provider_session_errors_keep_their_existing_envelope() {
    let fixture = fixture(
        vec![flat("flat-a")],
        camera_map(vec![(
            "flat-a",
            vec![camera("flat-a", "first-live", Capability::Available)],
        )]),
        None,
        "flat-a",
        "first-live",
        SourceResult::SessionExpired,
    )
    .await;

    let response = fixture
        .app
        .oneshot(
            Request::get("/api/v1/cameras/stream")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("stream response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let headers = response.headers().clone();
    let body = response_bytes(response).await;
    let error: Value = serde_json::from_slice(&body).expect("error JSON");
    assert_eq!(error["code"], "session_expired");
    assert_eq!(error["message"], "Provider session expired.");
    assert_no_private_markers(&headers, &body);
    assert_eq!(fixture.source.calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.runner.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn non_get_method_uses_the_method_not_allowed_envelope() {
    let fixture = fixture(
        vec![flat("flat-a")],
        BTreeMap::new(),
        None,
        "unused",
        "unused",
        SourceResult::Ready,
    )
    .await;

    let response = fixture
        .app
        .oneshot(
            Request::post("/api/v1/cameras/stream")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("stream response");
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(response_json(response).await["code"], "method_not_allowed");
    assert_eq!(fixture.source.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn camera_specific_live_post_keeps_its_json_contract() {
    let fixture = fixture(
        vec![flat("flat-b")],
        camera_map(vec![(
            "flat-b",
            vec![camera("flat-b", "b-live", Capability::Available)],
        )]),
        None,
        "flat-b",
        "b-live",
        SourceResult::Ready,
    )
    .await;

    let response = fixture
        .app
        .oneshot(
            Request::post("/api/v1/cameras/b-live/live?flatId=flat-b")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("camera-specific live response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE),
        Some(&"application/json".parse().expect("content type"))
    );
    let headers = response.headers().clone();
    let body = response_bytes(response).await;
    assert_no_private_markers(&headers, &body);
    let json: Value = serde_json::from_slice(&body).expect("live session JSON");
    let session_id =
        Uuid::parse_str(json["sessionId"].as_str().expect("session ID")).expect("session UUID");
    assert_eq!(
        json["playlistUrl"],
        format!("/api/v1/live/{session_id}/index.m3u8")
    );
    assert_eq!(fixture.source.calls.load(Ordering::SeqCst), 1);
    fixture
        .sessions
        .stop(session_id)
        .await
        .expect("stop live fixture");
}
