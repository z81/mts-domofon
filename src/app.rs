use std::fmt;

use axum::{
    Router,
    extract::Request,
    http::{Uri, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use tower_http::{
    sensitive_headers::SetSensitiveHeadersLayer,
    trace::{DefaultOnRequest, DefaultOnResponse, TraceLayer},
};
use tracing::{Level, Span};

use crate::{
    api,
    config::AppConfig,
    error::{ApiError, AppError, ErrorCode},
    request_id::{self, RequestId},
    state::AppState,
    web::WebAssets,
};

const SENSITIVE_QUERY_KEYS: &[&str] = &[
    "token",
    "access_token",
    "idt_token",
    "web_sso_access_token",
    "device_token",
    "key",
    "signature",
    "sig",
    "expires",
    "media_token",
    "media_key",
    "media_lease",
    "media_credential",
    "stream_token",
    "playback_token",
    "x-media-token",
    "x-media-key",
    "x-media-lease",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RedactedUri(String);

impl RedactedUri {
    fn from_uri(uri: &Uri) -> Self {
        let mut output = uri.path().to_owned();
        if let Some(query) = uri.query() {
            let mut serializer = url::form_urlencoded::Serializer::new(String::new());
            for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
                if is_sensitive_query_key(&key) {
                    serializer.append_pair(&key, "[REDACTED]");
                } else {
                    serializer.append_pair(&key, &value);
                }
            }
            output.push('?');
            output.push_str(&serializer.finish());
        }
        Self(output)
    }
}

impl fmt::Display for RedactedUri {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

fn is_sensitive_query_key(key: &str) -> bool {
    SENSITIVE_QUERY_KEYS
        .iter()
        .any(|candidate| key.eq_ignore_ascii_case(candidate))
}

async fn attach_redacted_uri(mut request: Request, next: Next) -> Response {
    let redacted_uri = RedactedUri::from_uri(request.uri());
    request.extensions_mut().insert(redacted_uri);
    next.run(request).await
}

fn make_request_span(request: &Request) -> Span {
    let redacted_uri = request
        .extensions()
        .get::<RedactedUri>()
        .cloned()
        .unwrap_or_else(|| RedactedUri::from_uri(request.uri()));
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .map(ToString::to_string)
        .unwrap_or_else(|| "[missing]".into());

    tracing::info_span!(
        "http_request",
        method = %request.method(),
        uri = %redacted_uri,
        request_id = %request_id,
    )
}

pub fn sensitive_headers_layer() -> SetSensitiveHeadersLayer {
    SetSensitiveHeadersLayer::new([
        header::AUTHORIZATION,
        header::COOKIE,
        header::SET_COOKIE,
        header::HeaderName::from_static("x-device-token"),
        header::HeaderName::from_static("x-media-token"),
        header::HeaderName::from_static("x-media-key"),
        header::HeaderName::from_static("x-media-lease"),
    ])
}

pub fn with_http_middleware(router: Router) -> Router {
    router
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(make_request_span)
                .on_request(DefaultOnRequest::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .layer(middleware::from_fn(attach_redacted_uri))
        .layer(sensitive_headers_layer())
        .layer(request_id::layer())
}

pub fn build_router(state: AppState) -> Router {
    let config = AppConfig::from_env().expect("application configuration was validated at startup");
    build_router_with_config(state, &config)
}

pub fn build_router_with_config(state: AppState, config: &AppConfig) -> Router {
    let web_assets = WebAssets::new(config.web_dist_dir.clone());
    let router = api::router()
        .with_state(state)
        .fallback(move |request: Request| {
            let web_assets = web_assets.clone();
            async move {
                if request.uri().path().starts_with("/api") {
                    return api_not_found(&request);
                }
                web_assets.serve(request).await
            }
        });
    with_http_middleware(router)
}

pub async fn test_router() -> Router {
    let config = AppConfig::from_iter(std::iter::empty::<(&str, &str)>())
        .expect("default application configuration is valid");
    build_router_with_config(AppState::default(), &config)
}

fn api_not_found(request: &Request) -> Response {
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .copied()
        .unwrap_or_else(RequestId::generate);
    ApiError::new(
        request_id,
        AppError::new(ErrorCode::NotFound, "Resource not found.", false),
    )
    .into_response()
}
