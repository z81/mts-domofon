use std::sync::Arc;

use async_trait::async_trait;
use domru_mts::{
    domain::{AccessPoint, AccessPointKind, Camera, CameraCapabilities, Capability, FlatId, Lock},
    error::{AppError, ErrorCode},
    provider::mts::{
        MtsClient, TokenSource,
        intercom::{DeviceProvider, MtsDeviceProvider},
    },
    service::devices::DeviceService,
};
use secrecy::SecretString;
use tokio::sync::Mutex;
use url::Url;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, query_param},
};

const ACCESS_PATH: &str = "/vdome-smart-facade/devices/api/v1/accessPoints";
const CAMERA_PATH: &str = "/vdome-smart-facade/devices/api/v1/cameras";

#[derive(Clone, Copy)]
struct TestTokens;

#[async_trait]
impl TokenSource for TestTokens {
    async fn access_token(&self) -> Option<SecretString> {
        Some(SecretString::from("access-test"))
    }

    async fn device_token(&self) -> Option<SecretString> {
        None
    }

    async fn refresh_after_unauthorized(&self) -> Result<(), AppError> {
        Err(AppError::new(
            ErrorCode::SessionExpired,
            "test session expired",
            false,
        ))
    }
}

fn client(server: &MockServer) -> MtsClient {
    let base = Url::parse(&format!("{}/", server.uri())).unwrap();
    MtsClient::new_for_test(base.clone(), base, Arc::new(TestTokens)).unwrap()
}

#[tokio::test]
async fn access_points_form_encode_scope_and_category_and_merge_pages_deterministically() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(ACCESS_PATH))
        .and(query_param("page", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            include_str!("../fixtures/access_points.json"),
            "application/json",
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(ACCESS_PATH))
        .and(query_param("page", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            include_str!("../fixtures/access_points_page_2.json"),
            "application/json",
        ))
        .mount(&server)
        .await;
    let provider = MtsDeviceProvider::new(client(&server));
    let flat = FlatId("flat /?#&+".to_owned());

    let points = provider
        .access_points(&flat, Some("gate / lobby&north"))
        .await
        .unwrap();

    assert_eq!(
        points
            .iter()
            .map(|point| point.id.as_str())
            .collect::<Vec<_>>(),
        vec!["access-a", "access-shared", "access-z"]
    );
    assert_eq!(points[0].flat_id, flat);
    assert_eq!(
        points[0].kind,
        AccessPointKind::Unknown("unknown".to_owned())
    );
    assert_eq!(
        points[1]
            .locks
            .iter()
            .map(|lock| lock.number.as_str())
            .collect::<Vec<_>>(),
        vec!["3", "4"]
    );
    assert_eq!(points[1].camera_ids, vec!["camera-archive"]);
    assert_eq!(
        points[2]
            .locks
            .iter()
            .map(|lock| lock.number.as_str())
            .collect::<Vec<_>>(),
        vec!["1", "2"]
    );
    assert!(!points[2].locks[1].permanently_open);

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].url.query(),
        Some(
            "flatId=flat+%2F%3F%23%26%2B&category=gate+%2F+lobby%26north&page=0&size=100&sort=name%2Casc"
        )
    );
    assert_eq!(
        requests[1].url.query(),
        Some(
            "flatId=flat+%2F%3F%23%26%2B&category=gate+%2F+lobby%26north&page=1&size=100&sort=name%2Casc"
        )
    );
}

#[tokio::test]
async fn cameras_map_independent_capabilities_without_disclosing_private_media_data() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(CAMERA_PATH))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(include_str!("../fixtures/cameras.json"), "application/json"),
        )
        .mount(&server)
        .await;
    let provider = MtsDeviceProvider::new(client(&server));
    let flat = FlatId("flat-synthetic".to_owned());

    let cameras = provider.cameras(&flat).await.unwrap();

    assert_eq!(
        cameras
            .iter()
            .map(|camera| camera.id.as_str())
            .collect::<Vec<_>>(),
        vec!["camera-archive", "camera-bare", "camera-live"]
    );
    assert_eq!(cameras[0].flat_id, flat);
    assert_eq!(cameras[0].capabilities.snapshot, Capability::Available);
    assert_eq!(cameras[0].capabilities.live, unavailable());
    assert_eq!(cameras[0].capabilities.archive, Capability::Available);
    assert_eq!(cameras[1].capabilities.snapshot, Capability::Available);
    assert_eq!(cameras[1].capabilities.live, unavailable());
    assert_eq!(cameras[1].capabilities.archive, unavailable());
    assert_eq!(cameras[2].access_point_id.as_deref(), Some("access-z"));
    assert_eq!(cameras[2].capabilities.live, Capability::Available);
    assert_eq!(cameras[2].capabilities.archive, unavailable());

    let stable_json = serde_json::to_string(&cameras).unwrap();
    let stable_debug = format!("{cameras:?}");
    for private in [
        "rtsp://media.invalid/live/private",
        "https://provider.invalid/private",
        "provider-camera-secret",
        "synthetic-provider-token-secret",
    ] {
        assert!(!stable_json.contains(private));
        assert!(!stable_debug.contains(private));
    }
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests[0].url.query(),
        Some("flatId=flat-synthetic&page=0&size=100&sort=name%2Casc")
    );
}

#[tokio::test]
async fn provider_rejects_contradictory_identity_and_bounded_pagination() {
    let server = MockServer::start().await;
    let contradictory = serde_json::json!({
        "content": [
            {"id": "same", "name": "First", "camera": null, "addresses": null, "locks": null},
            {"id": "same", "name": "Second", "camera": null, "addresses": null, "locks": null}
        ],
        "totalElements": 2, "totalPages": 1, "number": 0, "size": 100,
        "last": true, "first": true, "empty": false
    });
    Mock::given(method("GET"))
        .and(path(ACCESS_PATH))
        .and(query_param("flatId", "flat-contradictory"))
        .respond_with(ResponseTemplate::new(200).set_body_json(contradictory))
        .mount(&server)
        .await;
    let too_many_pages = serde_json::json!({
        "content": [], "totalElements": 0, "totalPages": 51,
        "number": 0, "size": 100, "last": false, "first": true, "empty": true
    });
    Mock::given(method("GET"))
        .and(path(CAMERA_PATH))
        .and(query_param("flatId", "flat-unbounded"))
        .respond_with(ResponseTemplate::new(200).set_body_json(too_many_pages))
        .mount(&server)
        .await;
    let provider = MtsDeviceProvider::new(client(&server));

    let identity_error = provider
        .access_points(&FlatId("flat-contradictory".to_owned()), None)
        .await
        .unwrap_err();
    let pagination_error = provider
        .cameras(&FlatId("flat-unbounded".to_owned()))
        .await
        .unwrap_err();

    assert_eq!(identity_error.code, ErrorCode::UpstreamSchemaChanged);
    assert_eq!(pagination_error.code, ErrorCode::UpstreamSchemaChanged);
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[derive(Clone, Copy)]
enum SourceState {
    Available,
    Unavailable,
}

struct ScopedProvider {
    access_state: SourceState,
    camera_state: SourceState,
    calls: Mutex<Vec<(String, &'static str, Option<String>)>>,
}

impl ScopedProvider {
    fn new(access_state: SourceState, camera_state: SourceState) -> Self {
        Self {
            access_state,
            camera_state,
            calls: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl DeviceProvider for ScopedProvider {
    async fn access_points(
        &self,
        flat: &FlatId,
        category: Option<&str>,
    ) -> Result<Vec<AccessPoint>, AppError> {
        self.calls
            .lock()
            .await
            .push((flat.0.clone(), "access", category.map(str::to_owned)));
        if category.is_some() {
            return Err(AppError::invalid_request(
                "inventory must not request a category filter",
            ));
        }
        if matches!(self.access_state, SourceState::Unavailable) {
            return Err(unavailable_error("access points"));
        }
        Ok(vec![AccessPoint {
            id: "access-owned".to_owned(),
            flat_id: FlatId("provider-overridden-flat".to_owned()),
            name: "Owned entrance".to_owned(),
            kind: AccessPointKind::Unknown("unknown".to_owned()),
            locks: vec![Lock {
                number: "1".to_owned(),
                name: "Door".to_owned(),
                permanently_open: false,
            }],
            camera_ids: vec!["camera-owned".to_owned()],
            available: true,
        }])
    }

    async fn cameras(&self, flat: &FlatId) -> Result<Vec<Camera>, AppError> {
        self.calls
            .lock()
            .await
            .push((flat.0.clone(), "camera", None));
        if matches!(self.camera_state, SourceState::Unavailable) {
            return Err(unavailable_error("cameras"));
        }
        let camera_id = if flat.0 == "flat-other" {
            "camera-other"
        } else {
            "camera-owned"
        };
        Ok(vec![Camera {
            id: camera_id.to_owned(),
            flat_id: FlatId("provider-overridden-flat".to_owned()),
            name: "Owned camera".to_owned(),
            access_point_id: None,
            capabilities: CameraCapabilities {
                snapshot: Capability::Available,
                live: unavailable(),
                archive: unavailable(),
            },
        }])
    }
}

#[tokio::test]
async fn inventory_stamps_ownership_associates_cameras_and_degrades_each_source_independently() {
    let requested = FlatId("flat-owned".to_owned());
    let provider = Arc::new(ScopedProvider::new(
        SourceState::Available,
        SourceState::Available,
    ));
    let service = DeviceService::new(provider.clone());

    let inventory = service.inventory(&requested).await.unwrap();

    assert_eq!(inventory.access_points[0].flat_id, requested);
    assert_eq!(inventory.cameras[0].flat_id, requested);
    assert_eq!(
        inventory.cameras[0].access_point_id.as_deref(),
        Some("access-owned")
    );
    let cameras_only = DeviceService::new(Arc::new(ScopedProvider::new(
        SourceState::Unavailable,
        SourceState::Available,
    )))
    .inventory(&requested)
    .await
    .unwrap();
    assert!(cameras_only.access_points.is_empty());
    assert_eq!(cameras_only.cameras.len(), 1);

    let access_only = DeviceService::new(Arc::new(ScopedProvider::new(
        SourceState::Available,
        SourceState::Unavailable,
    )))
    .inventory(&requested)
    .await
    .unwrap();
    assert_eq!(access_only.access_points.len(), 1);
    assert!(access_only.cameras.is_empty());
}

#[tokio::test]
async fn camera_lookup_never_searches_another_flat_and_returns_stable_not_found() {
    let provider = Arc::new(ScopedProvider::new(
        SourceState::Available,
        SourceState::Available,
    ));
    let service = DeviceService::new(provider.clone());

    let error = service
        .camera(&FlatId("flat-owned".to_owned()), "camera-other")
        .await
        .unwrap_err();

    assert_eq!(error.code, ErrorCode::NotFound);
    assert_eq!(
        error.to_string(),
        "Camera was not found for the requested flat."
    );
    assert!(
        provider
            .calls
            .lock()
            .await
            .iter()
            .all(|(flat, _, _)| flat == "flat-owned")
    );
}

#[tokio::test]
async fn both_inventory_sources_failing_returns_an_upstream_error() {
    let service = DeviceService::new(Arc::new(ScopedProvider::new(
        SourceState::Unavailable,
        SourceState::Unavailable,
    )));

    let error = service
        .inventory(&FlatId("flat-owned".to_owned()))
        .await
        .unwrap_err();

    assert_eq!(error.code, ErrorCode::UpstreamUnavailable);
}

fn unavailable() -> Capability {
    Capability::Unavailable { reason: None }
}

fn unavailable_error(source: &str) -> AppError {
    AppError::new(
        ErrorCode::UpstreamUnavailable,
        format!("{source} unavailable"),
        true,
    )
}
