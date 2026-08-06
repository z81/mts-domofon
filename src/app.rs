use axum::Router;

use crate::{api, state::AppState};

pub fn build_router(state: AppState) -> Router {
    api::router().with_state(state)
}

pub async fn test_router() -> Router {
    build_router(AppState)
}
