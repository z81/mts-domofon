pub mod auth;
pub mod calls;
pub mod door;
pub mod events;
pub mod health;
pub mod intercom;

use axum::{Router, routing::get};

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(auth::router())
        .merge(calls::router())
        .merge(door::router())
        .merge(events::router())
        .merge(intercom::router())
        .route("/health/live", get(health::live))
        .method_not_allowed_fallback(auth::method_not_allowed)
}
