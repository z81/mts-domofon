use domru_mts::config::{AppConfig, ConfigError};

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
fn wildcard_bind_requires_exact_opt_in() {
    for bind in ["0.0.0.0:3000", "[::]:3000"] {
        assert!(matches!(
            AppConfig::from_iter([("VDOME_BIND", bind)]),
            Err(ConfigError::BindNotAllowed)
        ));
        assert!(matches!(
            AppConfig::from_iter([("VDOME_BIND", bind), ("VDOME_ALLOW_NON_LOOPBACK", "false"),]),
            Err(ConfigError::BindNotAllowed)
        ));
        assert_eq!(
            AppConfig::from_iter([("VDOME_BIND", bind), ("VDOME_ALLOW_NON_LOOPBACK", "true"),])
                .unwrap()
                .bind
                .to_string(),
            bind
        );
    }
}

#[test]
fn rejects_invalid_or_stale_non_loopback_opt_in() {
    for value in [
        "", "1", "TRUE", " true", "true ", " false", "false ", "FALSE",
    ] {
        assert!(matches!(
            AppConfig::from_iter([("VDOME_ALLOW_NON_LOOPBACK", value)]),
            Err(ConfigError::InvalidAllowNonLoopback)
        ));
    }
    assert!(matches!(
        AppConfig::from_iter([("VDOME_ALLOW_NON_LOOPBACK", "true")]),
        Err(ConfigError::AllowNonLoopbackRequiresWildcard)
    ));
}

#[test]
fn rejected_config_values_are_not_rendered_in_errors() {
    let sensitive_flag = "sensitive-flag-token";
    let flag_error =
        AppConfig::from_iter([("VDOME_ALLOW_NON_LOOPBACK", sensitive_flag)]).unwrap_err();
    assert!(matches!(flag_error, ConfigError::InvalidAllowNonLoopback));
    assert_eq!(flag_error.to_string(), "invalid VDOME_ALLOW_NON_LOOPBACK");
    assert!(!flag_error.to_string().contains(sensitive_flag));

    let sensitive_bind = "10.73.42.19:49152";
    let bind_error = AppConfig::from_iter([
        ("VDOME_BIND", sensitive_bind),
        ("VDOME_ALLOW_NON_LOOPBACK", "true"),
    ])
    .unwrap_err();
    assert!(matches!(bind_error, ConfigError::BindNotAllowed));
    assert_eq!(bind_error.to_string(), "VDOME_BIND is not allowed");
    assert!(!bind_error.to_string().contains(sensitive_bind));
}

#[test]
fn accepts_loopback_bind_addresses_without_or_with_disabled_opt_in() {
    for value in ["127.0.0.2:3100", "[::1]:3100"] {
        for allow_non_loopback in [None, Some("false")] {
            let values = std::iter::once(("VDOME_BIND", value)).chain(
                allow_non_loopback
                    .into_iter()
                    .map(|flag| ("VDOME_ALLOW_NON_LOOPBACK", flag)),
            );
            let config = AppConfig::from_iter(values).unwrap();
            assert!(config.bind.ip().is_loopback());
            assert_eq!(config.bind.port(), 3100);
        }
    }
}

#[test]
fn rejects_non_wildcard_non_loopback_binds_even_with_opt_in() {
    for bind in ["192.168.1.50:3000", "10.0.0.8:3000", "[2001:db8::1]:3000"] {
        assert!(matches!(
            AppConfig::from_iter([("VDOME_BIND", bind), ("VDOME_ALLOW_NON_LOOPBACK", "true"),]),
            Err(ConfigError::BindNotAllowed)
        ));
    }
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
