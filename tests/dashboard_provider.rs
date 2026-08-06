use std::sync::Arc;

use async_trait::async_trait;
use domru_mts::{
    domain::{
        CallMode, CallSettings, CallSettingsPatch, CallSettingsStatus, Capability, FlatId,
        PropertyDashboard,
    },
    error::{AppError, ErrorCode},
    provider::mts::{MtsClient, MtsDashboardProvider, NoTokens, PropertyProvider},
    service::property::PropertyService,
    storage::SecretStore,
};
use secrecy::SecretString;
use serde_json::json;
use url::Url;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, method, path},
};

const FLATS: &str = include_str!("fixtures/flats.json");
const SERVICES: &str = include_str!("fixtures/flat_services.json");

#[derive(Clone, Copy)]
struct StaticTokens;

#[async_trait]
impl domru_mts::provider::mts::TokenSource for StaticTokens {
    async fn access_token(&self) -> Option<SecretString> {
        Some(SecretString::from("access-synthetic"))
    }

    async fn device_token(&self) -> Option<SecretString> {
        None
    }

    async fn refresh_after_unauthorized(&self) -> Result<(), AppError> {
        Err(AppError::new(
            ErrorCode::SessionExpired,
            "Synthetic session expired.",
            false,
        ))
    }
}

fn provider(server: &MockServer) -> MtsDashboardProvider {
    let base = Url::parse(&format!("{}/", server.uri())).unwrap();
    let client = MtsClient::new_for_test(base.clone(), base, Arc::new(StaticTokens)).unwrap();
    MtsDashboardProvider::new(client)
}

async fn mount_flats(server: &MockServer, body: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path("/vdome-smart-facade/estate/api/v1/residents/flats"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

async fn mount_dashboard_reads(server: &MockServer) {
    mount_flats(server, serde_json::from_str(FLATS).unwrap()).await;
    Mock::given(method("GET"))
        .and(path(
            "/vdome-smart-facade/devices/api/v1/flats/flat%20synthetic+one/available",
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!(["provider-secret-future-capability"])),
        )
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/vdome-smart-facade/services/api/v1/residents/flats/flat%20synthetic+one",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_raw(SERVICES, "application/json"))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/vdome-smart-facade/devices/api/v1/flats/flat-synthetic-two/available",
        ))
        .respond_with(
            ResponseTemplate::new(503).set_body_string("raw-provider-body-that-must-not-escape"),
        )
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/vdome-smart-facade/services/api/v1/residents/flats/flat-synthetic-two",
        ))
        .respond_with(ResponseTemplate::new(503))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/vdome-smart-facade/devices/api/v1/call-settings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"type": "both"})))
        .mount(server)
        .await;
}

fn resident_fixture(prefix: &str, count: usize) -> serde_json::Value {
    let parts = (0..count)
        .map(|index| {
            json!({
                "id": format!("{prefix}-flat-{index}"),
                "name": format!("Label {index}"),
                "displayName": null,
                "buildingId": format!("{prefix}-building-{index}")
            })
        })
        .collect::<Vec<_>>();
    let buildings = (0..count)
        .map(|index| {
            json!({
                "id": format!("{prefix}-building-{index}"),
                "address": {
                    "fullAddress": format!("Synthetic bounded address {index}")
                }
            })
        })
        .collect::<Vec<_>>();
    json!({"parts": parts, "buildings": buildings})
}

fn buildings_only_fixture(prefix: &str, count: usize) -> serde_json::Value {
    let buildings = (0..count)
        .map(|index| {
            json!({
                "id": format!("{prefix}-building-{index}"),
                "address": {"fullAddress": "Synthetic unused address"}
            })
        })
        .collect::<Vec<_>>();
    json!({"parts": [], "buildings": buildings})
}

#[tokio::test]
async fn expands_every_part_and_joins_the_exact_building() {
    let server = MockServer::start().await;
    mount_flats(&server, serde_json::from_str(FLATS).unwrap()).await;

    let flats = provider(&server).flats().await.unwrap();

    assert_eq!(flats.len(), 2);
    assert_eq!(flats[0].id, FlatId("flat synthetic+one".into()));
    assert_eq!(flats[0].label, "Entrance One");
    assert_eq!(flats[0].address, "Synthetic City, Hidden Lane 17");
    assert_eq!(flats[1].id, FlatId("flat-synthetic-two".into()));
    assert_eq!(flats[1].label, "Fallback Two");
    assert_eq!(flats[1].address, "Synthetic City, Hidden Lane 22");
}

#[tokio::test]
async fn accepts_nullable_and_additive_arrays_but_rejects_identity_or_join_drift() {
    let server = MockServer::start().await;
    mount_flats(
        &server,
        json!([{
            "company": {"id": "company-synthetic", "name": "Synthetic"},
            "parts": null,
            "buildings": null,
            "category": {"id": "category-synthetic", "name": "Apartment"},
            "future": true
        }]),
    )
    .await;
    assert!(provider(&server).flats().await.unwrap().is_empty());

    let null_server = MockServer::start().await;
    mount_flats(&null_server, serde_json::Value::Null).await;
    assert!(provider(&null_server).flats().await.unwrap().is_empty());

    for drifted in [
        json!([{
            "company": {"id": "company-synthetic", "name": "Synthetic"},
            "parts": [{"id": "", "name": "label", "buildingId": "building-synthetic"}],
            "buildings": [{"id": "building-synthetic", "address": {"fullAddress": "Synthetic City, Secret Address 1"}}],
            "category": {"id": "category-synthetic", "name": "Apartment"}
        }]),
        json!([{
            "company": {"id": "company-synthetic", "name": "Synthetic"},
            "parts": [{"id": "flat-synthetic", "name": "label", "buildingId": "missing-building"}],
            "buildings": [{"id": "building-synthetic", "address": {"fullAddress": "Synthetic City, Secret Address 2"}}],
            "category": {"id": "category-synthetic", "name": "Apartment"}
        }]),
    ] {
        let drift_server = MockServer::start().await;
        mount_flats(&drift_server, drifted).await;
        let error = provider(&drift_server).flats().await.unwrap_err();
        assert_eq!(error.code, ErrorCode::UpstreamSchemaChanged);
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.contains("Secret Address"));
        assert!(!rendered.contains("missing-building"));
        assert!(rendered.len() < 512);
    }
}

#[tokio::test]
async fn rejects_ambiguous_duplicate_building_ids_instead_of_selecting_the_first() {
    let server = MockServer::start().await;
    mount_flats(
        &server,
        json!([{
            "parts": [{
                "id": "flat-ambiguous",
                "name": "Ambiguous flat",
                "buildingId": "building-duplicate"
            }],
            "buildings": [
                {
                    "id": "building-duplicate",
                    "address": {"fullAddress": "Synthetic first private address"}
                },
                {
                    "id": "building-duplicate",
                    "address": {"fullAddress": "Synthetic second private address"}
                }
            ]
        }]),
    )
    .await;

    let error = provider(&server).flats().await.unwrap_err();

    assert_eq!(error.code, ErrorCode::UpstreamSchemaChanged);
    let rendered = format!("{error:?} {error}");
    assert!(!rendered.contains("first private address"));
    assert!(!rendered.contains("second private address"));
}

#[tokio::test]
async fn enforces_checked_aggregate_flat_boundary_across_resident_objects() {
    let boundary_server = MockServer::start().await;
    mount_flats(
        &boundary_server,
        json!([
            resident_fixture("boundary-a", 256),
            resident_fixture("boundary-b", 256)
        ]),
    )
    .await;
    assert_eq!(provider(&boundary_server).flats().await.unwrap().len(), 512);

    let oversized_server = MockServer::start().await;
    mount_flats(
        &oversized_server,
        json!([
            resident_fixture("oversized-a", 256),
            resident_fixture("oversized-b", 257)
        ]),
    )
    .await;

    let error = provider(&oversized_server).flats().await.unwrap_err();
    assert_eq!(error.code, ErrorCode::UpstreamSchemaChanged);
}

#[tokio::test]
async fn enforces_checked_aggregate_dto_item_boundary_without_flat_output() {
    let boundary_server = MockServer::start().await;
    mount_flats(
        &boundary_server,
        json!([
            buildings_only_fixture("boundary-a", 512),
            buildings_only_fixture("boundary-b", 512)
        ]),
    )
    .await;
    assert!(provider(&boundary_server).flats().await.unwrap().is_empty());

    let oversized_server = MockServer::start().await;
    mount_flats(
        &oversized_server,
        json!([
            buildings_only_fixture("oversized-a", 512),
            buildings_only_fixture("oversized-b", 512),
            buildings_only_fixture("oversized-c", 1)
        ]),
    )
    .await;

    let error = provider(&oversized_server).flats().await.unwrap_err();
    assert_eq!(error.code, ErrorCode::UpstreamSchemaChanged);
}

#[tokio::test]
async fn uses_exact_percent_safe_availability_and_service_paths_only() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/vdome-smart-facade/devices/api/v1/flats/flat%20synthetic+one/available",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!(["future-value"])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/vdome-smart-facade/services/api/v1/residents/flats/flat%20synthetic+one",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(1)
        .mount(&server)
        .await;
    let provider = provider(&server);
    let flat = FlatId("flat synthetic+one".into());

    provider.availability(&flat).await.unwrap();
    provider.services(&flat).await.unwrap();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|request| {
        let path = request.url.path();
        !path.contains("favorite")
            && !path.contains("invoice")
            && !path.contains("marketplace")
            && !path.contains("meters")
    }));
}

#[tokio::test]
async fn opaque_flat_ids_remain_one_url_segment_for_all_supported_characters() {
    let cases = [
        ("flat%percent", "flat%25percent"),
        ("flat/section", "flat%2Fsection"),
        ("flat?query#fragment", "flat%3Fquery%23fragment"),
        (
            "квартира",
            "%D0%BA%D0%B2%D0%B0%D1%80%D1%82%D0%B8%D1%80%D0%B0",
        ),
        ("//evil.example/../flat", "%2F%2Fevil.example%2F..%2Fflat"),
    ];

    for (flat_id, encoded) in cases {
        let server = MockServer::start().await;
        let expected_path = format!("/vdome-smart-facade/devices/api/v1/flats/{encoded}/available");
        Mock::given(method("GET"))
            .and(path(expected_path))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(1)
            .mount(&server)
            .await;

        provider(&server)
            .availability(&FlatId(flat_id.to_owned()))
            .await
            .unwrap();
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_ne!(requests[0].url.host_str(), Some("evil.example"));
    }
}

#[tokio::test]
async fn exact_dot_segments_are_rejected_without_dispatch_or_path_ambiguity() {
    let server = MockServer::start().await;
    let provider = provider(&server);

    for flat_id in [".", ".."] {
        let error = provider
            .availability(&FlatId(flat_id.to_owned()))
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn composes_only_confirmed_aliases_and_degrades_each_read_independently() {
    let server = MockServer::start().await;
    mount_dashboard_reads(&server).await;
    let dir = tempfile::tempdir().unwrap();
    let store = SecretStore::open(dir.path(), Some([31; 32])).await.unwrap();
    let service = PropertyService::new(Arc::new(provider(&server)), store);

    let dashboard = service.dashboard().await.unwrap();

    assert_eq!(dashboard.flats.len(), 2);
    assert!(matches!(
        dashboard.flats[0].capabilities.calls,
        Capability::Available
    ));
    assert!(matches!(
        dashboard.flats[0].capabilities.archive,
        Capability::Available
    ));
    assert!(matches!(
        dashboard.flats[1].capabilities.calls,
        Capability::Unavailable { .. }
    ));
    assert!(matches!(
        dashboard.flats[1].capabilities.archive,
        Capability::Unavailable { .. }
    ));
    for flat in &dashboard.flats {
        assert!(matches!(
            flat.capabilities.access_points,
            Capability::Unavailable { .. }
        ));
        assert!(matches!(
            flat.capabilities.cameras,
            Capability::Unavailable { .. }
        ));
        assert!(matches!(
            flat.capabilities.events,
            Capability::Unavailable { .. }
        ));
        assert!(matches!(
            flat.capabilities.call_settings,
            Capability::Available
        ));
    }
    assert!(matches!(
        dashboard.call_settings,
        CallSettingsStatus::Available(ref settings)
            if settings.mode == CallMode::Both && settings.revision == 0
    ));
    let debug = format!("{dashboard:?}");
    for sensitive in [
        "flat synthetic+one",
        "Synthetic City, Hidden Lane 17",
        "Synthetic City, Hidden Lane 22",
        "provider-secret-future-alias",
        "provider-secret-future-capability",
        "raw-provider-body-that-must-not-escape",
    ] {
        assert!(!debug.contains(sensitive), "leaked {sensitive}");
    }
}

#[tokio::test]
async fn selection_checks_ownership_restores_encrypted_state_and_clears_missing_flats() {
    let dir = tempfile::tempdir().unwrap();
    let key = [42; 32];
    let first_server = MockServer::start().await;
    mount_dashboard_reads(&first_server).await;
    let store = SecretStore::open(dir.path(), Some(key)).await.unwrap();
    let service = PropertyService::new(Arc::new(provider(&first_server)), store);
    assert_eq!(service.dashboard().await.unwrap().selected_flat_id, None);

    let selected = FlatId("flat synthetic+one".into());
    service.select_flat(selected.clone()).await.unwrap();
    let ciphertext = tokio::fs::read(dir.path().join("selected-flat.enc"))
        .await
        .unwrap();
    assert!(
        !ciphertext
            .windows(selected.0.len())
            .any(|value| value == selected.0.as_bytes())
    );
    assert!(!String::from_utf8_lossy(&ciphertext).contains("Hidden Lane"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join("selected-flat.enc"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    let reopened_store = SecretStore::open(dir.path(), Some(key)).await.unwrap();
    let reopened = PropertyService::new(Arc::new(provider(&first_server)), reopened_store);
    assert_eq!(
        reopened.dashboard().await.unwrap().selected_flat_id,
        Some(selected.clone())
    );

    let missing_server = MockServer::start().await;
    mount_flats(&missing_server, json!([])).await;
    Mock::given(method("GET"))
        .and(path("/vdome-smart-facade/devices/api/v1/call-settings"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&missing_server)
        .await;
    let missing_store = SecretStore::open(dir.path(), Some(key)).await.unwrap();
    let missing = PropertyService::new(Arc::new(provider(&missing_server)), missing_store);
    assert_eq!(missing.dashboard().await.unwrap().selected_flat_id, None);
    assert!(!dir.path().join("selected-flat.enc").exists());
    let error = missing.select_flat(selected).await.unwrap_err();
    assert_eq!(error.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn unavailable_account_call_settings_do_not_hide_a_linked_flat() {
    let server = MockServer::start().await;
    mount_flats(&server, serde_json::from_str(FLATS).unwrap()).await;
    Mock::given(method("GET"))
        .and(path("/vdome-smart-facade/devices/api/v1/call-settings"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let store = SecretStore::open(dir.path(), Some([61; 32])).await.unwrap();

    let dashboard = PropertyService::new(Arc::new(provider(&server)), store)
        .dashboard()
        .await
        .unwrap();

    assert_eq!(dashboard.flats.len(), 2);
    assert!(matches!(
        dashboard.call_settings,
        CallSettingsStatus::Unavailable
    ));
    assert!(dashboard.flats.iter().all(|flat| matches!(
        flat.capabilities.call_settings,
        Capability::Unavailable { .. }
    )));
}

#[tokio::test]
async fn call_settings_are_account_scoped_exact_and_preserve_unknown_reads() {
    for (raw, expected) in [
        ("gsm", CallMode::Gsm),
        ("sip", CallMode::Sip),
        ("both", CallMode::Both),
        (
            "provider-secret-future-mode",
            CallMode::Unknown("provider-secret-future-mode".into()),
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/vdome-smart-facade/devices/api/v1/call-settings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"type": raw})))
            .expect(1)
            .mount(&server)
            .await;
        let mode = provider(&server).call_settings().await.unwrap();
        assert_eq!(mode, expected);
        assert!(!format!("{mode:?}").contains("provider-secret-future-mode"));
        let public_json = serde_json::to_value(PropertyDashboard {
            flats: Vec::new(),
            selected_flat_id: None,
            call_settings: CallSettingsStatus::Available(CallSettings {
                mode: mode.clone(),
                revision: 7,
            }),
        })
        .unwrap();
        assert!(
            !public_json
                .to_string()
                .contains("provider-secret-future-mode")
        );
        if matches!(mode, CallMode::Unknown(_)) {
            assert_eq!(public_json["callSettings"]["settings"]["mode"], "unknown");
        }
    }

    for (mode_value, raw) in [
        (CallMode::Gsm, "gsm"),
        (CallMode::Sip, "sip"),
        (CallMode::Both, "both"),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/vdome-smart-facade/devices/api/v1/call-settings"))
            .and(body_json(json!({"type": raw})))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        provider(&server)
            .update_call_settings(&mode_value)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn unknown_and_stale_writes_are_local_and_success_increments_local_revision() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/vdome-smart-facade/devices/api/v1/call-settings"))
        .and(body_json(json!({"type": "sip"})))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let direct = provider(&server);
    let error = direct
        .update_call_settings(&CallMode::Unknown("future".into()))
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(server.received_requests().await.unwrap().is_empty());

    let dir = tempfile::tempdir().unwrap();
    let store = SecretStore::open(dir.path(), Some([51; 32])).await.unwrap();
    let service = PropertyService::new(Arc::new(direct), store);
    let stale = service
        .update_call_settings(CallSettingsPatch {
            mode: CallMode::Sip,
            expected_revision: 1,
        })
        .await
        .unwrap_err();
    assert_eq!(stale.code, ErrorCode::InvalidRequest);
    let unknown = service
        .update_call_settings(CallSettingsPatch {
            mode: CallMode::Unknown("future".into()),
            expected_revision: 0,
        })
        .await
        .unwrap_err();
    assert_eq!(unknown.code, ErrorCode::InvalidRequest);
    assert!(server.received_requests().await.unwrap().is_empty());

    let changed = service
        .update_call_settings(CallSettingsPatch {
            mode: CallMode::Sip,
            expected_revision: 0,
        })
        .await
        .unwrap();
    assert_eq!(changed.mode, CallMode::Sip);
    assert_eq!(changed.revision, 1);
    let stale_again = service
        .update_call_settings(CallSettingsPatch {
            mode: CallMode::Gsm,
            expected_revision: 0,
        })
        .await
        .unwrap_err();
    assert_eq!(stale_again.code, ErrorCode::InvalidRequest);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn successful_call_setting_status_advances_revision_despite_oversized_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/vdome-smart-facade/devices/api/v1/call-settings"))
        .and(body_json(json!({"type": "both"})))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; 32 * 1024]))
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let store = SecretStore::open(dir.path(), Some([71; 32])).await.unwrap();
    let service = PropertyService::new(Arc::new(provider(&server)), store);

    let changed = service
        .update_call_settings(CallSettingsPatch {
            mode: CallMode::Both,
            expected_revision: 0,
        })
        .await
        .unwrap();

    assert_eq!(changed.revision, 1);
    assert_eq!(changed.mode, CallMode::Both);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn call_setting_mutation_dispatches_once_on_unauthorized_or_server_ambiguity() {
    for status in [401, 503] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/vdome-smart-facade/devices/api/v1/call-settings"))
            .respond_with(ResponseTemplate::new(status))
            .expect(1)
            .mount(&server)
            .await;

        let error = provider(&server)
            .update_call_settings(&CallMode::Both)
            .await
            .unwrap_err();
        assert!(matches!(
            error.code,
            ErrorCode::UpstreamAuthenticationFailed | ErrorCode::UpstreamUnavailable
        ));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn no_tokens_helper_is_not_accidentally_usable_for_dashboard_calls() {
    let server = MockServer::start().await;
    let base = Url::parse(&format!("{}/", server.uri())).unwrap();
    let client = MtsClient::new_for_test(base.clone(), base, Arc::new(NoTokens)).unwrap();
    let error = MtsDashboardProvider::new(client).flats().await.unwrap_err();
    assert_eq!(error.code, ErrorCode::SessionExpired);
    assert!(server.received_requests().await.unwrap().is_empty());
}
