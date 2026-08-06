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
    Mock, MockServer, Request, Respond, ResponseTemplate,
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

fn empty_page() -> serde_json::Value {
    serde_json::json!({
        "content": [],
        "totalElements": 0,
        "totalPages": 0,
        "number": 0,
        "size": 100,
        "last": true,
        "first": true,
        "empty": true
    })
}

#[tokio::test]
async fn access_points_form_encode_scope_and_category_and_merge_pages_deterministically() {
    let server = MockServer::start().await;
    let mut first_page: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/access_points.json")).unwrap();
    let second_page: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/access_points_page_2.json")).unwrap();
    first_page["content"]
        .as_array_mut()
        .unwrap()
        .push(second_page["content"][1].clone());
    while first_page["content"].as_array().unwrap().len() < 100 {
        first_page["content"]
            .as_array_mut()
            .unwrap()
            .push(second_page["content"][1].clone());
    }
    first_page["totalElements"] = serde_json::json!(101);
    let mut last_page = second_page;
    last_page["content"] = serde_json::json!([last_page["content"][0].clone()]);
    last_page["totalElements"] = serde_json::json!(101);
    Mock::given(method("GET"))
        .and(path(ACCESS_PATH))
        .and(query_param("page", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(first_page))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(ACCESS_PATH))
        .and(query_param("page", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(last_page))
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
    assert_eq!(points[2].locks[0].permanently_open, Some(false));
    assert_eq!(points[2].locks[1].permanently_open, None);

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
async fn camera_private_nested_evidence_is_bounded_without_leaking_secret_markers() {
    let stream_entries = serde_json::Map::from_iter((0..33).map(|index| {
        (
            format!("scheme-{index}"),
            serde_json::Value::String(format!("https://media.invalid/{index}")),
        )
    }));
    let too_many_intercoms = serde_json::Value::Array(
        (0..65)
            .map(|index| {
                serde_json::json!({
                    "id": format!("intercom-{index}"),
                    "locks": null
                })
            })
            .collect(),
    );
    let cases = [
        (
            "too many streams",
            serde_json::Value::Object(stream_entries),
            serde_json::Value::Null,
        ),
        (
            "oversized stream value",
            serde_json::json!({"hls": format!("https://media.invalid/{}", "s".repeat(4_097))}),
            serde_json::Value::Null,
        ),
        (
            "oversized stream key",
            serde_json::json!({"k".repeat(129): "https://media.invalid/live"}),
            serde_json::Value::Null,
        ),
        (
            "too many intercoms",
            serde_json::Value::Null,
            too_many_intercoms,
        ),
        (
            "oversized intercom id",
            serde_json::Value::Null,
            serde_json::json!([{"id": "i".repeat(513), "locks": null}]),
        ),
    ];

    for (case, streams, intercoms) in cases {
        let server = MockServer::start().await;
        let private_marker = format!("private-marker-{case}");
        let page = single_camera_page(
            "camera-private-bound",
            streams,
            intercoms,
            serde_json::json!({
                "cameraId": "provider-private-id",
                "server": "https://provider.invalid/private",
                "token": private_marker,
                "depth": 0
            }),
        );
        Mock::given(method("GET"))
            .and(path(CAMERA_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(page))
            .mount(&server)
            .await;
        let provider = MtsDeviceProvider::new(client(&server));

        let error = provider
            .cameras(&FlatId(format!("flat-private-{case}")))
            .await
            .expect_err(case);

        assert_eq!(error.code, ErrorCode::UpstreamSchemaChanged, "{case}");
        assert!(!format!("{error:?}").contains(&private_marker));
        assert!(!error.to_string().contains(&private_marker));
    }
}

#[tokio::test]
async fn camera_ignores_unused_intercom_lock_payloads_without_retaining_them() {
    let server = MockServer::start().await;
    let private_marker = "private-unused-camera-lock-marker";
    let page = single_camera_page(
        "camera-unused-locks",
        serde_json::Value::Null,
        serde_json::json!([{
            "id": "intercom-owned",
            "locks": {"opaqueProviderData": private_marker}
        }]),
        serde_json::Value::Null,
    );
    Mock::given(method("GET"))
        .and(path(CAMERA_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(page))
        .mount(&server)
        .await;
    let provider = MtsDeviceProvider::new(client(&server));

    let cameras = provider
        .cameras(&FlatId("flat-unused-locks".to_owned()))
        .await
        .unwrap();

    assert_eq!(
        cameras[0].access_point_id.as_deref(),
        Some("intercom-owned")
    );
    assert!(
        !serde_json::to_string(&cameras)
            .unwrap()
            .contains(private_marker)
    );
    assert!(!format!("{cameras:?}").contains(private_marker));
}

fn single_camera_page(
    id: &str,
    streams: serde_json::Value,
    intercoms: serde_json::Value,
    ivideon: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "content": [{
            "id": id,
            "name": "Synthetic camera",
            "addresses": null,
            "streams": streams,
            "streamsToWatch": null,
            "streamMode": null,
            "ivideon": ivideon,
            "intercoms": intercoms
        }],
        "totalElements": 1,
        "totalPages": 1,
        "number": 0,
        "size": 100,
        "last": true,
        "first": true,
        "empty": false
    })
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

#[tokio::test]
async fn access_point_lock_merge_preserves_null_evidence_and_rejects_conflicts() {
    let server = MockServer::start().await;
    let evidence_page = serde_json::json!({
        "content": [
            {
                "id": "access-evidence", "name": "Evidence door", "camera": null,
                "addresses": null,
                "locks": [{"number": 1, "name": "Door", "isPermanentlyOpen": null}]
            },
            {
                "id": "access-evidence", "name": "Evidence door", "camera": null,
                "addresses": null,
                "locks": [{"number": 1, "name": "Door", "isPermanentlyOpen": true}]
            }
        ],
        "totalElements": 2, "totalPages": 1, "number": 0, "size": 100,
        "last": true, "first": true, "empty": false
    });
    Mock::given(method("GET"))
        .and(path(ACCESS_PATH))
        .and(query_param("flatId", "flat-lock-evidence"))
        .respond_with(ResponseTemplate::new(200).set_body_json(evidence_page))
        .mount(&server)
        .await;
    let provider = MtsDeviceProvider::new(client(&server));

    let points = provider
        .access_points(&FlatId("flat-lock-evidence".to_owned()), None)
        .await
        .unwrap();

    assert_eq!(points[0].locks.len(), 1);
    assert_eq!(points[0].locks[0].number, "1");
    assert_eq!(points[0].locks[0].permanently_open, Some(true));

    let conflicts = [
        (
            "name",
            serde_json::json!([
                {"number": 1, "name": "Door A", "isPermanentlyOpen": null},
                {"number": 1, "name": "Door B", "isPermanentlyOpen": null}
            ]),
        ),
        (
            "status",
            serde_json::json!([
                {"number": 1, "name": "Door", "isPermanentlyOpen": true},
                {"number": 1, "name": "Door", "isPermanentlyOpen": false}
            ]),
        ),
    ];
    for (case, locks) in conflicts {
        let server = MockServer::start().await;
        let page = single_access_page("access-conflict", locks);
        Mock::given(method("GET"))
            .and(path(ACCESS_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(page))
            .mount(&server)
            .await;
        let provider = MtsDeviceProvider::new(client(&server));

        let error = provider
            .access_points(&FlatId(format!("flat-lock-{case}")), None)
            .await
            .expect_err(case);

        assert_eq!(error.code, ErrorCode::UpstreamSchemaChanged, "{case}");
    }
}

#[tokio::test]
async fn access_point_locks_reject_malformed_required_fields_and_nested_bounds() {
    let cases = [
        (
            "missing name",
            serde_json::json!([{"number": 1, "isPermanentlyOpen": null}]),
        ),
        (
            "non-integer number",
            serde_json::json!([{"number": "1", "name": "Door", "isPermanentlyOpen": null}]),
        ),
        (
            "malformed nullable status",
            serde_json::json!([{"number": 1, "name": "Door", "isPermanentlyOpen": "false"}]),
        ),
        (
            "too many locks",
            serde_json::Value::Array(
                (0..65)
                    .map(|number| {
                        serde_json::json!({
                            "number": number,
                            "name": format!("Door {number}"),
                            "isPermanentlyOpen": null
                        })
                    })
                    .collect(),
            ),
        ),
        (
            "oversized lock name",
            serde_json::json!([{
                "number": 1,
                "name": "n".repeat(513),
                "isPermanentlyOpen": null
            }]),
        ),
    ];

    for (case, locks) in cases {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(ACCESS_PATH))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(single_access_page("access-malformed", locks)),
            )
            .mount(&server)
            .await;
        let provider = MtsDeviceProvider::new(client(&server));

        let error = provider
            .access_points(&FlatId(format!("flat-malformed-{case}")), None)
            .await
            .expect_err(case);

        assert_eq!(error.code, ErrorCode::UpstreamSchemaChanged, "{case}");
    }
}

fn single_access_page(id: &str, locks: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "content": [{
            "id": id,
            "name": "Synthetic access point",
            "camera": null,
            "addresses": null,
            "locks": locks
        }],
        "totalElements": 1,
        "totalPages": 1,
        "number": 0,
        "size": 100,
        "last": true,
        "first": true,
        "empty": false
    })
}

#[tokio::test]
async fn provider_rejects_inconsistent_spring_page_totals_sizes_and_boundaries() {
    let cases = vec![
        (
            "page number arithmetic overflow",
            vec![serde_json::json!({
                "content": camera_items("overflow", 1),
                "totalElements": 1, "totalPages": 1,
                "number": u32::MAX, "size": 100,
                "last": false, "first": false, "empty": false
            })],
        ),
        (
            "zero pages with a nonzero total",
            vec![serde_json::json!({
                "content": [], "totalElements": 1, "totalPages": 0,
                "number": 0, "size": 100, "last": true, "first": true, "empty": true
            })],
        ),
        (
            "response size differs from requested size",
            vec![serde_json::json!({
                "content": [], "totalElements": 0, "totalPages": 0,
                "number": 0, "size": 99, "last": true, "first": true, "empty": true
            })],
        ),
        (
            "page count disagrees with total elements",
            vec![
                serde_json::json!({
                    "content": camera_items("count", 50),
                    "totalElements": 50, "totalPages": 2,
                    "number": 0, "size": 100, "last": false, "first": true, "empty": false
                }),
                serde_json::json!({
                    "content": [], "totalElements": 50, "totalPages": 2,
                    "number": 1, "size": 100, "last": true, "first": false, "empty": true
                }),
            ],
        ),
        (
            "early last page",
            vec![serde_json::json!({
                "content": camera_items("early", 100),
                "totalElements": 101, "totalPages": 2,
                "number": 0, "size": 100, "last": true, "first": true, "empty": false
            })],
        ),
        (
            "underfilled non-final page",
            vec![
                serde_json::json!({
                    "content": camera_items("under-first", 99),
                    "totalElements": 101, "totalPages": 2,
                    "number": 0, "size": 100, "last": false, "first": true, "empty": false
                }),
                serde_json::json!({
                    "content": camera_items("under-last", 2),
                    "totalElements": 101, "totalPages": 2,
                    "number": 1, "size": 100, "last": true, "first": false, "empty": false
                }),
            ],
        ),
        (
            "underfilled final page",
            vec![serde_json::json!({
                "content": camera_items("final", 1),
                "totalElements": 2, "totalPages": 1,
                "number": 0, "size": 100, "last": true, "first": true, "empty": false
            })],
        ),
        (
            "metadata changes between pages",
            vec![
                serde_json::json!({
                    "content": camera_items("drift-first", 100),
                    "totalElements": 101, "totalPages": 2,
                    "number": 0, "size": 100, "last": false, "first": true, "empty": false
                }),
                serde_json::json!({
                    "content": camera_items("drift-last", 1),
                    "totalElements": 102, "totalPages": 2,
                    "number": 1, "size": 100, "last": true, "first": false, "empty": false
                }),
            ],
        ),
    ];

    for (case, pages) in cases {
        let server = MockServer::start().await;
        mount_camera_pages(&server, "flat-invalid-pages", &pages).await;
        let provider = MtsDeviceProvider::new(client(&server));

        let error = provider
            .cameras(&FlatId("flat-invalid-pages".to_owned()))
            .await
            .expect_err(case);

        assert_eq!(error.code, ErrorCode::UpstreamSchemaChanged, "{case}");
    }
}

#[derive(Clone)]
struct FiftyCameraPageResponder;

impl Respond for FiftyCameraPageResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let page = request
            .url
            .query_pairs()
            .find_map(|(name, value)| (name == "page").then(|| value.parse::<u32>().unwrap()))
            .unwrap();
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "content": camera_items(&format!("boundary-{page}"), 100),
            "totalElements": 5_000,
            "totalPages": 50,
            "number": page,
            "size": 100,
            "last": page == 49,
            "first": page == 0,
            "empty": false
        }))
    }
}

#[tokio::test]
async fn provider_accepts_the_exact_fifty_page_five_thousand_item_boundary() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(CAMERA_PATH))
        .and(query_param("flatId", "flat-page-boundary"))
        .respond_with(FiftyCameraPageResponder)
        .mount(&server)
        .await;
    let provider = MtsDeviceProvider::new(client(&server));

    let cameras = provider
        .cameras(&FlatId("flat-page-boundary".to_owned()))
        .await
        .unwrap();

    assert_eq!(cameras.len(), 5_000);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 50);
    assert_eq!(
        requests[49].url.query(),
        Some("flatId=flat-page-boundary&page=49&size=100&sort=name%2Casc")
    );
}

async fn mount_camera_pages(server: &MockServer, flat: &str, pages: &[serde_json::Value]) {
    for (number, page) in pages.iter().enumerate() {
        Mock::given(method("GET"))
            .and(path(CAMERA_PATH))
            .and(query_param("flatId", flat))
            .and(query_param("page", number.to_string()))
            .respond_with(ResponseTemplate::new(200).set_body_json(page))
            .mount(server)
            .await;
    }
}

fn camera_items(prefix: &str, count: usize) -> Vec<serde_json::Value> {
    (0..count)
        .map(|index| {
            serde_json::json!({
                "id": format!("camera-{prefix}-{index}"),
                "name": format!("Camera {prefix} {index}"),
                "addresses": null,
                "streams": null,
                "streamsToWatch": null,
                "streamMode": null,
                "ivideon": null,
                "intercoms": null
            })
        })
        .collect()
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
                permanently_open: Some(false),
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
async fn camera_lookup_propagates_camera_source_errors_without_specific_or_unowned_probes() {
    let cases = [
        (
            "forbidden",
            ResponseTemplate::new(403),
            ErrorCode::Forbidden,
            None,
        ),
        (
            "schema",
            ResponseTemplate::new(200).set_body_json(serde_json::json!({})),
            ErrorCode::UpstreamSchemaChanged,
            None,
        ),
        (
            "rate-limit",
            ResponseTemplate::new(429).insert_header("Retry-After", "17"),
            ErrorCode::RateLimited,
            Some(17),
        ),
    ];

    for (case, camera_response, expected_code, expected_retry_after) in cases {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(ACCESS_PATH))
            .and(query_param("flatId", "flat-owned"))
            .respond_with(ResponseTemplate::new(200).set_body_json(empty_page()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(CAMERA_PATH))
            .and(query_param("flatId", "flat-owned"))
            .respond_with(camera_response)
            .mount(&server)
            .await;
        let service = DeviceService::new(Arc::new(MtsDeviceProvider::new(client(&server))));

        let error = service
            .camera(&FlatId("flat-owned".to_owned()), "camera-unowned")
            .await
            .unwrap_err();

        assert_eq!(error.code, expected_code, "case {case}");
        assert_eq!(
            error.retry_after_seconds, expected_retry_after,
            "case {case}"
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2, "case {case}");
        let mut paths = requests
            .iter()
            .map(|request| request.url.path())
            .collect::<Vec<_>>();
        paths.sort_unstable();
        assert_eq!(paths, vec![ACCESS_PATH, CAMERA_PATH], "case {case}");
        assert!(requests.iter().all(|request| {
            request
                .url
                .query_pairs()
                .any(|(name, value)| name == "flatId" && value == "flat-owned")
        }));
    }
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
