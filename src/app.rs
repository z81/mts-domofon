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
    web::{ValidatedPath, WebAssets},
};

const SENSITIVE_QUERY_KEYS: &[&str] = &[
    "token",
    "code",
    "state",
    "callback",
    "callback_url",
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
        let mut output = redacted_path(uri.path());
        if let Some(query) = uri.query() {
            if is_query_sensitive_route_path(uri.path()) {
                output.push_str("?[REDACTED]");
                return Self(output);
            }
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

fn is_query_sensitive_route_path(path: &str) -> bool {
    is_removed_mts_relay_path(path)
        || is_camera_live_path(path)
        || is_camera_archive_path(path)
        || path == "/api/v1/door-open-intents"
        || path == "/api/v1/door-open-intents/confirm"
        || path == "/api/v1/access/intents"
        || path.starts_with("/api/v1/access/intents/")
        || path == "/api/open-door"
        || path == "/api/v1/auth"
        || path.starts_with("/api/v1/auth/")
        || matches!(
            path,
            "/api/v1/session"
                | "/api/v1/session/refresh"
                | "/api/login"
                | "/api/confirm"
                | "/api/session"
                | "/api/refresh"
                | "/api/logout"
        )
}

fn is_camera_live_path(path: &str) -> bool {
    path.strip_prefix("/api/v1/cameras/")
        .and_then(|descendant| descendant.strip_suffix("/live"))
        .is_some_and(|camera_id| !camera_id.is_empty() && !camera_id.contains('/'))
}

fn is_camera_archive_path(path: &str) -> bool {
    let Some(descendant) = path.strip_prefix("/api/v1/cameras/") else {
        return false;
    };
    descendant
        .strip_suffix("/archive")
        .or_else(|| descendant.strip_suffix("/archive/distribution"))
        .is_some_and(|camera_id| !camera_id.is_empty() && !camera_id.contains('/'))
}

fn is_removed_mts_relay_path(path: &str) -> bool {
    ValidatedPath::parse(path).is_some_and(|path| path.is_removed_mts_relay_path())
}

fn redacted_path(path: &str) -> String {
    const PREFIX: &str = "/api/v1/access/intents/";
    if let Some(descendant) = path.strip_prefix(PREFIX) {
        if let Some((intent_id, suffix)) = descendant.split_once('/') {
            if !intent_id.is_empty() {
                return format!("{PREFIX}[REDACTED]/{suffix}");
            }
        } else if !descendant.is_empty() {
            return format!("{PREFIX}[REDACTED]");
        }
    }
    const LIVE_PREFIX: &str = "/api/v1/live/";
    if let Some(descendant) = path.strip_prefix(LIVE_PREFIX) {
        if let Some((_session_id, suffix)) = descendant.split_once('/') {
            return format!("{LIVE_PREFIX}[REDACTED]/{suffix}");
        }
        if !descendant.is_empty() {
            return format!("{LIVE_PREFIX}[REDACTED]");
        }
    }
    if is_camera_live_path(path) {
        return "/api/v1/cameras/[REDACTED]/live".to_owned();
    }
    if is_camera_archive_path(path) {
        let suffix = if path.ends_with("/archive/distribution") {
            "/archive/distribution"
        } else {
            "/archive"
        };
        return format!("/api/v1/cameras/[REDACTED]{suffix}");
    }
    path.to_owned()
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
                let Some(path) = ValidatedPath::parse(request.uri().path()) else {
                    return axum::http::StatusCode::NOT_FOUND.into_response();
                };
                if path.is_api() {
                    return api_not_found(&request);
                }
                if path.is_removed_mts_relay_path() {
                    return axum::http::StatusCode::NOT_FOUND.into_response();
                }
                web_assets.serve(request, &path).await
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
