use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use chrono::{TimeZone, Utc};
use domru_mts::{
    app::with_http_middleware,
    domain::{
        AccessPoint, CallMode, Camera, CameraCapabilities, Capability, CapabilitySet, Flat, FlatId,
        archive::ArchiveInterval,
    },
    error::{AppError, ErrorCode},
    provider::mts::{
        AvailabilitySnapshot, MtsClient, PropertyProvider, ServiceSnapshot, TokenSource,
        archive::{ArchiveProvider, MtsArchiveProvider},
        intercom::DeviceProvider,
    },
    service::{
        archive::ArchiveService,
        devices::DeviceService,
        live::{LiveProcess, LiveProcessRunner, LiveSessionManager},
        property::PropertyService,
    },
    state::AppState,
    storage::SecretStore,
};
use http_body_util::BodyExt;
use secrecy::{ExposeSecret, SecretString};
use serde_json::json;
use tower::ServiceExt;
use url::Url;
use uuid::Uuid;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

const CAMERA_PREFIX: &str = "/domofon/api/intercom/v1/cameras";

#[derive(Clone, Copy)]
struct TestTokens;

#[async_trait]
impl TokenSource for TestTokens {
    async fn access_token(&self) -> Option<SecretString> {
        Some(SecretString::from("archive-access-secret"))
    }

    async fn device_token(&self) -> Option<SecretString> {
        Some(SecretString::from("archive-device-secret"))
    }

    async fn refresh_after_unauthorized(&self) -> Result<(), AppError> {
        Err(AppError::new(
            ErrorCode::SessionExpired,
            "synthetic session expired",
            false,
        ))
    }
}

fn client(server: &MockServer) -> MtsClient {
    let base = Url::parse(&format!("{}/", server.uri())).unwrap();
    MtsClient::new_for_test(base.clone(), base, Arc::new(TestTokens)).unwrap()
}

#[tokio::test]
async fn provider_uses_exact_archive_contract_and_keeps_rtsp_secret() {
    // Catches path injection, wrong UTC formatting, choosing a foreign camera row, and exposing the
    // provider media URL through a public archive model.
    let server = MockServer::start().await;
    let camera_id = "camera /%?# юг";
    let encoded = "camera%20%2F%25%3F%23%20%D1%8E%D0%B3";
    Mock::given(method("GET"))
        .and(path(format!("{CAMERA_PREFIX}/{encoded}/archive/distribution")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "list": [
                {"id": "foreign-camera", "distribution": [{"from": "2026-08-07T06:00:00.000", "to": "2026-08-07T07:00:00.000"}]},
                {"id": camera_id, "distribution": [
                    {"from": "2026-08-07T08:00:00.000", "to": "2026-08-07T08:30:00.000"},
                    {"from": "2026-08-07T09:00:00.000", "to": "2026-08-07T09:15:00.000"}
                ]}
            ],
            "metadata": null,
            "error": null
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{CAMERA_PREFIX}/{encoded}/archive")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "list": [
                {"id": "foreign-camera", "rtsp": "rtsp://private.invalid/foreign"},
                {"id": camera_id, "rtsp": "rtsp://archive.invalid/private-owned-source"}
            ],
            "metadata": null,
            "error": null
        })))
        .expect(1)
        .mount(&server)
        .await;
    let provider = MtsArchiveProvider::new(client(&server));
    let start = Utc.with_ymd_and_hms(2026, 8, 7, 8, 5, 0).unwrap();
    let end = Utc.with_ymd_and_hms(2026, 8, 7, 8, 25, 0).unwrap();

    let distribution = provider.distribution(camera_id).await.unwrap();
    let source = provider.source(camera_id, start, end).await.unwrap();

    assert_eq!(
        distribution,
        vec![
            ArchiveInterval {
                from: Utc.with_ymd_and_hms(2026, 8, 7, 8, 0, 0).unwrap(),
                to: Utc.with_ymd_and_hms(2026, 8, 7, 8, 30, 0).unwrap(),
            },
            ArchiveInterval {
                from: Utc.with_ymd_and_hms(2026, 8, 7, 9, 0, 0).unwrap(),
                to: Utc.with_ymd_and_hms(2026, 8, 7, 9, 15, 0).unwrap(),
            },
        ]
    );
    assert_eq!(
        source.expose_secret(),
        "rtsp://archive.invalid/private-owned-source"
    );
    assert_eq!(format!("{source:?}"), "SecretBox<str>([REDACTED])");

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    let source_request = requests
        .iter()
        .find(|request| request.url.path().ends_with("/archive"))
        .unwrap();
    let query: std::collections::BTreeMap<_, _> =
        source_request.url.query_pairs().into_owned().collect();
    assert_eq!(
        query.get("start").map(String::as_str),
        Some("2026-08-07T08:05:00.000")
    );
    assert_eq!(
        query.get("end").map(String::as_str),
        Some("2026-08-07T08:25:00.000")
    );
    assert_eq!(
        source_request.headers.get("authorization").unwrap(),
        "Bearer archive-access-secret"
    );
    assert_eq!(
        source_request.headers.get("x-device-token").unwrap(),
        "archive-device-secret"
    );
    assert!(
        Uuid::parse_str(
            source_request
                .headers
                .get("request-id")
                .unwrap()
                .to_str()
                .unwrap()
        )
        .is_ok()
    );
}

#[derive(Clone)]
struct OwnedProperty;

#[async_trait]
impl PropertyProvider for OwnedProperty {
    async fn flats(&self) -> Result<Vec<Flat>, AppError> {
        Ok(vec![Flat {
            id: FlatId("flat-owned".to_owned()),
            label: "Owned flat".to_owned(),
            address: "Synthetic address".to_owned(),
            capabilities: CapabilitySet {
                access_points: Capability::Available,
                cameras: Capability::Available,
                archive: Capability::Available,
                events: Capability::Available,
                calls: Capability::Available,
                call_settings: Capability::Unavailable { reason: None },
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

struct OwnedCamera;

#[async_trait]
impl DeviceProvider for OwnedCamera {
    async fn access_points(
        &self,
        _flat: &FlatId,
        _category: Option<&str>,
    ) -> Result<Vec<AccessPoint>, AppError> {
        Ok(Vec::new())
    }

    async fn cameras(&self, flat: &FlatId) -> Result<Vec<Camera>, AppError> {
        if flat.0 != "flat-owned" {
            return Ok(Vec::new());
        }
        Ok(vec![Camera {
            id: "camera-owned".to_owned(),
            flat_id: flat.clone(),
            name: "Owned camera".to_owned(),
            access_point_id: None,
            capabilities: CameraCapabilities {
                snapshot: Capability::Available,
                live: Capability::Available,
                archive: Capability::Available,
            },
        }])
    }
}

type SourceRange = (String, chrono::DateTime<Utc>, chrono::DateTime<Utc>);

struct RecordingArchive {
    distribution_calls: AtomicUsize,
    source_ranges: Mutex<Vec<SourceRange>>,
}

#[async_trait]
impl ArchiveProvider for RecordingArchive {
    async fn distribution(&self, _camera_id: &str) -> Result<Vec<ArchiveInterval>, AppError> {
        self.distribution_calls.fetch_add(1, Ordering::SeqCst);
        Ok(vec![ArchiveInterval {
            from: Utc.with_ymd_and_hms(2026, 8, 7, 8, 0, 0).unwrap(),
            to: Utc.with_ymd_and_hms(2026, 8, 7, 9, 0, 0).unwrap(),
        }])
    }

    async fn source(
        &self,
        camera_id: &str,
        start: chrono::DateTime<Utc>,
        end: chrono::DateTime<Utc>,
    ) -> Result<SecretString, AppError> {
        self.source_ranges
            .lock()
            .unwrap()
            .push((camera_id.to_owned(), start, end));
        Ok(SecretString::from(
            "rtsp://archive.invalid/service-private-source",
        ))
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

struct RecordingRunner {
    sources: Mutex<Vec<String>>,
}

#[async_trait]
impl LiveProcessRunner for RecordingRunner {
    async fn start(
        &self,
        source: &SecretString,
        _output: &Path,
    ) -> Result<Box<dyn LiveProcess>, AppError> {
        self.sources
            .lock()
            .unwrap()
            .push(source.expose_secret().to_owned());
        Ok(Box::new(TestProcess))
    }
}

async fn archive_service(
    provider: Arc<RecordingArchive>,
    runner: Arc<RecordingRunner>,
) -> ArchiveService {
    let directory = tempfile::tempdir().unwrap();
    let store = SecretStore::open(directory.path(), Some([29; 32]))
        .await
        .unwrap();
    let properties = Arc::new(PropertyService::new(Arc::new(OwnedProperty), store));
    let devices = Arc::new(DeviceService::new(Arc::new(OwnedCamera)));
    let sessions = Arc::new(LiveSessionManager::new_for_test(runner, 2).unwrap());
    ArchiveService::new(properties, devices, provider, sessions)
}

#[tokio::test]
async fn service_proves_camera_ownership_and_clamps_playback_to_thirty_minutes() {
    // Catches bypassing camera ownership/capability, requesting outside the distribution, leaking
    // the RTSP source, or sending more than the bounded playback window upstream.
    let provider = Arc::new(RecordingArchive {
        distribution_calls: AtomicUsize::new(0),
        source_ranges: Mutex::new(Vec::new()),
    });
    let runner = Arc::new(RecordingRunner {
        sources: Mutex::new(Vec::new()),
    });
    let service = archive_service(Arc::clone(&provider), Arc::clone(&runner)).await;
    let start = Utc.with_ymd_and_hms(2026, 8, 7, 8, 5, 0).unwrap();

    let distribution = service
        .distribution(&FlatId("flat-owned".to_owned()), "camera-owned")
        .await
        .unwrap();
    let session = service
        .start(&FlatId("flat-owned".to_owned()), "camera-owned", start)
        .await
        .unwrap();

    assert_eq!(distribution.intervals.len(), 1);
    assert_eq!(provider.distribution_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        provider.source_ranges.lock().unwrap().as_slice(),
        &[(
            "camera-owned".to_owned(),
            start,
            Utc.with_ymd_and_hms(2026, 8, 7, 8, 35, 0).unwrap(),
        ),]
    );
    assert_eq!(
        runner.sources.lock().unwrap().as_slice(),
        &["rtsp://archive.invalid/service-private-source"]
    );
    let json = serde_json::to_string(&session).unwrap();
    assert_eq!(
        session.playlist_url,
        format!("/api/v1/live/{}/index.m3u8", session.session_id)
    );
    assert!(!json.contains("rtsp"));
    assert!(!json.contains("private"));

    let error = service
        .start(&FlatId("flat-owned".to_owned()), "camera-foreign", start)
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::NotFound);
    assert_eq!(provider.source_ranges.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn archive_routes_return_distribution_and_only_an_opaque_local_session() {
    // Catches missing routes, accepting archive POST without JSON, cacheable responses, and RTSP
    // leakage from either discovery or playback startup.
    let provider = Arc::new(RecordingArchive {
        distribution_calls: AtomicUsize::new(0),
        source_ranges: Mutex::new(Vec::new()),
    });
    let runner = Arc::new(RecordingRunner {
        sources: Mutex::new(Vec::new()),
    });
    let service = Arc::new(archive_service(provider, runner).await);
    let state = AppState::default().with_archive_service(service);
    let app = with_http_middleware(domru_mts::api::router().with_state(state));

    let distribution = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/cameras/camera-owned/archive/distribution?flatId=flat-owned")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(distribution.status(), StatusCode::OK);
    assert_eq!(
        distribution.headers().get(header::CACHE_CONTROL).unwrap(),
        "no-store"
    );
    let distribution_body = distribution.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&distribution_body).unwrap()["intervals"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let start = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/cameras/camera-owned/archive?flatId=flat-owned")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"start":"2026-08-07T08:05:00Z"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(start.status(), StatusCode::OK);
    assert_eq!(
        start.headers().get(header::CACHE_CONTROL).unwrap(),
        "no-store"
    );
    let start_body = start.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&start_body).unwrap();
    let session_id = json["sessionId"].as_str().unwrap();
    assert!(Uuid::parse_str(session_id).is_ok());
    assert_eq!(
        json["playlistUrl"],
        format!("/api/v1/live/{session_id}/index.m3u8")
    );
    let rendered = String::from_utf8(start_body.to_vec()).unwrap();
    assert!(!rendered.contains("rtsp"));
    assert!(!rendered.contains("private"));
}
