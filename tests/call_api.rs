use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use chrono::{DateTime, TimeZone, Utc};
use domru_mts::{
    app::build_router_with_config,
    config::AppConfig,
    domain::call::{CallId, CallMedia},
    error::AppError,
    provider::mts::{
        MtsClient, TokenSource,
        call::{CallActionProvider, MtsCallActionProvider, MtsCallOpenProvider, RemoteCallAction},
    },
    service::calls::{
        CallClock, CallOpenProvider, CallService, CallTransport, ObservedIncomingCall,
    },
    state::AppState,
};
use http_body_util::BodyExt;
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use tower::ServiceExt;
use url::Url;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json as wire_body_json, header as wire_header, method, path},
};

#[derive(Clone, Debug, PartialEq, Eq)]
enum TransportAction {
    Answer(CallId, String),
    Decline(CallId),
    Ignore(CallId),
    Hangup(CallId),
}

#[derive(Default)]
struct RecordingTransport {
    actions: Mutex<Vec<TransportAction>>,
}

#[async_trait]
impl CallTransport for RecordingTransport {
    async fn answer(&self, call_id: &CallId, answer_sdp: &str) -> Result<(), AppError> {
        self.actions.lock().unwrap().push(TransportAction::Answer(
            call_id.clone(),
            answer_sdp.to_owned(),
        ));
        Ok(())
    }

    async fn decline(&self, call_id: &CallId) -> Result<(), AppError> {
        self.actions
            .lock()
            .unwrap()
            .push(TransportAction::Decline(call_id.clone()));
        Ok(())
    }

    async fn ignore(&self, call_id: &CallId) -> Result<(), AppError> {
        self.actions
            .lock()
            .unwrap()
            .push(TransportAction::Ignore(call_id.clone()));
        Ok(())
    }

    async fn hangup(&self, call_id: &CallId) -> Result<(), AppError> {
        self.actions
            .lock()
            .unwrap()
            .push(TransportAction::Hangup(call_id.clone()));
        Ok(())
    }
}

#[derive(Default)]
struct RecordingOpenProvider {
    calls: Mutex<Vec<(String, String)>>,
}

#[async_trait]
impl CallOpenProvider for RecordingOpenProvider {
    async fn open(
        &self,
        intercom_id: &SecretString,
        call_session_id: &SecretString,
    ) -> Result<(), AppError> {
        self.calls.lock().unwrap().push((
            intercom_id.expose_secret().to_owned(),
            call_session_id.expose_secret().to_owned(),
        ));
        Ok(())
    }
}

#[derive(Clone)]
struct FixedClock(DateTime<Utc>);

impl CallClock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

struct Fixture {
    app: Router,
    service: Arc<CallService>,
    transport: Arc<RecordingTransport>,
    opener: Arc<RecordingOpenProvider>,
    call_id: CallId,
}

async fn fixture(with_call: bool) -> Fixture {
    let now = Utc.with_ymd_and_hms(2026, 8, 6, 10, 0, 0).unwrap();
    let transport = Arc::new(RecordingTransport::default());
    let opener = Arc::new(RecordingOpenProvider::default());
    let service = Arc::new(
        CallService::new_for_test(
            transport.clone(),
            opener.clone(),
            Arc::new(FixedClock(now)),
            Duration::from_secs(30),
            16,
        )
        .unwrap(),
    );
    let call_id = CallId::new();
    if with_call {
        service
            .observe_incoming(ObservedIncomingCall::new(
                call_id.clone(),
                CallMedia::AudioVideo,
                "Подъезд 1".to_owned(),
                SecretString::from("v=0\r\na=ice-ufrag:remote-secret\r\n"),
                SecretString::from("intercom-private-42"),
                SecretString::from("call-session-private-99"),
                now,
            ))
            .await
            .unwrap();
    }
    let state = AppState::default().with_call_service(service.clone());
    let config = AppConfig::from_iter(std::iter::empty::<(&str, &str)>()).unwrap();
    Fixture {
        app: build_router_with_config(state, &config),
        service,
        transport,
        opener,
        call_id,
    }
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

fn json_post(uri: String, body: Value) -> Request<Body> {
    Request::post(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

#[tokio::test]
async fn current_is_empty_until_a_real_invite_and_redacts_provider_identifiers() {
    let empty = fixture(false).await;
    let response = empty
        .app
        .oneshot(
            Request::get("/api/v1/calls/current")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let live = fixture(true).await;
    let response = live
        .app
        .oneshot(
            Request::get("/api/v1/calls/current")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let body = body_json(response).await;
    assert_eq!(body["id"], live.call_id.to_string());
    assert_eq!(body["revision"], 1);
    assert_eq!(body["phase"], "ringing");
    assert_eq!(body["callerName"], "Подъезд 1");
    assert_eq!(body["remoteSdp"], "v=0\r\na=ice-ufrag:remote-secret\r\n");
    assert_eq!(body["canOpen"], false);
    let encoded = serde_json::to_string(&body).unwrap();
    assert!(!encoded.contains("intercom-private-42"));
    assert!(!encoded.contains("call-session-private-99"));
}

#[tokio::test]
async fn answer_forwards_complete_sdp_once_and_uses_revision_concurrency() {
    let fixture = fixture(true).await;
    let uri = format!("/api/v1/calls/{}/answer", fixture.call_id);
    let answer_sdp = "v=0\r\na=ice-ufrag:browser-complete\r\n";
    let request = json_post(uri.clone(), json!({"revision": 1, "sdp": answer_sdp}));
    let response = fixture.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["phase"], "connecting");
    assert_eq!(body["revision"], 2);
    assert_eq!(
        fixture.transport.actions.lock().unwrap().as_slice(),
        &[TransportAction::Answer(
            fixture.call_id.clone(),
            answer_sdp.to_owned()
        )]
    );

    let response = fixture
        .app
        .oneshot(json_post(uri, json!({"revision": 1, "sdp": answer_sdp})))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(fixture.transport.actions.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn confirmed_call_open_is_active_only_one_use_and_bound_to_provider_session() {
    let fixture = fixture(true).await;
    let answer_uri = format!("/api/v1/calls/{}/answer", fixture.call_id);
    fixture
        .app
        .clone()
        .oneshot(json_post(
            answer_uri,
            json!({"revision": 1, "sdp": "v=0\r\na=sendrecv\r\n"}),
        ))
        .await
        .unwrap();
    fixture
        .service
        .mark_connected(&fixture.call_id)
        .await
        .unwrap();

    let create_uri = format!("/api/v1/calls/{}/open-intents", fixture.call_id);
    let created = fixture
        .app
        .clone()
        .oneshot(json_post(create_uri, json!({"revision": 3})))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let intent = body_json(created).await;
    let intent_id = intent["intentId"].as_str().unwrap();
    assert!(intent["expiresAt"].as_str().is_some());
    assert_eq!(fixture.opener.calls.lock().unwrap().len(), 0);

    let execute_uri = format!(
        "/api/v1/calls/{}/open-intents/{intent_id}/execute",
        fixture.call_id
    );
    let executed = fixture
        .app
        .clone()
        .oneshot(json_post(execute_uri.clone(), json!({"revision": 3})))
        .await
        .unwrap();
    assert_eq!(executed.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        fixture.opener.calls.lock().unwrap().as_slice(),
        &[(
            "intercom-private-42".to_owned(),
            "call-session-private-99".to_owned()
        )]
    );

    let replay = fixture
        .app
        .oneshot(json_post(execute_uri, json!({"revision": 3})))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::NOT_FOUND);
    assert_eq!(fixture.opener.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn mutation_requires_json_and_rejects_unbounded_sdp_before_transport() {
    let fixture = fixture(true).await;
    let uri = format!("/api/v1/calls/{}/answer", fixture.call_id);
    let response = fixture
        .app
        .clone()
        .oneshot(Request::post(uri.clone()).body(Body::from("{}")).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let response = fixture
        .app
        .oneshot(json_post(
            uri,
            json!({"revision": 1, "sdp": "x".repeat(300_000)}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(fixture.transport.actions.lock().unwrap().is_empty());
}

#[tokio::test]
async fn decline_ignore_and_hangup_use_distinct_transport_actions() {
    let decline = fixture(true).await;
    let response = decline
        .app
        .oneshot(json_post(
            format!("/api/v1/calls/{}/decline", decline.call_id),
            json!({"revision": 1}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        decline.transport.actions.lock().unwrap().as_slice(),
        &[TransportAction::Decline(decline.call_id.clone())]
    );

    let ignore = fixture(true).await;
    let response = ignore
        .app
        .oneshot(json_post(
            format!("/api/v1/calls/{}/ignore", ignore.call_id),
            json!({"revision": 1}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        ignore.transport.actions.lock().unwrap().as_slice(),
        &[TransportAction::Ignore(ignore.call_id.clone())]
    );

    let hangup = fixture(true).await;
    hangup
        .app
        .clone()
        .oneshot(json_post(
            format!("/api/v1/calls/{}/answer", hangup.call_id),
            json!({"revision": 1, "sdp": "v=0\r\n"}),
        ))
        .await
        .unwrap();
    hangup
        .service
        .mark_connected(&hangup.call_id)
        .await
        .unwrap();
    let response = hangup
        .app
        .oneshot(json_post(
            format!("/api/v1/calls/{}/hangup", hangup.call_id),
            json!({"revision": 3}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        hangup.transport.actions.lock().unwrap().last(),
        Some(&TransportAction::Hangup(hangup.call_id.clone()))
    );
}

#[derive(Clone, Copy)]
struct ProviderTokens;

#[async_trait]
impl TokenSource for ProviderTokens {
    async fn access_token(&self) -> Option<SecretString> {
        Some(SecretString::from("call-access-secret"))
    }

    async fn device_token(&self) -> Option<SecretString> {
        Some(SecretString::from("call-device-secret"))
    }

    async fn refresh_after_unauthorized(&self) -> Result<(), AppError> {
        panic!("call mutations must not retry after refresh")
    }
}

fn provider_client(server: &MockServer) -> MtsClient {
    let base = Url::parse(&format!("{}/", server.uri())).unwrap();
    MtsClient::new_for_test(base.clone(), base, Arc::new(ProviderTokens)).unwrap()
}

fn provider_client_with_bases(gateway: &MockServer, resident: &MockServer) -> MtsClient {
    let gateway = Url::parse(&format!("{}/", gateway.uri())).unwrap();
    let resident = Url::parse(&format!("{}/", resident.uri())).unwrap();
    MtsClient::new_for_test(gateway, resident, Arc::new(ProviderTokens)).unwrap()
}

#[tokio::test]
async fn call_open_provider_sends_call_scoped_header_without_a_lock_number() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/vdome-smart-facade/devices/api/v1/intercoms/intercom%20%2F%2042/lock",
        ))
        .and(wire_header("x-call-session-id", "session-private-99"))
        .and(wire_body_json(json!({"action": "open"})))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let provider = MtsCallOpenProvider::new(provider_client(&server));

    provider
        .open(
            &SecretString::from("intercom / 42"),
            &SecretString::from("session-private-99"),
        )
        .await
        .unwrap();

    let request = &server.received_requests().await.unwrap()[0];
    assert_eq!(
        request.headers["authorization"],
        "Bearer call-access-secret"
    );
    assert_eq!(request.headers["x-device-token"], "call-device-secret");
    assert!(request.headers.get("x-auth-token").is_none());
}

#[tokio::test]
async fn remote_ignore_action_uses_the_opaque_session_route_and_explicit_empty_body() {
    let gateway = MockServer::start().await;
    let resident = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/domofon/api/intercom/v1/calls/session%20%2F%2099/action/ignore",
        ))
        .and(wire_header("content-length", "0"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&gateway)
        .await;
    let provider = MtsCallActionProvider::new(provider_client_with_bases(&gateway, &resident));

    provider
        .action(
            &SecretString::from("session / 99"),
            RemoteCallAction::Ignore,
        )
        .await
        .unwrap();

    assert!(
        gateway.received_requests().await.unwrap()[0]
            .body
            .is_empty()
    );
    assert!(resident.received_requests().await.unwrap().is_empty());
}
