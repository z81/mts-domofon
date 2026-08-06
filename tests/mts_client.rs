use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use domru_mts::{
    error::{AppError, ErrorCode},
    provider::mts::{MtsClient, ProviderBase, TokenSource},
};
use reqwest::Method;
use secrecy::SecretString;
use serde::Deserialize;
use tokio::sync::RwLock;
use url::Url;
use uuid::Uuid;
use wiremock::{
    Mock, MockServer, Request, Respond, ResponseTemplate,
    matchers::{method, path},
};

#[derive(Clone)]
struct TestTokens {
    access: Arc<RwLock<String>>,
    device: Option<String>,
    refreshes: Arc<AtomicUsize>,
}

impl TestTokens {
    fn new(device: Option<&str>) -> Self {
        Self {
            access: Arc::new(RwLock::new("access-test".into())),
            device: device.map(str::to_owned),
            refreshes: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait]
impl TokenSource for TestTokens {
    async fn access_token(&self) -> Option<SecretString> {
        Some(SecretString::from(self.access.read().await.clone()))
    }

    async fn device_token(&self) -> Option<SecretString> {
        self.device.clone().map(SecretString::from)
    }

    async fn refresh_after_unauthorized(&self) -> Result<(), AppError> {
        self.refreshes.fetch_add(1, Ordering::SeqCst);
        *self.access.write().await = "access-fresh".into();
        Ok(())
    }
}

#[derive(Clone)]
struct AuthResponder;

impl Respond for AuthResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let authorization = request.headers["authorization"].to_str().unwrap();
        if authorization == "Bearer access-test" {
            ResponseTemplate::new(401)
        } else {
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true}))
        }
    }
}

fn client(server: &MockServer, tokens: Arc<dyn TokenSource>) -> MtsClient {
    let base = Url::parse(&format!("{}/", server.uri())).unwrap();
    MtsClient::new_for_test(reqwest::Client::new(), base.clone(), base, tokens)
}

#[derive(Debug, Deserialize)]
struct OkResponse {
    ok: bool,
}

#[tokio::test]
async fn safe_read_has_exact_headers_and_retries_once_with_fresh_credentials() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/resource"))
        .respond_with(AuthResponder)
        .mount(&server)
        .await;
    let tokens = Arc::new(TestTokens::new(Some("device-test")));
    let client = client(&server, tokens.clone());

    let request = client
        .authorized(Method::GET, ProviderBase::Gateway, "/resource")
        .await
        .unwrap();
    let response: OkResponse = client.send_json(request).await.unwrap();

    assert!(response.ok);
    assert_eq!(tokens.refreshes.load(Ordering::SeqCst), 1);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].headers["authorization"], "Bearer access-test");
    assert_eq!(requests[1].headers["authorization"], "Bearer access-fresh");
    let mut request_ids = Vec::new();
    for request in requests {
        assert_eq!(request.headers["x-device-token"], "device-test");
        assert_eq!(request.headers["x-device-type"], "ANDROID");
        assert_eq!(request.headers["accept"], "application/json");
        let request_id = request.headers["request-id"].to_str().unwrap();
        assert!(Uuid::parse_str(request_id).is_ok());
        request_ids.push(request_id.to_owned());
    }
    assert_ne!(request_ids[0], request_ids[1]);
}

#[tokio::test]
async fn concurrent_unauthorized_reads_share_one_serialized_refresh() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/resource"))
        .respond_with(AuthResponder)
        .mount(&server)
        .await;
    let tokens = Arc::new(TestTokens::new(None));
    let client = client(&server, tokens.clone());

    let mut tasks = Vec::new();
    for _ in 0..20 {
        let client = client.clone();
        tasks.push(tokio::spawn(async move {
            let request = client
                .authorized(Method::GET, ProviderBase::Gateway, "/resource")
                .await
                .unwrap();
            client.send_json::<OkResponse>(request).await.unwrap().ok
        }));
    }
    for task in tasks {
        assert!(task.await.unwrap());
    }

    assert_eq!(tokens.refreshes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_second_unauthorized_response_is_not_retried_or_refreshed_again() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/resource"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let tokens = Arc::new(TestTokens::new(None));
    let client = client(&server, tokens.clone());

    let request = client
        .authorized(Method::GET, ProviderBase::Gateway, "/resource")
        .await
        .unwrap();
    let error = client.send_json::<OkResponse>(request).await.unwrap_err();

    assert_eq!(error.code, ErrorCode::SessionExpired);
    assert_eq!(tokens.refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn mutation_is_never_refreshed_or_retried_after_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/door/open"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let tokens = Arc::new(TestTokens::new(None));
    let client = client(&server, tokens.clone());

    let request = client
        .authorized(Method::POST, ProviderBase::Gateway, "/door/open")
        .await
        .unwrap()
        .json(&serde_json::json!({"action": "open"}));
    let error = client.send_empty(request).await.unwrap_err();

    assert_eq!(error.code, ErrorCode::UpstreamAuthenticationFailed);
    assert_eq!(tokens.refreshes.load(Ordering::SeqCst), 0);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn classifies_stable_http_errors_and_bounds_redacted_error_bodies() {
    let server = MockServer::start().await;
    let cases = [
        ("forbidden", 403, ErrorCode::Forbidden),
        ("missing", 404, ErrorCode::NotFound),
        ("conflict", 409, ErrorCode::InvalidRequest),
        ("invalid", 422, ErrorCode::InvalidRequest),
        ("unavailable", 503, ErrorCode::UpstreamUnavailable),
    ];
    for (name, status, _) in cases {
        Mock::given(method("POST"))
            .and(path(format!("/{name}")))
            .respond_with(
                ResponseTemplate::new(status)
                    .insert_header("X-Request-Id", format!("upstream-{name}"))
                    .set_body_string(format!("secret-{name}{}", "x".repeat(32_768))),
            )
            .mount(&server)
            .await;
    }
    let client = client(&server, Arc::new(TestTokens::new(None)));

    for (name, _, code) in cases {
        let request = client
            .authorized(Method::POST, ProviderBase::Gateway, &format!("/{name}"))
            .await
            .unwrap();
        let error = client.send_empty(request).await.unwrap_err();
        assert_eq!(error.code, code);
        assert_eq!(error.upstream_request_id.as_deref(), Some(&format!("upstream-{name}")));
        let formatted = format!("{error:?}");
        assert!(!formatted.contains(&format!("secret-{name}")));
        assert!(formatted.len() < 1_000);
    }
}

#[tokio::test]
async fn rate_limit_carries_retry_after_and_provider_correlation() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/limited"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("Request-Id", "provider-correlation")
                .insert_header("Retry-After", "17"),
        )
        .mount(&server)
        .await;
    let client = client(&server, Arc::new(TestTokens::new(None)));

    let request = client
        .authorized(Method::POST, ProviderBase::Gateway, "/limited")
        .await
        .unwrap();
    let error = client.send_empty(request).await.unwrap_err();

    assert_eq!(error.code, ErrorCode::RateLimited);
    assert!(error.retryable);
    assert_eq!(error.retry_after_seconds, Some(17));
    assert_eq!(error.upstream_request_id.as_deref(), Some("provider-correlation"));
}

#[tokio::test]
async fn stream_uses_the_authorized_contract_without_json_decoding() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/stream"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"stream-body"))
        .mount(&server)
        .await;
    let client = client(&server, Arc::new(TestTokens::new(None)));

    let request = client
        .authorized(Method::GET, ProviderBase::Resident, "/stream")
        .await
        .unwrap();
    let response = client.send_stream(request).await.unwrap();

    assert_eq!(response.bytes().await.unwrap(), &b"stream-body"[..]);
}
