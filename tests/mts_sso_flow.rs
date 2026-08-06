use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use domru_mts::{
    domain::auth::{AuthChallenge, AuthFlowId, AuthProgress, ChallengeInput},
    provider::mts::sso::MtsSsoTransport,
};
#[cfg(feature = "test-support")]
use domru_mts::{
    error::{AppError, ErrorCode},
    provider::mts::sso::MtsSsoTestConfig,
};
#[cfg(feature = "test-support")]
use secrecy::{ExposeSecret, SecretString};
use std::str::FromStr;
#[cfg(feature = "test-support")]
use std::{sync::Arc, time::Duration};
use url::Url;
#[cfg(feature = "test-support")]
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

#[tokio::test]
async fn begin_creates_unique_bounded_server_side_browser_flows() {
    let transport = MtsSsoTransport::new("synthetic-device");

    let first = transport.begin().await.unwrap();
    let second = transport.begin().await.unwrap();

    let AuthProgress::BrowserRedirect {
        flow_id: first_id,
        authorization_url: first_url,
    } = first
    else {
        panic!("begin must return a browser redirect")
    };
    let AuthProgress::BrowserRedirect {
        flow_id: second_id,
        authorization_url: second_url,
    } = second
    else {
        panic!("begin must return a browser redirect")
    };

    assert_ne!(first_id, second_id);
    assert!(first_id.as_str().len() <= 64);
    assert!(second_id.as_str().len() <= 64);

    let first_state = first_url
        .query_pairs()
        .find_map(|(name, value)| (name == "state").then(|| value.into_owned()))
        .unwrap();
    let second_state = second_url
        .query_pairs()
        .find_map(|(name, value)| (name == "state").then(|| value.into_owned()))
        .unwrap();
    assert_eq!(first_state.len(), 43);
    assert_eq!(URL_SAFE_NO_PAD.decode(&first_state).unwrap().len(), 32);
    assert_ne!(first_state, second_state);

    let first_debug = format!("{first_id:?}");
    assert!(!first_debug.contains(&first_state));
    assert!(!format!("{transport:?}").contains(&first_state));
}

#[tokio::test]
async fn begin_rejects_an_oversized_authorization_url_without_creating_a_flow() {
    let transport = MtsSsoTransport::new("x".repeat(8_200));

    let error = transport.begin().await.unwrap_err();

    assert_eq!(error.code, domru_mts::error::ErrorCode::InvalidRequest);
    assert!(!format!("{error:?}").contains(&"x".repeat(256)));
    assert!(format!("{transport:?}").contains("active_flows: 0"));
}

#[test]
fn auth_types_use_the_exact_local_snake_case_wire_contract() {
    let flow_id = AuthFlowId::from_str("00000000-0000-4000-8000-000000000001").unwrap();
    let browser = AuthProgress::BrowserRedirect {
        flow_id: flow_id.clone(),
        authorization_url: Url::parse("https://login.invalid/authorize?opaque=value").unwrap(),
    };
    assert_eq!(
        serde_json::to_value(browser).unwrap(),
        serde_json::json!({
            "status": "browser_redirect",
            "flow_id": "00000000000040008000000000000001",
            "authorization_url": "https://login.invalid/authorize?opaque=value"
        })
    );

    let challenge = AuthProgress::Challenge {
        flow_id,
        challenge: AuthChallenge::SmsOtp {
            resend_after_seconds: 30,
        },
    };
    assert_eq!(
        serde_json::to_value(challenge).unwrap(),
        serde_json::json!({
            "status": "challenge",
            "flow_id": "00000000000040008000000000000001",
            "challenge": {
                "type": "sms_otp",
                "resend_after_seconds": 30
            }
        })
    );

    let input: ChallengeInput = serde_json::from_value(serde_json::json!({
        "type": "phone",
        "phone": "+7 999 000-00-00"
    }))
    .unwrap();
    assert!(matches!(input, ChallengeInput::Phone { .. }));
    assert!(
        serde_json::from_value::<ChallengeInput>(serde_json::json!({
            "kind": "phone",
            "phone": "+7 999 000-00-00"
        }))
        .is_err()
    );
    assert!(serde_json::from_value::<AuthFlowId>(serde_json::json!("not-a-flow-id")).is_err());
    assert!(serde_json::from_value::<AuthFlowId>(serde_json::json!("x".repeat(4_096))).is_err());
}

#[cfg(feature = "test-support")]
fn callback_target(server: &MockServer) -> Url {
    Url::parse(&format!("{}/user-service/api/auth/callback", server.uri())).unwrap()
}

#[cfg(feature = "test-support")]
fn test_transport(server: &MockServer) -> MtsSsoTransport {
    MtsSsoTransport::new_for_test(
        "synthetic-device",
        MtsSsoTestConfig::new(callback_target(server)),
    )
    .unwrap()
}

#[cfg(feature = "test-support")]
async fn begin_parts(transport: &MtsSsoTransport) -> (AuthFlowId, String) {
    let AuthProgress::BrowserRedirect {
        flow_id,
        authorization_url,
    } = transport.begin().await.unwrap()
    else {
        panic!("begin must return a browser redirect")
    };
    let state = authorization_url
        .query_pairs()
        .find_map(|(name, value)| (name == "state").then(|| value.into_owned()))
        .unwrap();
    (flow_id, state)
}

#[cfg(feature = "test-support")]
fn raw_callback(server: &MockServer, state: &str) -> SecretString {
    SecretString::from(format!(
        "{}/user-service/api/auth/callback?code=opaque&state={state}",
        server.uri()
    ))
}

#[cfg(feature = "test-support")]
async fn completion_error(
    transport: &MtsSsoTransport,
    flow_id: &AuthFlowId,
    callback: SecretString,
) -> AppError {
    match transport.complete_callback(flow_id, callback).await {
        Ok(_) => panic!("callback completion unexpectedly succeeded"),
        Err(error) => error,
    }
}

#[tokio::test]
#[cfg(feature = "test-support")]
async fn callback_success_parses_all_persisted_token_classes_and_discards_id_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user-service/api/auth/callback"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            include_str!("fixtures/sso/callback-success.json"),
            "application/json",
        ))
        .expect(1)
        .mount(&server)
        .await;
    let transport = test_transport(&server);
    let (flow_id, state) = begin_parts(&transport).await;

    let tokens = transport
        .complete_callback(&flow_id, raw_callback(&server, &state))
        .await
        .unwrap();

    assert_eq!(
        tokens.web_sso_access_token.expose_secret(),
        "synthetic-wsso"
    );
    assert_eq!(tokens.access_token.expose_secret(), "synthetic-access");
    assert_eq!(tokens.refresh_token.expose_secret(), "synthetic-refresh");
    assert!(tokens.device_token.is_none());
    let output = format!("{transport:?}");
    assert!(!output.contains("synthetic-wsso"));
    assert!(!output.contains("synthetic-access"));
    assert!(!output.contains("synthetic-refresh"));
    assert!(!output.contains("synthetic-id-token"));
}

#[tokio::test]
#[cfg(feature = "test-support")]
async fn invalid_callback_values_fail_before_any_upstream_request() {
    let server = MockServer::start().await;
    let transport = test_transport(&server);
    let (flow_id, state) = begin_parts(&transport).await;
    let port = callback_target(&server).port().unwrap();
    let oversized = format!(
        "{}/user-service/api/auth/callback?state={state}&padding={}",
        server.uri(),
        "x".repeat(8_200)
    );
    let invalid = [
        format!(
            "{}/user-service/api/auth/callback?state=wrong",
            server.uri()
        ),
        format!(
            "{}/user-service/api/auth/callback?state={state}&state={state}",
            server.uri()
        ),
        format!("http://localhost:{port}/user-service/api/auth/callback?state={state}"),
        format!("{}/wrong-path?state={state}", server.uri()),
        "http://127.0.0.1:1/user-service/api/auth/callback?state=".to_owned() + &state,
        format!("http://user@127.0.0.1:{port}/user-service/api/auth/callback?state={state}"),
        format!(
            "{}/user-service/api/auth/callback?state={state}#fragment",
            server.uri()
        ),
        oversized,
    ];

    for callback in invalid {
        let error =
            completion_error(&transport, &flow_id, SecretString::from(callback.clone())).await;
        assert_eq!(error.code, ErrorCode::InvalidRequest, "{callback}");
        assert!(!format!("{error:?}").contains(&callback));
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
#[cfg(feature = "test-support")]
async fn callback_completion_is_one_use_under_concurrency() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user-service/api/auth/callback"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            include_str!("fixtures/sso/callback-success.json"),
            "application/json",
        ))
        .mount(&server)
        .await;
    let transport = Arc::new(test_transport(&server));
    let (flow_id, state) = begin_parts(&transport).await;
    let callback = raw_callback(&server, &state);

    let mut tasks = Vec::new();
    for _ in 0..16 {
        let transport = transport.clone();
        let flow_id = flow_id.clone();
        let callback = callback.clone();
        tasks.push(tokio::spawn(async move {
            transport.complete_callback(&flow_id, callback).await
        }));
    }
    let mut successes = 0;
    for task in tasks {
        successes += usize::from(task.await.unwrap().is_ok());
    }

    assert_eq!(successes, 1);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert!(
        transport
            .complete_callback(&flow_id, raw_callback(&server, &state))
            .await
            .is_err()
    );
}

#[tokio::test]
#[cfg(feature = "test-support")]
async fn unknown_expired_and_oldest_evicted_flows_fail_closed() {
    let server = MockServer::start().await;
    let unknown = AuthFlowId::from_str("00000000-0000-4000-8000-000000000001").unwrap();
    let transport = test_transport(&server);
    let error = completion_error(&transport, &unknown, SecretString::from("secret-callback")).await;
    assert_eq!(error.code, ErrorCode::SessionExpired);

    let mut config = MtsSsoTestConfig::new(callback_target(&server));
    config.flow_lifetime = Duration::ZERO;
    let expiring = MtsSsoTransport::new_for_test("synthetic-device", config).unwrap();
    let (flow_id, state) = begin_parts(&expiring).await;
    let error = completion_error(&expiring, &flow_id, raw_callback(&server, &state)).await;
    assert_eq!(error.code, ErrorCode::SessionExpired);

    Mock::given(method("GET"))
        .and(path("/user-service/api/auth/callback"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            include_str!("fixtures/sso/callback-success.json"),
            "application/json",
        ))
        .expect(1)
        .mount(&server)
        .await;
    let bounded = test_transport(&server);
    let (oldest_id, oldest_state) = begin_parts(&bounded).await;
    let mut newest = None;
    for _ in 0..5 {
        newest = Some(begin_parts(&bounded).await);
    }
    let error = completion_error(&bounded, &oldest_id, raw_callback(&server, &oldest_state)).await;
    assert_eq!(error.code, ErrorCode::SessionExpired);
    let (newest_id, newest_state) = newest.unwrap();
    bounded
        .complete_callback(&newest_id, raw_callback(&server, &newest_state))
        .await
        .unwrap();
}

#[tokio::test]
#[cfg(feature = "test-support")]
async fn oversized_non_200_and_malformed_callback_responses_fail_safely() {
    for (status, body, expected) in [
        (200, "x".repeat(257), ErrorCode::UpstreamSchemaChanged),
        (
            503,
            "unavailable".to_owned(),
            ErrorCode::UpstreamAuthenticationFailed,
        ),
        (
            200,
            "{not-json".to_owned(),
            ErrorCode::UpstreamSchemaChanged,
        ),
        (
            200,
            r#"{"webSsoAccessToken":"only-one-field"}"#.to_owned(),
            ErrorCode::UpstreamSchemaChanged,
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user-service/api/auth/callback"))
            .respond_with(ResponseTemplate::new(status).set_body_string(body))
            .expect(1)
            .mount(&server)
            .await;
        let mut config = MtsSsoTestConfig::new(callback_target(&server));
        config.max_callback_body_len = 256;
        let transport = MtsSsoTransport::new_for_test("synthetic-device", config).unwrap();
        let (flow_id, state) = begin_parts(&transport).await;

        let error = completion_error(&transport, &flow_id, raw_callback(&server, &state)).await;
        assert_eq!(error.code, expected);
        assert!(!format!("{error:?}").contains("only-one-field"));
    }
}

#[tokio::test]
#[cfg(feature = "test-support")]
async fn callback_redirects_are_not_followed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user-service/api/auth/callback"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Location", format!("{}/unexpected", server.uri())),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/unexpected"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            include_str!("fixtures/sso/callback-success.json"),
            "application/json",
        ))
        .mount(&server)
        .await;
    let transport = test_transport(&server);
    let (flow_id, state) = begin_parts(&transport).await;

    let error = completion_error(&transport, &flow_id, raw_callback(&server, &state)).await;

    assert_eq!(error.code, ErrorCode::UpstreamAuthenticationFailed);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.path(), "/user-service/api/auth/callback");
}

#[tokio::test]
#[cfg(feature = "test-support")]
async fn unconfirmed_challenge_inputs_make_no_upstream_requests() {
    let server = MockServer::start().await;
    let transport = test_transport(&server);
    let (flow_id, _) = begin_parts(&transport).await;
    let inputs = [
        ChallengeInput::Phone {
            phone: "+7 999 000-00-00".to_owned(),
        },
        ChallengeInput::SmsOtp {
            code: SecretString::from("1234"),
        },
        ChallengeInput::Captcha {
            answer: SecretString::from("answer"),
        },
        ChallengeInput::Password {
            password: SecretString::from("password"),
        },
        ChallengeInput::Account {
            id: "account".to_owned(),
        },
        ChallengeInput::Consent { accepted: true },
    ];

    for input in inputs {
        let error = transport.submit(&flow_id, input).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::UpstreamSchemaChanged);
        assert!(error.to_string().to_lowercase().contains("capture"));
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}
