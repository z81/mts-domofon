use axum::{
    Json, Router,
    extract::{Extension, Query, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;

use crate::{
    domain::{
        FlatId,
        event::{EventKind, EventQuery},
    },
    error::{ApiError, AppError, ErrorCode},
    request_id::RequestId,
    state::AppState,
};

const MAX_OPAQUE_ID_BYTES: usize = 512;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/events", get(events))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventParameters {
    flat_id: String,
    page: Option<u32>,
    size: Option<u32>,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    kind: Option<EventKind>,
}

async fn events(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Query(parameters): Query<EventParameters>,
) -> Result<Response, ApiError> {
    validate_id(&parameters.flat_id).map_err(|error| ApiError::new(request_id, error))?;
    let to = parameters.to.unwrap_or_else(Utc::now);
    let from = parameters
        .from
        .unwrap_or_else(|| to.checked_sub_signed(TimeDelta::days(7)).unwrap_or(to));
    let query = EventQuery {
        flat_id: FlatId(parameters.flat_id),
        page: parameters.page.unwrap_or(0),
        size: parameters.size.unwrap_or(20),
        from,
        to,
        kind: parameters.kind,
    };
    let page = state
        .event_service()
        .ok_or_else(runtime_unavailable)
        .map_err(|error| ApiError::new(request_id, error))?
        .events(query)
        .await
        .map_err(|error| ApiError::new(request_id, error))?;
    let mut response = Json(page).into_response();
    no_store(response.headers_mut());
    Ok(response)
}

fn validate_id(value: &str) -> Result<(), AppError> {
    if value.is_empty() || value.len() > MAX_OPAQUE_ID_BYTES || value.chars().any(char::is_control)
    {
        return Err(AppError::invalid_request("Invalid resource identifier."));
    }
    Ok(())
}

fn runtime_unavailable() -> AppError {
    AppError::new(ErrorCode::Internal, "Event service is unavailable.", false)
}

fn no_store(headers: &mut HeaderMap) {
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
}
