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
