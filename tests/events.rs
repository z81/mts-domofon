use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
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
        AccessPoint, AccessPointKind, CallMode, Camera, CameraCapabilities, Capability,
        CapabilitySet, Flat, FlatId, Lock,
        event::{CallOutcome, EventKind, EventQuery},
    },
    error::{AppError, ErrorCode},
    provider::mts::{
        AvailabilitySnapshot, MtsClient, PropertyProvider, ServiceSnapshot, TokenSource,
        events::{
            EventProvider, MtsEventProvider, ProviderEvent, ProviderEventPage, ProviderEventQuery,
        },
        intercom::DeviceProvider,
    },
    service::{devices::DeviceService, events::EventService, property::PropertyService},
    state::AppState,
    storage::SecretStore,
};
use http_body_util::BodyExt;
use secrecy::SecretString;
use serde_json::json;
use tower::ServiceExt;
use url::Url;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

#[derive(Clone, Copy)]
struct TestTokens;

#[async_trait]
impl TokenSource for TestTokens {
    async fn access_token(&self) -> Option<SecretString> {
        Some(SecretString::from("event-access-secret"))
    }

    async fn device_token(&self) -> Option<SecretString> {
        Some(SecretString::from("event-device-secret"))
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
async fn provider_uses_the_apk_event_contract_and_retains_only_normalization_evidence() {
    // Catches a wrong MTS route/query, dropped ownership evidence, or retention of private media,
    // SIP, peer, and phone fields in the provider-neutral boundary.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{
                "id": "event-owned-call",
                "alias": "intercom.incoming.call",
                "created": "2026-08-07T08:15:30.125",
                "consumerId": "flat-owned",
                "payload": {
                    "apartment": {"id": "flat-owned", "building": "private-building", "owner": "private-owner"},
                    "camera": {"id": "camera-owned", "pictureUrl": "https://private.invalid/frame"},
                    "context": [{"id": "entry-owned", "type": "INTERCOM"}],
                    "device": {"id": "entry-owned", "name": "Подъезд 1", "type": "INTERCOM"},
                    "intercom": {"id": "entry-owned", "name": "Домофон", "sip": "private-sip"},
                    "createdAt": "2026-08-07T08:15:30.125",
                    "finishedAt": "2026-08-07T08:16:00.125",
                    "status": "missed",
                    "peers": [{"dialStatus": {"name": "MISSED", "text": "private-phone"}, "opened": false}],
                    "pictureUrl": "https://private.invalid/legacy-frame",
                    "intercomPhone": "+70000000000"
                }
            }],
            "first": true,
            "last": true,
            "empty": false,
            "number": 0,
            "numberOfElements": 1,
            "size": 20,
            "totalElements": 1,
            "totalPages": 1
        })))
        .expect(1)
        .mount(&server)
        .await;
    let from = Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap();
    let to = Utc.with_ymd_and_hms(2026, 8, 7, 23, 59, 59).unwrap();

    let page = MtsEventProvider::new(client(&server))
        .page(ProviderEventQuery {
            page: 0,
            size: 20,
            from,
            to,
        })
        .await
        .unwrap();

    assert!(page.last);
    assert_eq!(page.events.len(), 1);
    let event = &page.events[0];
    assert_eq!(event.id, "event-owned-call");
    assert_eq!(event.alias.as_deref(), Some("intercom.incoming.call"));
    assert_eq!(
        event.identity_ids,
        ["camera-owned", "entry-owned", "flat-owned"]
    );
    assert_eq!(event.source_label.as_deref(), Some("Домофон"));
    assert_eq!(event.status.as_deref(), Some("missed"));
    assert_eq!(
        event.started_at,
        Some(
            Utc.with_ymd_and_hms(2026, 8, 7, 8, 15, 30).unwrap()
                + chrono::TimeDelta::milliseconds(125)
        )
    );
    assert_eq!(
        event.finished_at,
        Some(
            Utc.with_ymd_and_hms(2026, 8, 7, 8, 16, 0).unwrap()
                + chrono::TimeDelta::milliseconds(125)
        )
    );
    let rendered = format!("{event:?}");
    for private in ["private", "+70000000000", "pictureUrl", "sip"] {
        assert!(!rendered.contains(private));
    }

    let requests = server.received_requests().await.unwrap();
    let request = &requests[0];
    assert_eq!(request.url.path(), "/vdome-smart-facade/api/v1/event");
    let query: std::collections::BTreeMap<_, _> = request.url.query_pairs().into_owned().collect();
    assert_eq!(query.get("page").map(String::as_str), Some("0"));
    assert_eq!(query.get("size").map(String::as_str), Some("20"));
    assert_eq!(
        query.get("from").map(String::as_str),
        Some("2026-08-01T00:00:00.000")
    );
    assert_eq!(
        query.get("to").map(String::as_str),
        Some("2026-08-07T23:59:59.000")
    );
}

#[derive(Clone)]
struct OwnedPropertyProvider;

#[async_trait]
impl PropertyProvider for OwnedPropertyProvider {
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

struct OwnedDevices {
    archive: Capability,
}

#[async_trait]
impl DeviceProvider for OwnedDevices {
    async fn access_points(
        &self,
        flat: &FlatId,
        _category: Option<&str>,
    ) -> Result<Vec<AccessPoint>, AppError> {
        Ok(vec![AccessPoint {
            id: "entry-owned".to_owned(),
            flat_id: flat.clone(),
            name: "Подъезд 1".to_owned(),
            kind: AccessPointKind::Intercom,
            locks: vec![Lock {
                number: "1".to_owned(),
                name: "Основная дверь".to_owned(),
                permanently_open: Some(false),
            }],
            camera_ids: vec!["camera-owned".to_owned()],
            available: true,
        }])
    }

    async fn cameras(&self, flat: &FlatId) -> Result<Vec<Camera>, AppError> {
        Ok(vec![Camera {
            id: "camera-owned".to_owned(),
            flat_id: flat.clone(),
            name: "Камера подъезда".to_owned(),
            access_point_id: Some("entry-owned".to_owned()),
            capabilities: CameraCapabilities {
                snapshot: Capability::Available,
                live: Capability::Available,
                archive: self.archive.clone(),
            },
        }])
    }
}

struct QueuedEvents {
    pages: Mutex<VecDeque<ProviderEventPage>>,
}

#[async_trait]
impl EventProvider for QueuedEvents {
    async fn page(&self, _query: ProviderEventQuery) -> Result<ProviderEventPage, AppError> {
        self.pages
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| AppError::invalid_request("unexpected provider page"))
    }
}

fn provider_event(
    id: &str,
    alias: Option<&str>,
    identity_ids: &[&str],
    camera_id: Option<&str>,
    status: Option<&str>,
) -> ProviderEvent {
    ProviderEvent {
        id: id.to_owned(),
        alias: alias.map(ToOwned::to_owned),
        occurred_at: Utc.with_ymd_and_hms(2026, 8, 7, 8, 15, 30).unwrap(),
        started_at: Some(Utc.with_ymd_and_hms(2026, 8, 7, 8, 15, 30).unwrap()),
        finished_at: Some(Utc.with_ymd_and_hms(2026, 8, 7, 8, 16, 0).unwrap()),
        identity_ids: identity_ids
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        source_label: Some("Подъезд 1".to_owned()),
        status: status.map(ToOwned::to_owned),
        camera_id: camera_id.map(ToOwned::to_owned),
        accepted: false,
        ignored: false,
        missed: status == Some("missed"),
        rejected: false,
        unlocked: alias == Some("intercom.unlocked"),
    }
}

async fn event_service(pages: Vec<ProviderEventPage>) -> EventService {
    event_service_with_archive(pages, Capability::Available).await
}

async fn event_service_with_archive(
    pages: Vec<ProviderEventPage>,
    archive: Capability,
) -> EventService {
    let directory = tempfile::tempdir().unwrap();
    let store = SecretStore::open(directory.path(), Some([17; 32]))
        .await
        .unwrap();
    // PropertyService owns a store clone; keeping the temporary directory is unnecessary after
    // construction because this test does not select a flat or persist property state.
    let properties = Arc::new(PropertyService::new(Arc::new(OwnedPropertyProvider), store));
    let devices = Arc::new(DeviceService::new(Arc::new(OwnedDevices { archive })));
    EventService::new(
        properties,
        devices,
        Arc::new(QueuedEvents {
            pages: Mutex::new(pages.into()),
        }),
    )
}

#[tokio::test]
async fn events_do_not_offer_an_archive_link_for_a_camera_without_provider_archive() {
    let service = event_service_with_archive(
        vec![ProviderEventPage {
            events: vec![provider_event(
                "owned-access",
                Some("intercom.unlocked"),
                &["entry-owned"],
                Some("camera-owned"),
                None,
            )],
            last: true,
        }],
        Capability::Unavailable { reason: None },
    )
    .await;

    let page = service
        .events(EventQuery {
            flat_id: FlatId("flat-owned".to_owned()),
            page: 0,
            size: 20,
            from: Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap(),
            to: Utc.with_ymd_and_hms(2026, 8, 7, 23, 59, 59).unwrap(),
            kind: None,
        })
        .await
        .unwrap();

    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].kind, EventKind::Access);
    assert_eq!(page.items[0].camera_id, None);
    assert_eq!(page.items[0].archive_at, None);
}

#[tokio::test]
async fn service_filters_foreign_events_classifies_owned_events_and_uses_local_has_more() {
    // Catches widening ownership to fill a page, trusting account-wide totals, misclassifying
    // call/access/camera events, or leaking a foreign camera ID.
    let service = event_service(vec![ProviderEventPage {
        events: vec![
            provider_event(
                "foreign-event",
                Some("intercom.incoming.call"),
                &["entry-foreign"],
                Some("camera-foreign"),
                Some("answered"),
            ),
            provider_event(
                "owned-call",
                Some("intercom.incoming.call"),
                &["entry-owned"],
                Some("camera-owned"),
                Some("missed"),
            ),
            provider_event(
                "owned-access",
                Some("intercom.unlocked"),
                &["entry-owned"],
                None,
                None,
            ),
            provider_event(
                "owned-camera",
                Some("camera.motion"),
                &["camera-owned"],
                Some("camera-owned"),
                None,
            ),
        ],
        last: true,
    }])
    .await;

    let page = service
        .events(EventQuery {
            flat_id: FlatId("flat-owned".to_owned()),
            page: 0,
            size: 2,
            from: Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap(),
            to: Utc.with_ymd_and_hms(2026, 8, 7, 23, 59, 59).unwrap(),
            kind: None,
        })
        .await
        .unwrap();

    assert_eq!(page.page, 0);
    assert!(page.has_more);
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items[0].id, "owned-call");
    assert_eq!(page.items[0].kind, EventKind::Call);
    assert_eq!(page.items[0].title, "Пропущенный вызов");
    assert_eq!(page.items[0].call_outcome, Some(CallOutcome::Missed));
    assert_eq!(page.items[0].duration_seconds, Some(30));
    assert_eq!(page.items[0].camera_id.as_deref(), Some("camera-owned"));
    assert_eq!(page.items[1].id, "owned-access");
    assert_eq!(page.items[1].kind, EventKind::Access);
    assert_eq!(page.items[1].title, "Дверь открыта");
    let json = serde_json::to_string(&page).unwrap();
    assert!(!json.contains("foreign"));
    assert!(!json.contains("total"));
    assert!(!json.contains("picture"));
    assert!(!json.contains("sip"));
}

#[tokio::test]
async fn local_route_applies_the_call_filter_and_exposes_only_the_stable_page() {
    // Catches a missing route, ignored kind filter, unsafe cacheability, or accidental exposure of
    // provider paging fields and nested call data.
    let service = Arc::new(
        event_service(vec![ProviderEventPage {
            events: vec![
                provider_event(
                    "owned-call",
                    Some("intercom.incoming.call"),
                    &["flat-owned"],
                    Some("camera-owned"),
                    Some("answered"),
                ),
                provider_event(
                    "owned-access",
                    Some("intercom.unlocked"),
                    &["entry-owned"],
                    None,
                    None,
                ),
            ],
            last: true,
        }])
        .await,
    );
    let state = AppState::default().with_event_service(service);
    let app = with_http_middleware(domru_mts::api::router().with_state(state));
    let request = Request::builder()
        .uri("/api/v1/events?flatId=flat-owned&page=0&size=20&from=2026-08-01T00%3A00%3A00Z&to=2026-08-07T23%3A59%3A59Z&kind=call")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CACHE_CONTROL).unwrap(),
        "no-store"
    );
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["page"], 0);
    assert_eq!(value["hasMore"], false);
    assert_eq!(value["items"].as_array().unwrap().len(), 1);
    assert_eq!(value["items"][0]["id"], "owned-call");
    assert_eq!(value["items"][0]["kind"], "call");
    let rendered = String::from_utf8(body.to_vec()).unwrap();
    for forbidden in ["totalElements", "payload", "pictureUrl", "sip", "peers"] {
        assert!(!rendered.contains(forbidden));
    }
}
