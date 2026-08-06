use std::{
    io::Write,
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    body::Body,
    extract::Extension,
    http::{Request, StatusCode, header::HeaderValue},
    response::IntoResponse,
    routing::get,
};
use domru_mts::{
    app,
    error::{ApiError, AppError},
    request_id::{self, RequestId},
};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;
use tracing_subscriber::fmt::MakeWriter;
use uuid::Uuid;

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

#[test]
fn error_logging_redacts_the_cause_and_upstream_request_id() {
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

#[tokio::test]
async fn valid_request_id_is_retained_in_the_response() {
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
    async fn handler(request: Request<Body>) -> StatusCode {
        for name in [
            "authorization",
            "cookie",
            "set-cookie",
            "x-device-token",
            "x-media-token",
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
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}
