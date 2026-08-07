use std::{
    io::Write,
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    body::Body,
    extract::Extension,
    http::{Request, StatusCode, Uri, header, header::HeaderValue},
    response::IntoResponse,
    routing::get,
};
use domru_mts::{
    app,
    app::RedactedUri,
    error::{ApiError, AppError},
    request_id::{self, RequestId},
};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;
use tracing::instrument::WithSubscriber;
use tracing_subscriber::fmt::MakeWriter;
use uuid::Uuid;

static TRACING_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Clone)]
struct BufferWriter(Arc<Mutex<Vec<u8>>>);

impl Write for BufferWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'writer> MakeWriter<'writer> for BufferWriter {
    type Writer = Self;

    fn make_writer(&'writer self) -> Self::Writer {
        self.clone()
    }
}

async fn body_json(response: axum::response::Response) -> Value {
    let body = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn error_response_is_stable_and_hides_cause() {
    let _tracing_guard = TRACING_TEST_LOCK.lock().await;
    let err = ApiError::new(
        RequestId::parse("e7411f58-d5e9-4a43-b25d-88ddf2014713").unwrap(),
        AppError::upstream_auth("Bearer secret-token", Some("mts-42".into())),
    );

    let response = err.into_response();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let json = body_json(response).await;
    assert_eq!(json["code"], "upstream_authentication_failed");
    assert_eq!(json["retryable"], false);
    assert_eq!(json["requestId"], "e7411f58-d5e9-4a43-b25d-88ddf2014713");
    assert!(json.get("upstreamRequestId").is_none());
    assert!(!json.to_string().contains("secret-token"));
    assert!(!json.to_string().contains("mts-42"));
}

#[tokio::test(flavor = "current_thread")]
async fn error_logging_redacts_the_cause_and_upstream_request_id() {
    let _tracing_guard = TRACING_TEST_LOCK.lock().await;
    let output = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_writer(BufferWriter(Arc::clone(&output)))
        .finish();
    let dispatch = tracing::Dispatch::new(subscriber);
    let error = ApiError::new(
        RequestId::parse("e7411f58-d5e9-4a43-b25d-88ddf2014713").unwrap(),
        AppError::upstream_auth("Bearer secret-token", Some("mts-42".into())),
    );

    tracing::dispatcher::with_default(&dispatch, || {
        let _response = error.into_response();
    });

    let formatted = String::from_utf8(output.lock().unwrap().clone()).unwrap();
    assert!(formatted.contains("[REDACTED]"));
    assert!(!formatted.contains("secret-token"));
    assert!(!formatted.contains("mts-42"));
}

#[tokio::test(flavor = "current_thread")]
async fn request_tracing_redacts_sensitive_query_values() {
    let _tracing_guard = TRACING_TEST_LOCK.lock().await;
    let output = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_writer(BufferWriter(Arc::clone(&output)))
        .finish();
    let dispatch = tracing::Dispatch::new(subscriber);
    let uri = concat!(
        "/health/live?camera=front-door",
        "&token=token-secret",
        "&access_token=access-secret",
        "&key=key-secret",
        "&signature=signature-secret",
        "&sig=sig-secret",
        "&expires=expires-secret",
        "&media_token=media-token-secret",
        "&media_key=media-key-secret",
        "&media_lease=media-lease-secret",
        "&quality=hd"
    );

    let response = app::test_router()
        .await
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .with_subscriber(dispatch)
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let formatted = String::from_utf8(output.lock().unwrap().clone()).unwrap();
    assert!(formatted.contains("/health/live"));
    assert!(formatted.contains("camera=front-door"));
    assert!(formatted.contains("quality=hd"));
    for key in [
        "token",
        "access_token",
        "key",
        "signature",
        "sig",
        "expires",
        "media_token",
        "media_key",
        "media_lease",
    ] {
        assert!(
            formatted.contains(&format!("{key}=%5BREDACTED%5D")),
            "trace omitted the redacted {key} field"
        );
    }
    for secret in [
        "token-secret",
        "access-secret",
        "key-secret",
        "signature-secret",
        "sig-secret",
        "expires-secret",
        "media-token-secret",
        "media-key-secret",
        "media-lease-secret",
    ] {
        assert!(!formatted.contains(secret), "trace exposed {secret}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn request_tracing_redacts_the_entire_mts_relay_query() {
    let _tracing_guard = TRACING_TEST_LOCK.lock().await;
    let output = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_writer(BufferWriter(Arc::clone(&output)))
        .finish();
    let dispatch = tracing::Dispatch::new(subscriber);

    let app = app::test_router().await;
    for path in [
        "/amserver?ordinary=visible-looking&code=relay-secret",
        "/amserver/oauth2/authorize?ordinary=visible-looking&code=relay-secret",
        "/wsso/authenticate?ordinary=visible-looking&code=relay-secret",
        "/amserver%2Foauth2%2Fauthorize?ordinary=encoded-visible&code=encoded-relay-secret",
    ] {
        let response = app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .with_subscriber(dispatch.clone())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
    let formatted = String::from_utf8(output.lock().unwrap().clone()).unwrap();
    assert!(formatted.contains("/amserver/oauth2/authorize"));
    assert!(formatted.contains("[REDACTED]"));
    assert!(!formatted.contains("ordinary"));
    assert!(!formatted.contains("visible-looking"));
    assert!(!formatted.contains("relay-secret"));
    assert!(!formatted.contains("encoded-visible"));
    assert!(!formatted.contains("encoded-relay-secret"));
}

#[tokio::test]
async fn handler_receives_original_uri_and_redacted_uri_extension() {
    let _tracing_guard = TRACING_TEST_LOCK.lock().await;
    async fn handler(uri: Uri, Extension(redacted_uri): Extension<RedactedUri>) -> String {
        format!("{uri}\n{redacted_uri}")
    }

    let app = app::with_http_middleware(Router::new().route("/media/live", get(handler)));
    let original = "/media/live?token=token-secret&camera=front-door";
    let response = app
        .oneshot(Request::get(original).body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        &body[..],
        b"/media/live?token=token-secret&camera=front-door\n/media/live?token=%5BREDACTED%5D&camera=front-door"
    );
}

#[tokio::test]
async fn camera_live_scope_is_redacted_from_request_metadata() {
    let _tracing_guard = TRACING_TEST_LOCK.lock().await;
    async fn handler(Extension(redacted_uri): Extension<RedactedUri>) -> String {
        redacted_uri.to_string()
    }

    let app = app::with_http_middleware(
        Router::new().route("/api/v1/cameras/{camera_id}/live", get(handler)),
    );
    let response = app
        .oneshot(
            Request::get("/api/v1/cameras/private-camera/live?flatId=private-flat")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"/api/v1/cameras/[REDACTED]/live?[REDACTED]");
}

#[tokio::test]
async fn camera_archive_scope_and_time_are_redacted_from_request_metadata() {
    // Catches archive camera IDs, flat IDs, or selected timestamps entering request traces.
    let _tracing_guard = TRACING_TEST_LOCK.lock().await;
    async fn handler(Extension(redacted_uri): Extension<RedactedUri>) -> String {
        redacted_uri.to_string()
    }

    let app = app::with_http_middleware(
        Router::new()
            .route("/api/v1/cameras/{camera_id}/archive", get(handler))
            .route(
                "/api/v1/cameras/{camera_id}/archive/distribution",
                get(handler),
            ),
    );
    for (path, expected) in [
        (
            "/api/v1/cameras/private-camera/archive?flatId=private-flat&start=private-time",
            "/api/v1/cameras/[REDACTED]/archive?[REDACTED]",
        ),
        (
            "/api/v1/cameras/private-camera/archive/distribution?flatId=private-flat",
            "/api/v1/cameras/[REDACTED]/archive/distribution?[REDACTED]",
        ),
    ] {
        let response = app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, expected.as_bytes());
    }
}

#[tokio::test]
async fn auto_open_confirmation_identifier_is_redacted_from_request_metadata() {
    let _tracing_guard = TRACING_TEST_LOCK.lock().await;
    async fn handler(Extension(redacted_uri): Extension<RedactedUri>) -> String {
        redacted_uri.to_string()
    }

    let app = app::with_http_middleware(Router::new().route(
        "/api/v1/calls/auto-open/intents/{intent_id}/execute",
        get(handler),
    ));
    let response = app
        .oneshot(
            Request::get(
                "/api/v1/calls/auto-open/intents/private-intent/execute?future=private-value",
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        &body[..],
        b"/api/v1/calls/auto-open/intents/[REDACTED]/execute?[REDACTED]"
    );
}

#[tokio::test]
async fn valid_request_id_is_retained_in_the_response() {
    let _tracing_guard = TRACING_TEST_LOCK.lock().await;
    let app = app::test_router().await;
    let request_id = "e7411f58-d5e9-4a43-b25d-88ddf2014713";

    let response = app
        .oneshot(
            Request::get("/health/live")
                .header("x-request-id", request_id)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-request-id"], request_id);
}

#[tokio::test]
async fn malformed_or_oversized_request_ids_are_replaced() {
    let _tracing_guard = TRACING_TEST_LOCK.lock().await;
    for inbound in ["not-a-uuid", &"a".repeat(8_192)] {
        let app = app::test_router().await;
        let response = app
            .oneshot(
                Request::get("/health/live")
                    .header("x-request-id", inbound)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let canonical = response.headers()["x-request-id"].to_str().unwrap();
        assert_ne!(canonical, inbound);
        assert!(Uuid::parse_str(canonical).is_ok());
    }
}

#[tokio::test]
async fn error_json_request_id_matches_the_response_header() {
    let _tracing_guard = TRACING_TEST_LOCK.lock().await;
    async fn handler(Extension(request_id): Extension<RequestId>) -> ApiError {
        ApiError::new(
            request_id,
            AppError::upstream_auth("Bearer secret-token", Some("mts-42".into())),
        )
    }

    let app = Router::new()
        .route("/failure", get(handler))
        .layer(request_id::layer());
    let response = app
        .oneshot(Request::get("/failure").body(Body::empty()).unwrap())
        .await
        .unwrap();

    let header_request_id = response.headers()["x-request-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let json = body_json(response).await;
    assert_eq!(json["requestId"], header_request_id);
    assert!(!json.to_string().contains("secret-token"));
}

#[tokio::test]
async fn sensitive_headers_remain_sensitive_at_the_handler_boundary() {
    let _tracing_guard = TRACING_TEST_LOCK.lock().await;
    async fn handler(request: Request<Body>) -> StatusCode {
        for name in [
            "authorization",
            "cookie",
            "set-cookie",
            "x-device-token",
            "x-media-token",
            "x-media-key",
            "x-media-lease",
        ] {
            assert!(request.headers()[name].is_sensitive(), "{name} was exposed");
        }
        StatusCode::NO_CONTENT
    }

    let app = Router::new()
        .route("/check", get(handler))
        .layer(app::sensitive_headers_layer());
    let response = app
        .oneshot(
            Request::get("/check")
                .header(
                    "authorization",
                    HeaderValue::from_static("Bearer secret-token"),
                )
                .header("cookie", HeaderValue::from_static("session=secret-cookie"))
                .header(
                    "set-cookie",
                    HeaderValue::from_static("session=secret-cookie"),
                )
                .header("x-device-token", HeaderValue::from_static("device-secret"))
                .header("x-media-token", HeaderValue::from_static("media-secret"))
                .header("x-media-key", HeaderValue::from_static("media-key-secret"))
                .header(
                    "x-media-lease",
                    HeaderValue::from_static("media-lease-secret"),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn set_cookie_response_header_is_marked_sensitive() {
    let _tracing_guard = TRACING_TEST_LOCK.lock().await;
    async fn handler() -> axum::response::Response {
        let mut response = StatusCode::NO_CONTENT.into_response();
        response.headers_mut().insert(
            header::SET_COOKIE,
            HeaderValue::from_static("session=secret-cookie"),
        );
        response
    }

    let app = Router::new()
        .route("/cookie", get(handler))
        .layer(app::sensitive_headers_layer());
    let response = app
        .oneshot(Request::get("/cookie").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(response.headers()[header::SET_COOKIE].is_sensitive());
}
