use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{Request, header},
};
use domru_mts::{
    app::build_router_with_config,
    config::AppConfig,
    domain::{
        AccessPoint, AccessPointKind, CallMode, Camera, CameraCapabilities, Capability,
        DeviceInventory, Flat, FlatId, ImageContentType, Lock, ProxiedMedia,
    },
    error::AppError,
    provider::mts::{
        AvailabilitySnapshot, PropertyProvider, ServiceSnapshot, intercom::DeviceProvider,
        media::SnapshotProvider,
    },
    service::{devices::DeviceService, media::MediaService, property::PropertyService},
    state::AppState,
    storage::SecretStore,
};
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

#[derive(Clone)]
struct TestPropertyProvider;

#[async_trait]
impl PropertyProvider for TestPropertyProvider {
    async fn flats(&self) -> Result<Vec<Flat>, AppError> {
        Ok(vec![Flat {
            id: FlatId("flat-owned".to_owned()),
            label: "Квартира 17".to_owned(),
            address: "Тестовая улица, 1".to_owned(),
            capabilities: domru_mts::domain::CapabilitySet {
                access_points: Capability::Available,
                cameras: Capability::Available,
                archive: unavailable(),
                events: unavailable(),
                calls: unavailable(),
                call_settings: unavailable(),
            },
        }])
    }

    async fn availability(&self, _flat: &FlatId) -> Result<AvailabilitySnapshot, AppError> {
        Err(AppError::invalid_request("not needed by this fixture"))
    }

    async fn services(&self, _flat: &FlatId) -> Result<ServiceSnapshot, AppError> {
        Err(AppError::invalid_request("not needed by this fixture"))
    }

    async fn call_settings(&self) -> Result<CallMode, AppError> {
        Ok(CallMode::Both)
    }

    async fn update_call_settings(&self, _mode: &CallMode) -> Result<(), AppError> {
        Ok(())
    }
}

#[derive(Clone)]
struct TestDeviceProvider;

#[async_trait]
impl DeviceProvider for TestDeviceProvider {
    async fn access_points(
        &self,
        flat: &FlatId,
        _category: Option<&str>,
    ) -> Result<Vec<AccessPoint>, AppError> {
        if flat.0 != "flat-owned" {
            return Ok(Vec::new());
        }
        Ok(vec![AccessPoint {
            id: "entrance-owned".to_owned(),
            flat_id: flat.clone(),
            name: "Подъезд".to_owned(),
            kind: AccessPointKind::Intercom,
            locks: vec![Lock {
                number: "1".to_owned(),
                name: "Дверь".to_owned(),
                permanently_open: Some(false),
            }],
            camera_ids: vec!["camera-owned".to_owned()],
            available: true,
        }])
    }

    async fn cameras(&self, flat: &FlatId) -> Result<Vec<Camera>, AppError> {
        if flat.0 != "flat-owned" {
            return Ok(Vec::new());
        }
        Ok(vec![Camera {
            id: "camera-owned".to_owned(),
            flat_id: flat.clone(),
            name: "Камера подъезда".to_owned(),
            access_point_id: Some("entrance-owned".to_owned()),
            capabilities: CameraCapabilities {
                snapshot: Capability::Available,
                live: unavailable(),
                archive: unavailable(),
            },
        }])
    }
}

#[derive(Clone)]
struct TestSnapshotProvider {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl SnapshotProvider for TestSnapshotProvider {
    async fn snapshot(&self, camera_id: &str) -> Result<ProxiedMedia, AppError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(camera_id, "camera-owned");
        Ok(ProxiedMedia::new(
            ImageContentType::Webp,
            bytes::Bytes::from_static(b"owned-webp"),
        ))
    }
}

fn unavailable() -> Capability {
    Capability::Unavailable { reason: None }
}

async fn app() -> (Router, TempDir, Arc<AtomicUsize>) {
    let directory = tempfile::tempdir().unwrap();
    let store = SecretStore::open(directory.path(), Some([9_u8; 32]))
        .await
        .unwrap();
    let property = Arc::new(PropertyService::new(Arc::new(TestPropertyProvider), store));
    let devices = Arc::new(DeviceService::new(Arc::new(TestDeviceProvider)));
    let calls = Arc::new(AtomicUsize::new(0));
    let media = Arc::new(MediaService::new(
        Arc::clone(&devices),
        Arc::new(TestSnapshotProvider {
            calls: Arc::clone(&calls),
        }),
    ));
    let state = AppState::with_core_services(property, devices, media);
    let config = AppConfig::from_iter(std::iter::empty::<(&str, &str)>()).unwrap();
    (build_router_with_config(state, &config), directory, calls)
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn returns_real_property_dashboard_and_inventory() {
    let (app, _directory, _calls) = app().await;
    let dashboard = app
        .clone()
        .oneshot(
            Request::get("/api/v1/dashboard")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(dashboard.status(), 200);
    let dashboard = body_json(dashboard).await;
    assert_eq!(dashboard["flats"][0]["id"], "flat-owned");
    assert_eq!(dashboard["flats"][0]["label"], "Квартира 17");
    assert_eq!(dashboard["callSettings"]["state"], "available");
    assert_eq!(dashboard["callSettings"]["settings"]["mode"], "both");

    let inventory = app
        .oneshot(
            Request::get("/api/v1/devices?flatId=flat-owned")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(inventory.status(), 200);
    let inventory = body_json(inventory).await;
    assert_eq!(inventory["accessPoints"][0]["id"], "entrance-owned");
    assert_eq!(inventory["accessPoints"][0]["locks"][0]["number"], "1");
    assert_eq!(inventory["cameras"][0]["id"], "camera-owned");
}

#[tokio::test]
async fn proxies_an_owned_snapshot_with_private_cache_headers() {
    let (app, _directory, calls) = app().await;
    let response = app
        .oneshot(
            Request::get("/api/v1/cameras/camera-owned/snapshot?flatId=flat-owned")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "image/webp");
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()[header::CONTENT_LENGTH], "10");
    assert_eq!(
        response.into_body().collect().await.unwrap().to_bytes(),
        "owned-webp"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn rejects_unowned_or_unbounded_snapshot_ids_before_provider_dispatch() {
    let (app, _directory, calls) = app().await;
    for uri in [
        "/api/v1/cameras/camera-other/snapshot?flatId=flat-owned".to_owned(),
        format!(
            "/api/v1/cameras/{}/snapshot?flatId=flat-owned",
            "x".repeat(513)
        ),
    ] {
        let response = app
            .clone()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(matches!(response.status().as_u16(), 400 | 404));
        let error = body_json(response).await;
        assert!(error["requestId"].as_str().is_some_and(|id| !id.is_empty()));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn rejects_an_unlinked_flat_before_returning_its_devices() {
    let (app, _directory, _calls) = app().await;
    let response = app
        .oneshot(
            Request::get("/api/v1/devices?flatId=flat-not-linked")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 404);
    assert_eq!(body_json(response).await["code"], "not_found");
}

#[tokio::test]
async fn keeps_nested_and_domru_z81_compatible_read_aliases_ownership_checked() {
    let (app, _directory, calls) = app().await;

    let nested = app
        .clone()
        .oneshot(
            Request::get("/api/v1/flats/flat-owned/devices")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(nested.status(), 200);

    let places = app
        .clone()
        .oneshot(Request::get("/api/places").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(places.status(), 200);
    assert_eq!(body_json(places).await["data"][0]["id"], "flat-owned");

    for (uri, key) in [
        ("/api/places/flat-owned/accesscontrols", "accessPoints"),
        ("/api/places/flat-owned/cameras", "cameras"),
    ] {
        let response = app
            .clone()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            body_json(response).await["data"][0]["id"],
            if key == "accessPoints" {
                "entrance-owned"
            } else {
                "camera-owned"
            }
        );
    }

    for uri in [
        "/api/snapshot/flat-owned/camera-owned",
        "/api/snapshot/camera-owned?flatId=flat-owned",
    ] {
        let response = app
            .clone()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            "owned-webp"
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn missing_runtime_services_fail_closed_with_an_error_envelope() {
    let config = AppConfig::from_iter(std::iter::empty::<(&str, &str)>()).unwrap();
    let app = build_router_with_config(AppState::default(), &config);
    let response = app
        .oneshot(
            Request::get("/api/v1/dashboard")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 500);
    assert_eq!(body_json(response).await["code"], "internal");
}

#[tokio::test]
async fn live_start_route_is_registered_and_fails_closed_without_services() {
    let config = AppConfig::from_iter(std::iter::empty::<(&str, &str)>()).unwrap();
    let response = build_router_with_config(AppState::default(), &config)
        .oneshot(
            Request::post("/api/v1/cameras/camera-owned/live?flatId=flat-owned")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 500);
    assert_eq!(body_json(response).await["code"], "internal");
}

#[test]
fn inventory_response_shape_stays_domain_native() {
    let inventory = DeviceInventory::default();
    assert_eq!(
        serde_json::to_value(inventory).unwrap(),
        serde_json::json!({"accessPoints": [], "cameras": []})
    );
}
