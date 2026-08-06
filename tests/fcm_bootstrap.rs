use domru_mts::provider::mts::fcm::parse_incoming_call_bootstrap;
use secrecy::ExposeSecret;
use serde_json::json;

fn wake_up_payload() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "data": {
            "action": "wake_up",
            "caller": json!({
                "id": "intercom-42",
                "number": "1001",
                "name": "Front door",
                "type": "intercom",
                "sessionId": "stale-caller-session"
            }).to_string(),
            "call": json!({"sessionId": "call-session-7"}).to_string(),
            "camera": json!({
                "streams": [
                    {"url": "https://camera.invalid/live.m3u8"},
                    {"url": "rtsp://camera.invalid/live"}
                ]
            }).to_string(),
            "ts": "1770000123456",
            "sipServer": json!({
                "ip": "sip.example.invalid",
                "sip": "resident-1001",
                "secret": "sip-secret-marker"
            }).to_string(),
            "webRTC": json!({"ws": "https://webrtc.example.invalid/ws"}).to_string()
        },
        "from": "firebase-sender",
        "priority": "high",
        "fcmMessageId": "message-1"
    }))
    .unwrap()
}

#[test]
fn parses_exact_wake_up_data_and_uses_call_session_as_authoritative() {
    let bootstrap = parse_incoming_call_bootstrap(&wake_up_payload())
        .unwrap()
        .expect("wake_up must produce a bootstrap");

    assert_eq!(bootstrap.intercom_id(), "intercom-42");
    assert_eq!(bootstrap.caller_name(), "Front door");
    assert_eq!(
        bootstrap.call_session_id().expose_secret(),
        "call-session-7"
    );
    assert_eq!(bootstrap.timestamp_millis(), 1_770_000_123_456);
    assert_eq!(bootstrap.camera_streams().len(), 2);
    assert_eq!(
        bootstrap.camera_streams()[0].expose_secret(),
        "https://camera.invalid/live.m3u8"
    );
    assert_eq!(
        bootstrap.websocket_url().expose_secret(),
        "https://webrtc.example.invalid/ws"
    );
    assert_eq!(bootstrap.sip().registrar(), "sip.example.invalid");
    assert_eq!(bootstrap.sip().username(), "resident-1001");
    assert_eq!(
        bootstrap.sip().secret().expose_secret(),
        "sip-secret-marker"
    );
}

#[test]
fn non_wake_up_data_is_ignored_without_requiring_call_fields() {
    let body = serde_json::to_vec(&json!({
        "data": {"action": "wave_up", "payload": "not-a-call"}
    }))
    .unwrap();

    assert!(parse_incoming_call_bootstrap(&body).unwrap().is_none());
}

#[test]
fn rejects_malformed_nested_json_and_unknown_wake_up_fields() {
    let mut malformed: serde_json::Value = serde_json::from_slice(&wake_up_payload()).unwrap();
    malformed["data"]["sipServer"] = json!("{");
    assert!(parse_incoming_call_bootstrap(&serde_json::to_vec(&malformed).unwrap()).is_err());

    let mut unknown: serde_json::Value = serde_json::from_slice(&wake_up_payload()).unwrap();
    unknown["data"]["unexpected"] = json!("value");
    assert!(parse_incoming_call_bootstrap(&serde_json::to_vec(&unknown).unwrap()).is_err());
}

#[test]
fn rejects_payload_and_nested_collection_size_over_bounds() {
    let oversized = vec![b'x'; 64 * 1024 + 1];
    assert!(parse_incoming_call_bootstrap(&oversized).is_err());

    let mut too_many: serde_json::Value = serde_json::from_slice(&wake_up_payload()).unwrap();
    too_many["data"]["camera"] = json!(
        json!({
            "streams": (0..17)
                .map(|index| json!({"url": format!("https://camera.invalid/{index}")}))
                .collect::<Vec<_>>()
        })
        .to_string()
    );
    assert!(parse_incoming_call_bootstrap(&serde_json::to_vec(&too_many).unwrap()).is_err());
}

#[test]
fn debug_output_redacts_session_urls_and_sip_secret() {
    let bootstrap = parse_incoming_call_bootstrap(&wake_up_payload())
        .unwrap()
        .unwrap();
    let debug = format!("{bootstrap:?}");

    for private_value in [
        "call-session-7",
        "sip-secret-marker",
        "https://camera.invalid/live.m3u8",
        "https://webrtc.example.invalid/ws",
    ] {
        assert!(
            !debug.contains(private_value),
            "debug leaked {private_value}"
        );
    }
    assert!(debug.contains("[REDACTED]"));
}
