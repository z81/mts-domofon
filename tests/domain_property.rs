use domru_mts::domain::{
    AccessPoint, AccessPointKind, Camera, CameraCapabilities, Capability, CapabilitySet, Flat,
    FlatId, Lock, Page,
};

fn unavailable(reason: &str) -> Capability {
    Capability::Unavailable {
        reason: Some(reason.to_owned()),
    }
}

fn capabilities() -> CapabilitySet {
    CapabilitySet {
        access_points: Capability::Available,
        cameras: unavailable("camera service is unavailable"),
        archive: Capability::Available,
        events: Capability::Available,
        calls: Capability::Available,
        call_settings: Capability::Available,
    }
}

#[test]
fn serializes_property_models_with_camel_case_capabilities() {
    let flat = Flat {
        id: FlatId("flat-synthetic".to_owned()),
        label: "Home".to_owned(),
        address: "Synthetic address".to_owned(),
        capabilities: capabilities(),
    };

    let value = serde_json::to_value(flat).unwrap();

    assert_eq!(value["id"], "flat-synthetic");
    assert_eq!(value["capabilities"]["accessPoints"]["state"], "available");
    assert_eq!(value["capabilities"]["cameras"]["state"], "unavailable");
    assert_eq!(
        value["capabilities"]["cameras"]["reason"],
        "camera service is unavailable"
    );
    assert!(value["capabilities"].get("access_points").is_none());
}

#[test]
fn preserves_unknown_access_point_kind_from_json() {
    let access_point: AccessPoint = serde_json::from_value(serde_json::json!({
        "id": "access-synthetic",
        "flatId": "flat-synthetic",
        "name": "Synthetic entrance",
        "kind": "turnstile",
        "locks": [],
        "cameraIds": [],
        "available": false
    }))
    .unwrap();

    assert_eq!(
        access_point.kind,
        AccessPointKind::Unknown("turnstile".to_owned())
    );
    assert_eq!(
        serde_json::to_value(access_point).unwrap()["kind"],
        "turnstile"
    );
}

#[test]
fn keeps_multiple_locks_on_an_access_point() {
    let access_point = AccessPoint {
        id: "access-synthetic".to_owned(),
        flat_id: FlatId("flat-synthetic".to_owned()),
        name: "Synthetic entrance".to_owned(),
        kind: AccessPointKind::Intercom,
        locks: vec![
            Lock {
                number: "1".to_owned(),
                name: "Pedestrian door".to_owned(),
                permanently_open: None,
            },
            Lock {
                number: "2".to_owned(),
                name: "Vehicle gate".to_owned(),
                permanently_open: Some(false),
            },
        ],
        camera_ids: vec!["camera-synthetic".to_owned()],
        available: true,
    };

    let value = serde_json::to_value(access_point).unwrap();

    assert_eq!(value["locks"].as_array().unwrap().len(), 2);
    assert_eq!(value["locks"][1]["number"], "2");
    assert!(value["locks"][0]["permanentlyOpen"].is_null());
    assert_eq!(value["locks"][1]["permanentlyOpen"], false);
    assert_eq!(value["cameraIds"][0], "camera-synthetic");
}

#[test]
fn serializes_camera_and_cursor_page_with_camel_case_fields() {
    let page = Page {
        items: vec![Camera {
            id: "camera-synthetic".to_owned(),
            flat_id: FlatId("flat-synthetic".to_owned()),
            name: "Synthetic camera".to_owned(),
            access_point_id: Some("access-synthetic".to_owned()),
            capabilities: CameraCapabilities {
                snapshot: Capability::Available,
                live: Capability::Available,
                archive: unavailable("archive is unavailable"),
            },
        }],
        next_cursor: Some("next-synthetic".to_owned()),
        total: None,
    };

    let value = serde_json::to_value(page).unwrap();

    assert_eq!(value["nextCursor"], "next-synthetic");
    assert!(value.get("next_cursor").is_none());
    assert_eq!(value["items"][0]["accessPointId"], "access-synthetic");
    assert_eq!(
        value["items"][0]["capabilities"]["archive"]["reason"],
        "archive is unavailable"
    );
}
