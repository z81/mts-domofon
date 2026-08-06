pub mod auth;
pub mod health;

use axum::{Router, routing::get};

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(auth::router())
        .route("/health/live", get(health::live))
        .method_not_allowed_fallback(auth::method_not_allowed)
}
