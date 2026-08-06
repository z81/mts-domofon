use domru_mts::config::AppConfig;

#[test]
fn defaults_are_loopback_and_local_data() {
    let cfg = AppConfig::from_iter(std::iter::empty::<(&str, &str)>()).unwrap();
    assert_eq!(cfg.bind.to_string(), "127.0.0.1:3000");
    assert_eq!(cfg.data_dir.to_string_lossy(), "./data");
    assert_eq!(cfg.web_dist_dir.to_string_lossy(), "./frontend/dist");
    assert_eq!(cfg.gateway.as_str(), "https://gateway.vdome.mts.ru/");
    assert_eq!(cfg.resident.as_str(), "https://resident-rest.vdome.mts.ru/");
    assert_eq!(
        cfg.call_endpoint.as_str(),
        "https://webrtc.domofon.mts.ru/ws"
    );
}

#[test]
fn rejects_non_https_provider_url() {
    let err = AppConfig::from_iter([("VDOME_GATEWAY_URL", "http://example.test")]).unwrap_err();
    assert!(err.to_string().contains("https"));
}

#[test]
fn rejects_https_provider_urls_on_unapproved_hosts() {
    for (key, value) in [
        ("VDOME_GATEWAY_URL", "https://attacker.example/"),
        ("VDOME_RESIDENT_URL", "https://attacker.example/"),
        ("VDOME_CALL_ENDPOINT", "https://attacker.example/ws"),
    ] {
        assert!(
            AppConfig::from_iter([(key, value)]).is_err(),
            "{key} accepted unapproved host"
        );
    }
}

#[test]
fn normalizes_provider_base_urls_to_trailing_slashes() {
    let cfg = AppConfig::from_iter([
        ("VDOME_GATEWAY_URL", "https://gateway.vdome.mts.ru"),
        ("VDOME_RESIDENT_URL", "https://resident-rest.vdome.mts.ru"),
    ])
    .unwrap();

    assert_eq!(cfg.gateway.as_str(), "https://gateway.vdome.mts.ru/");
    assert_eq!(cfg.resident.as_str(), "https://resident-rest.vdome.mts.ru/");
}

#[test]
fn rejects_non_root_provider_base_urls() {
    for (key, value) in [
        (
            "VDOME_GATEWAY_URL",
            "https://gateway.vdome.mts.ru/unexpected",
        ),
        (
            "VDOME_RESIDENT_URL",
            "https://resident-rest.vdome.mts.ru/unexpected",
        ),
    ] {
        assert!(
            AppConfig::from_iter([(key, value)]).is_err(),
            "{key} accepted a non-root base URL"
        );
    }
}

#[test]
fn rejects_call_endpoint_query_or_fragment() {
    for value in [
        "https://webrtc.domofon.mts.ru/ws?unexpected=true",
        "https://webrtc.domofon.mts.ru/ws#unexpected",
    ] {
        assert!(
            AppConfig::from_iter([("VDOME_CALL_ENDPOINT", value)]).is_err(),
            "call endpoint accepted query or fragment"
        );
    }
}
