use axum::{Router, http::header};
use tower_http::sensitive_headers::SetSensitiveHeadersLayer;

use crate::{api, request_id, state::AppState};

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

pub fn build_router(state: AppState) -> Router {
    api::router()
        .with_state(state)
        .layer(sensitive_headers_layer())
        .layer(request_id::layer())
}

pub async fn test_router() -> Router {
    build_router(AppState)
}
