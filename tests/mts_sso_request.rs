use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use domru_mts::{
    domain::auth::DeviceIdentity,
    provider::mts::{device::MtsDeviceDescriptor, sso::SsoRequest},
};
use serde_json::json;
use uuid::Uuid;

const FIXED_DEVICE_ID: &str = "eyJ1dWlkIjoiMzJmNmFkMmItOWYzZS00YTVkLThkMmMtMmFjNGRjZGJiM2U3IiwicGxhdGZvcm0iOiJBbmRyb2lkIiwidmVuZG9yIjoiVkRvbWUgV2ViIiwibW9kZWwiOiJSdXN0IFNlcnZpY2UiLCJuYW1lIjoiZG9tcnUtbXRzIiwiaXAiOiIiLCJ0aW1lem9uZSI6IkV1cm9wZS9Nb3Njb3ciLCJsb2NhbGUiOiJydV9SVSIsInVzZXJhZ2VudCI6ImRvbXJ1LW10cy8wLjEiLCJ2ZXJzaW9uIjoiNS44LjAiLCJzY3JlZW5TaXplIjoiIiwic3lzdGVtVmVyc2lvbiI6InNlcnZlciJ9";
const FIXED_STATE: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8";

#[test]
fn encodes_the_fixed_mts_device_descriptor_without_host_data() {
    let identity = DeviceIdentity {
        uuid: Uuid::parse_str("32f6ad2b-9f3e-4a5d-8d2c-2ac4dcdbb3e7").unwrap(),
    };

    let encoded = MtsDeviceDescriptor::from_identity(&identity)
        .encode()
        .unwrap();
    assert_eq!(encoded, FIXED_DEVICE_ID);
    let decoded: serde_json::Value =
        serde_json::from_slice(&STANDARD.decode(encoded).unwrap()).unwrap();

    assert_eq!(
        decoded,
        json!({
            "uuid": "32f6ad2b-9f3e-4a5d-8d2c-2ac4dcdbb3e7",
            "platform": "Android",
            "vendor": "VDome Web",
            "model": "Rust Service",
            "name": "domru-mts",
            "ip": "",
            "timezone": "Europe/Moscow",
            "locale": "ru_RU",
            "useragent": "domru-mts/0.1",
            "version": "5.8.0",
            "screenSize": "",
            "systemVersion": "server",
        })
    );
}

#[test]
fn builds_the_current_mts_authorization_url_with_all_scopes() {
    let request = SsoRequest::new(FIXED_DEVICE_ID, FIXED_STATE).unwrap();
    let url = request.authorize_url();

    assert_eq!(url.scheme(), "https");
    assert_eq!(url.host_str(), Some("login.mts.ru"));
    assert_eq!(url.path(), "/amserver/oauth2/authorize");
    assert_eq!(
        url.query_pairs().into_owned().collect::<Vec<_>>(),
        vec![
            ("client_id".to_owned(), "VDome".to_owned()),
            (
                "redirect_uri".to_owned(),
                "https://gateway.vdome.mts.ru/user-service/api/auth/callback".to_owned(),
            ),
            ("response_type".to_owned(), "code".to_owned()),
            (
                "scope".to_owned(),
                "account address sso profile ssom openid phone".to_owned(),
            ),
            ("state".to_owned(), FIXED_STATE.to_owned()),
            ("device_id".to_owned(), FIXED_DEVICE_ID.to_owned()),
        ]
    );
}

#[test]
fn supplies_the_observed_login_sdk_headers() {
    let request = SsoRequest::new(FIXED_DEVICE_ID, FIXED_STATE).unwrap();
    let headers = request.headers();

    assert_eq!(headers.len(), 5);
    assert_eq!(headers["X-SDK-APPVersion"], "3.0.1");
    assert_eq!(headers["X-SDK-Name"], "Login");
    assert_eq!(headers["X-SDK-ClientId"], "VDome");
    assert_eq!(headers["X-SDK-Platform"], "Android");
    assert_eq!(headers["X-SDK-Version"], "5.8.0");
}

#[test]
fn creates_distinct_256_bit_url_safe_states() {
    let first = SsoRequest::with_fresh_state(FIXED_DEVICE_ID);
    let second = SsoRequest::with_fresh_state(FIXED_DEVICE_ID);

    assert_eq!(first.state().len(), 43);
    assert!(
        first
            .state()
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    );
    assert_eq!(URL_SAFE_NO_PAD.decode(first.state()).unwrap().len(), 32);
    assert_ne!(first.state(), second.state());
}

#[test]
fn validates_only_the_expected_raw_https_gateway_callback_and_state() {
    let request = SsoRequest::new(FIXED_DEVICE_ID, FIXED_STATE).unwrap();
    let valid = format!(
        "https://gateway.vdome.mts.ru/user-service/api/auth/callback?code=opaque&state={FIXED_STATE}"
    );

    assert!(request.validate_callback(&valid).is_ok());

    for invalid in [
        format!("http://gateway.vdome.mts.ru/user-service/api/auth/callback?state={FIXED_STATE}"),
        format!(
            "https://gateway.vdome.mts.ru:443/user-service/api/auth/callback?state={FIXED_STATE}"
        ),
        format!(
            "https://user@gateway.vdome.mts.ru/user-service/api/auth/callback?state={FIXED_STATE}"
        ),
        format!(
            "https://gateway.vdome.mts.ru/user-service/api/auth/callback?state={FIXED_STATE}#fragment"
        ),
        format!("https://GATEWAY.vdome.mts.ru/user-service/api/auth/callback?state={FIXED_STATE}"),
        format!("https://gateway.vdome.mts.ru./user-service/api/auth/callback?state={FIXED_STATE}"),
        format!(
            "https://gateway.vdome.mts.ru.evil.test/user-service/api/auth/callback?state={FIXED_STATE}"
        ),
        format!("https://gateway.vdome.mts.ru/not-the-callback?state={FIXED_STATE}"),
        "https://gateway.vdome.mts.ru/user-service/api/auth/callback?state=wrong-state".to_owned(),
        "https://gateway.vdome.mts.ru/user-service/api/auth/callback".to_owned(),
        format!(
            "https://gateway.vdome.mts.ru/user-service/api/auth/callback?state={FIXED_STATE}&state={FIXED_STATE}"
        ),
    ] {
        assert!(
            request.validate_callback(&invalid).is_err(),
            "must reject {invalid}"
        );
    }
}

#[test]
fn rejects_malformed_short_and_long_states_at_each_boundary() {
    let malformed = "!AECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8";
    let short = &FIXED_STATE[..42];
    let long = format!("{FIXED_STATE}A");

    for state in [malformed, short, long.as_str()] {
        assert!(SsoRequest::new(FIXED_DEVICE_ID, state).is_err());
        let request = SsoRequest::new(FIXED_DEVICE_ID, FIXED_STATE).unwrap();
        let callback =
            format!("https://gateway.vdome.mts.ru/user-service/api/auth/callback?state={state}");
        assert!(request.validate_callback(&callback).is_err());
    }
}
