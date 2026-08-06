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
    device: Arc<RwLock<Option<String>>>,
    refresh_device: Option<String>,
    access_reads: Arc<AtomicUsize>,
    device_reads: Arc<AtomicUsize>,
    refreshes: Arc<AtomicUsize>,
    refresh_barrier: Option<Arc<tokio::sync::Barrier>>,
}

impl TestTokens {
    fn new(device: Option<&str>) -> Self {
        Self {
            access: Arc::new(RwLock::new("access-test".into())),
            device: Arc::new(RwLock::new(device.map(str::to_owned))),
            refresh_device: None,
            access_reads: Arc::new(AtomicUsize::new(0)),
            device_reads: Arc::new(AtomicUsize::new(0)),
            refreshes: Arc::new(AtomicUsize::new(0)),
            refresh_barrier: None,
        }
    }

    fn with_refresh_barrier(mut self, barrier: Arc<tokio::sync::Barrier>) -> Self {
        self.refresh_barrier = Some(barrier);
        self
    }

    fn with_refresh_device(mut self, device: &str) -> Self {
        self.refresh_device = Some(device.to_owned());
        self
    }

    async fn replace(&self, access: &str, device: Option<&str>) {
        *self.access.write().await = access.into();
        *self.device.write().await = device.map(str::to_owned);
    }
}

#[async_trait]
impl TokenSource for TestTokens {
    async fn access_token(&self) -> Option<SecretString> {
        self.access_reads.fetch_add(1, Ordering::SeqCst);
        Some(SecretString::from(self.access.read().await.clone()))
    }

    async fn device_token(&self) -> Option<SecretString> {
        self.device_reads.fetch_add(1, Ordering::SeqCst);
        self.device.read().await.clone().map(SecretString::from)
    }

    async fn refresh_after_unauthorized(&self) -> Result<(), AppError> {
        self.refreshes.fetch_add(1, Ordering::SeqCst);
        if let Some(barrier) = &self.refresh_barrier {
            barrier.wait().await;
        }
        *self.access.write().await = "access-fresh".into();
        if let Some(device) = &self.refresh_device {
            *self.device.write().await = Some(device.clone());
        }
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
    MtsClient::new_for_test(base.clone(), base, tokens).unwrap()
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
    let tokens = Arc::new(TestTokens::new(Some("device-test")).with_refresh_device("device-fresh"));
    let client = client(&server, tokens.clone());

    let request = client
        .authorized(Method::GET, ProviderBase::Gateway, "/resource")
        .unwrap();
    let response: OkResponse = client.send_json(request).await.unwrap();

    assert!(response.ok);
    assert_eq!(tokens.refreshes.load(Ordering::SeqCst), 1);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].headers["authorization"], "Bearer access-test");
    assert!(requests[0].headers.get("x-auth-token").is_none());
    assert_eq!(requests[0].headers["x-device-token"], "device-test");
    assert_eq!(requests[1].headers["authorization"], "Bearer access-fresh");
    assert_eq!(requests[1].headers["x-auth-token"], "access-fresh");
    assert_eq!(requests[1].headers["x-device-token"], "device-fresh");
    let mut request_ids = Vec::new();
    for request in requests {
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
    let refresh_barrier = Arc::new(tokio::sync::Barrier::new(2));
    let tokens = Arc::new(TestTokens::new(None).with_refresh_barrier(refresh_barrier.clone()));
    let client = client(&server, tokens.clone());

    let mut tasks = Vec::new();
    for _ in 0..20 {
        let client = client.clone();
        tasks.push(tokio::spawn(async move {
            let request = client
                .authorized(Method::GET, ProviderBase::Gateway, "/resource")
                .unwrap();
            client.send_json::<OkResponse>(request).await.unwrap().ok
        }));
    }
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let stale_attempts = server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .filter(|request| request.headers["authorization"] == "Bearer access-test")
                .count();
            if stale_attempts == 20 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("all reads must dispatch the stale credential before refresh");
    refresh_barrier.wait().await;
    for task in tasks {
        assert!(task.await.unwrap());
    }

    assert_eq!(tokens.refreshes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn prepared_request_fetches_credentials_and_request_id_at_dispatch_time() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mutation"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    let tokens = Arc::new(TestTokens::new(Some("device-old")));
    let client = client(&server, tokens.clone());

    let request = client
        .authorized(Method::POST, ProviderBase::Gateway, "/mutation")
        .unwrap();
    assert_eq!(tokens.access_reads.load(Ordering::SeqCst), 0);
    assert_eq!(tokens.device_reads.load(Ordering::SeqCst), 0);
    tokens.replace("access-fresh", Some("device-fresh")).await;
    client.send_empty(request).await.unwrap();

    assert_eq!(tokens.access_reads.load(Ordering::SeqCst), 1);
    assert_eq!(tokens.device_reads.load(Ordering::SeqCst), 1);

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].headers["authorization"], "Bearer access-fresh");
    assert_eq!(requests[0].headers["x-device-token"], "device-fresh");
}

#[tokio::test]
async fn client_owned_path_segment_keeps_opaque_data_out_of_url_structure() {
    let server = MockServer::start().await;
    let cases = [
        ("flat%percent", "/resource/flat%25percent/available"),
        ("flat/section", "/resource/flat%2Fsection/available"),
        (
            "flat?query#fragment",
            "/resource/flat%3Fquery%23fragment/available",
        ),
        (
            "квартира",
            "/resource/%D0%BA%D0%B2%D0%B0%D1%80%D1%82%D0%B8%D1%80%D0%B0/available",
        ),
        (
            "//evil.example/../flat",
            "/resource/%2F%2Fevil.example%2F..%2Fflat/available",
        ),
    ];
    for (_, expected_path) in cases {
        Mock::given(method("GET"))
            .and(path(expected_path))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
    }
    let client = client(&server, Arc::new(TestTokens::new(None)));

    for (segment, _) in cases {
        let request = client
            .authorized_with_path_segment(
                Method::GET,
                ProviderBase::Gateway,
                "/resource",
                segment,
                Some("available"),
            )
            .unwrap();
        client.send_empty(request).await.unwrap();
    }

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), cases.len());
    let received_hosts = requests
        .iter()
        .map(|request| request.url.host_str().map(str::to_owned))
        .collect::<Vec<_>>();
    assert!(
        received_hosts
            .iter()
            .all(|host| host.as_deref() != Some("evil.example")),
        "opaque data escaped to {received_hosts:?}"
    );
}

#[tokio::test]
async fn typed_path_segment_rejects_dot_ambiguity_and_unsafe_fixed_structure() {
    let server = MockServer::start().await;
    let client = client(&server, Arc::new(TestTokens::new(None)));

    for segment in [".", ".."] {
        let error = client
            .authorized_with_path_segment(
                Method::GET,
                ProviderBase::Gateway,
                "/resource",
                segment,
                Some("available"),
            )
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }
    for fixed_prefix in [
        "//evil.example/resource",
        "/../resource",
        "/%2e%2e/resource",
    ] {
        let error = client
            .authorized_with_path_segment(
                Method::GET,
                ProviderBase::Gateway,
                fixed_prefix,
                "flat-synthetic",
                Some("available"),
            )
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn reused_prepared_request_gets_fresh_credentials_and_uuid_on_every_dispatch() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/resource"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
        .mount(&server)
        .await;
    let tokens = Arc::new(TestTokens::new(Some("device-one")));
    let client = client(&server, tokens.clone());
    let prepared = client
        .authorized(Method::GET, ProviderBase::Gateway, "/resource")
        .unwrap();

    assert!(
        client
            .send_json::<OkResponse>(prepared.clone())
            .await
            .unwrap()
            .ok
    );
    tokens.replace("access-two", Some("device-two")).await;
    assert!(client.send_json::<OkResponse>(prepared).await.unwrap().ok);

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].headers["authorization"], "Bearer access-test");
    assert_eq!(requests[0].headers["x-device-token"], "device-one");
    assert_eq!(requests[1].headers["authorization"], "Bearer access-two");
    assert_eq!(requests[1].headers["x-device-token"], "device-two");
    assert!(
        requests
            .iter()
            .all(|request| request.headers.get("x-auth-token").is_none())
    );
    assert_ne!(
        requests[0].headers["request-id"],
        requests[1].headers["request-id"]
    );
}

#[tokio::test]
async fn send_methods_reject_a_prepared_request_owned_by_another_client() {
    let server = MockServer::start().await;
    let other_server = MockServer::start().await;
    let primary_client = client(&server, Arc::new(TestTokens::new(None)));
    let other_client = client(&other_server, Arc::new(TestTokens::new(None)));
    let request = other_client
        .authorized(Method::GET, ProviderBase::Gateway, "/bypass")
        .unwrap();

    let error = primary_client.send_empty(request).await.unwrap_err();

    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(server.received_requests().await.unwrap().is_empty());
    assert!(other_server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn test_support_client_does_not_follow_redirects() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/redirect"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "/followed"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/followed"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    let client = client(&server, Arc::new(TestTokens::new(None)));

    let request = client
        .authorized(Method::GET, ProviderBase::Gateway, "/redirect")
        .unwrap();
    let error = client.send_empty(request).await.unwrap_err();

    assert_eq!(error.code, ErrorCode::UpstreamUnavailable);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.path(), "/redirect");
}

#[tokio::test]
async fn a_second_unauthorized_response_is_not_retried_or_refreshed_again() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/resource"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let tokens = Arc::new(
        TestTokens::new(Some("device-secret-old")).with_refresh_device("device-secret-fresh"),
    );
    let client = client(&server, tokens.clone());

    let request = client
        .authorized(Method::GET, ProviderBase::Gateway, "/resource")
        .unwrap();
    let prepared_debug = format!("{request:?}");
    for secret in [
        "access-test",
        "access-fresh",
        "device-secret-old",
        "device-secret-fresh",
    ] {
        assert!(!prepared_debug.contains(secret));
    }
    let error = client.send_json::<OkResponse>(request).await.unwrap_err();

    assert_eq!(error.code, ErrorCode::SessionExpired);
    assert_eq!(tokens.refreshes.load(Ordering::SeqCst), 1);
    let error_debug = format!("{error:?}");
    let error_display = error.to_string();
    for secret in [
        "access-test",
        "access-fresh",
        "device-secret-old",
        "device-secret-fresh",
    ] {
        assert!(!error_debug.contains(secret));
        assert!(!error_display.contains(secret));
    }
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].headers.get("x-auth-token").is_none());
    assert_eq!(requests[1].headers["authorization"], "Bearer access-fresh");
    assert_eq!(requests[1].headers["x-auth-token"], "access-fresh");
    assert_eq!(requests[1].headers["x-device-token"], "device-secret-fresh");
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
        .unwrap()
        .json(&serde_json::json!({"action": "open"}))
        .unwrap();
    let error = client.send_empty(request).await.unwrap_err();

    assert_eq!(error.code, ErrorCode::UpstreamAuthenticationFailed);
    assert_eq!(tokens.refreshes.load(Ordering::SeqCst), 0);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body, br#"{"action":"open"}"#);
    assert_eq!(requests[0].headers["content-type"], "application/json");
    assert!(requests[0].headers.get("x-auth-token").is_none());
}

#[tokio::test]
async fn successful_unit_status_is_authoritative_despite_oversized_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/unit-with-unexpected-body"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; 32 * 1024]))
        .expect(1)
        .mount(&server)
        .await;
    let client = client(&server, Arc::new(TestTokens::new(None)));

    let request = client
        .authorized(
            Method::POST,
            ProviderBase::Gateway,
            "/unit-with-unexpected-body",
        )
        .unwrap();
    client.send_empty(request).await.unwrap();

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
            .unwrap();
        let error = client.send_empty(request).await.unwrap_err();
        assert_eq!(error.code, code);
        let expected_request_id = format!("upstream-{name}");
        assert_eq!(
            error.upstream_request_id.as_deref(),
            Some(expected_request_id.as_str())
        );
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
        .unwrap();
    let error = client.send_empty(request).await.unwrap_err();

    assert_eq!(error.code, ErrorCode::RateLimited);
    assert!(error.retryable);
    assert_eq!(error.retry_after_seconds, Some(17));
    assert_eq!(
        error.upstream_request_id.as_deref(),
        Some("provider-correlation")
    );
}

#[tokio::test]
async fn retry_after_supports_http_dates_and_caps_untrusted_delays() {
    let server = MockServer::start().await;
    let cases = [
        ("date-limited", "Wed, 21 Oct 2099 07:28:00 GMT", Some(3_600)),
        ("huge-limited", "999999999", Some(3_600)),
        ("past-limited", "Wed, 21 Oct 2015 07:28:00 GMT", Some(0)),
    ];
    for (path_name, value, _) in cases {
        Mock::given(method("POST"))
            .and(path(format!("/{path_name}")))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", value))
            .mount(&server)
            .await;
    }
    let client = client(&server, Arc::new(TestTokens::new(None)));

    for (path_name, _, expected) in cases {
        let request = client
            .authorized(
                Method::POST,
                ProviderBase::Gateway,
                &format!("/{path_name}"),
            )
            .unwrap();
        let error = client.send_empty(request).await.unwrap_err();
        assert_eq!(error.retry_after_seconds, expected);
    }
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
        .unwrap();
    let response = client.send_stream(request).await.unwrap();

    assert_eq!(response.bytes().await.unwrap(), &b"stream-body"[..]);
}
