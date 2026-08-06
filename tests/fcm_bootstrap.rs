use domru_mts::{
    calls::sip::{SipLimits, WssTransportConfig},
    provider::mts::fcm::parse_incoming_call_bootstrap,
};
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
        "wss://webrtc.example.invalid/ws"
    );
    assert!(
        WssTransportConfig::new(
            url::Url::parse(bootstrap.websocket_url().expose_secret()).unwrap(),
            SipLimits::default(),
        )
        .is_ok()
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
fn rejects_malformed_nested_json() {
    let mut malformed: serde_json::Value = serde_json::from_slice(&wake_up_payload()).unwrap();
    malformed["data"]["sipServer"] = json!("{");
    assert!(parse_incoming_call_bootstrap(&serde_json::to_vec(&malformed).unwrap()).is_err());
}

#[test]
fn ignores_additive_envelope_data_and_nested_fields() {
    let mut additive: serde_json::Value = serde_json::from_slice(&wake_up_payload()).unwrap();
    additive["futureEnvelopeField"] = json!({"version": 2});
    additive["data"]["futureDataField"] = json!("compatible");

    for nested_key in ["caller", "call", "camera", "webRTC", "sipServer"] {
        let nested = additive["data"][nested_key].as_str().unwrap();
        let mut nested: serde_json::Value = serde_json::from_str(nested).unwrap();
        nested["futureNestedField"] = json!({"enabled": true});
        additive["data"][nested_key] = json!(nested.to_string());
    }
    let camera = additive["data"]["camera"].as_str().unwrap();
    let mut camera: serde_json::Value = serde_json::from_str(camera).unwrap();
    camera["streams"][0]["futureStreamField"] = json!(17);
    additive["data"]["camera"] = json!(camera.to_string());

    let bootstrap = parse_incoming_call_bootstrap(&serde_json::to_vec(&additive).unwrap())
        .unwrap()
        .expect("additive fields must remain backward-compatible");
    assert_eq!(bootstrap.intercom_id(), "intercom-42");
}

#[test]
fn caller_label_falls_back_to_number_then_neutral_name() {
    let mut numbered: serde_json::Value = serde_json::from_slice(&wake_up_payload()).unwrap();
    let mut caller: serde_json::Value =
        serde_json::from_str(numbered["data"]["caller"].as_str().unwrap()).unwrap();
    caller["name"] = json!("  ");
    numbered["data"]["caller"] = json!(caller.to_string());
    let bootstrap = parse_incoming_call_bootstrap(&serde_json::to_vec(&numbered).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(bootstrap.caller_name(), "1001");

    caller["name"] = serde_json::Value::Null;
    caller["number"] = serde_json::Value::Null;
    numbered["data"]["caller"] = json!(caller.to_string());
    let bootstrap = parse_incoming_call_bootstrap(&serde_json::to_vec(&numbered).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(bootstrap.caller_name(), "Домофон");
}

#[test]
fn optional_caller_labels_remain_bounded_when_present() {
    let mut payload: serde_json::Value = serde_json::from_slice(&wake_up_payload()).unwrap();
    let mut caller: serde_json::Value =
        serde_json::from_str(payload["data"]["caller"].as_str().unwrap()).unwrap();
    caller["name"] = json!("x".repeat(513));
    payload["data"]["caller"] = json!(caller.to_string());

    assert!(parse_incoming_call_bootstrap(&serde_json::to_vec(&payload).unwrap()).is_err());
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
        "intercom-42",
        "Front door",
        "call-session-7",
        "sip-secret-marker",
        "sip.example.invalid",
        "resident-1001",
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
