use std::sync::Arc;

use domru_mts::{
    error::ErrorCode,
    provider::mts::{
        AuthType, BootstrapProvider, MtsClient, NoTokens, ProviderBase,
    },
};
use reqwest::Method;
use url::Url;
use uuid::Uuid;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

const CONFIG_FIXTURE: &str = include_str!("../fixtures/provider_config.json");
const FEATURES_FIXTURE: &str = include_str!("../fixtures/provider_features.json");

fn client(server: &MockServer) -> MtsClient {
    let base = Url::parse(&format!("{}/", server.uri())).unwrap();
    MtsClient::new_for_test(
        reqwest::Client::new(),
        base.clone(),
        base,
        Arc::new(NoTokens),
    )
}

async fn mount_bootstrap(server: &MockServer, config: impl Into<String>, features: impl Into<String>) {
    Mock::given(method("GET"))
        .and(path("/config"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(config.into(), "application/json"))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/feature"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(features.into(), "application/json"))
        .mount(server)
        .await;
}

#[tokio::test]
async fn maps_direct_nullable_bootstrap_arrays_and_preserves_unknown_names() {
    let server = MockServer::start().await;
    mount_bootstrap(&server, CONFIG_FIXTURE, FEATURES_FIXTURE).await;

    let bootstrap = BootstrapProvider::new(client(&server)).fetch().await.unwrap();

    assert_eq!(bootstrap.auth_type, AuthType::WebSso);
    assert_eq!(bootstrap.android_config.as_ref().unwrap().config_name, "ANDROID_APP_VERSION");
    assert_eq!(bootstrap.android_config.as_ref().unwrap().platform, "ANDROID");
    assert_eq!(bootstrap.android_config.as_ref().unwrap().value, "3.0.1");
    assert_eq!(bootstrap.unknown_config_names, ["WEB_DIAGNOSTIC"]);
    assert_eq!(bootstrap.unknown_feature_names, ["FUTURE_CAPABILITY"]);

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    for request in requests {
        assert!(request.headers.get("authorization").is_none());
        assert!(request.headers.get("cookie").is_none());
        assert!(request.headers.get("x-device-token").is_none());
        assert_eq!(request.headers["x-device-type"], "ANDROID");
        assert_eq!(request.headers["accept"], "application/json");
        let request_id = request.headers["request-id"].to_str().unwrap();
        assert!(Uuid::parse_str(request_id).is_ok());
    }
}

#[tokio::test]
async fn anonymous_builder_is_restricted_to_bootstrap_gets() {
    let server = MockServer::start().await;
    let client = client(&server);

    for (method, path) in [
        (Method::POST, "/config"),
        (Method::GET, "/user-service/api/v2/auth/init"),
        (Method::GET, "https://attacker.example/config"),
    ] {
        let error = client.anonymous_gateway(method, path).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }

    assert!(client.anonymous_gateway(Method::GET, "/config").is_ok());
    assert!(client.anonymous_gateway(Method::GET, "/feature").is_ok());
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn duplicate_config_or_feature_names_are_schema_errors() {
    for (config, features) in [
        (
            r#"[
                {"configName":"VERSION","platform":"ANDROID","value":"1"},
                {"configName":"VERSION","platform":"IOS","value":"2"}
            ]"#,
            FEATURES_FIXTURE,
        ),
        (
            "[]",
            r#"[
                {"featureName":"AUTH_TYPE","value":"WEB_SSO"},
                {"featureName":"AUTH_TYPE","value":"SMS"}
            ]"#,
        ),
    ] {
        let server = MockServer::start().await;
        mount_bootstrap(&server, config, features).await;

        let error = BootstrapProvider::new(client(&server)).fetch().await.unwrap_err();
        assert_eq!(error.code, ErrorCode::UpstreamSchemaChanged);
    }
}

#[tokio::test]
async fn missing_or_legacy_auth_type_is_an_explicit_compatibility_error() {
    for features in [
        "null",
        "[]",
        r#"[{"featureName":"AUTH_TYPE","value":"SMS"}]"#,
        r#"[{"featureName":"AUTH_TYPE","value":""}]"#,
    ] {
        let server = MockServer::start().await;
        mount_bootstrap(&server, "null", features).await;

        let error = BootstrapProvider::new(client(&server)).fetch().await.unwrap_err();
        assert_eq!(error.code, ErrorCode::UpstreamSchemaChanged);
        assert!(!error.retryable);
        assert!(!format!("{error:?}").contains("SMS"));
    }
}

#[tokio::test]
async fn bootstrap_rejects_oversized_or_drifted_responses() {
    let oversized = format!(
        "[{{\"configName\":\"ANDROID_APP_VERSION\",\"platform\":\"ANDROID\",\"value\":\"{}\"}}]",
        "x".repeat(1_048_576)
    );
    for config in [
        oversized,
        r#"[{"configName":"ANDROID_APP_VERSION","value":"3.0.1"}]"#.into(),
    ] {
        let server = MockServer::start().await;
        mount_bootstrap(&server, config, FEATURES_FIXTURE).await;

        let error = BootstrapProvider::new(client(&server)).fetch().await.unwrap_err();
        assert_eq!(error.code, ErrorCode::UpstreamSchemaChanged);
    }
}

#[test]
fn endpoint_selects_the_base_and_rejects_unsafe_paths() {
    let gateway = Url::parse("http://127.0.0.1:41001/").unwrap();
    let resident = Url::parse("http://127.0.0.1:41002/").unwrap();
    let client = MtsClient::new_for_test(
        reqwest::Client::new(),
        gateway,
        resident,
        Arc::new(NoTokens),
    );

    assert_eq!(
        client.endpoint(ProviderBase::Gateway, "/config").unwrap().as_str(),
        "http://127.0.0.1:41001/config"
    );
    assert_eq!(
        client.endpoint(ProviderBase::Resident, "/health").unwrap().as_str(),
        "http://127.0.0.1:41002/health"
    );
    for path in ["config", "/a/../config", "https://attacker.example/config"] {
        assert_eq!(
            client.endpoint(ProviderBase::Gateway, path).unwrap_err().code,
            ErrorCode::InvalidRequest
        );
    }
}
