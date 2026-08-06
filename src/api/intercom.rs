use axum::{
    Json, Router,
    body::Body,
    extract::{Extension, Path, State},
    http::{HeaderValue, Response, header},
    routing::get,
};

use crate::{
    domain::FlatId,
    error::{ApiError, AppError, ErrorCode},
    request_id::RequestId,
    state::AppState,
};

const MAX_OPAQUE_ID_BYTES: usize = 512;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/dashboard", get(dashboard))
        .route("/api/v1/flats/{flat_id}/devices", get(devices))
        .route(
            "/api/v1/flats/{flat_id}/cameras/{camera_id}/snapshot",
            get(snapshot),
        )
}

async fn dashboard(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
) -> Result<Json<crate::domain::PropertyDashboard>, ApiError> {
    let service = state
        .property_service()
        .ok_or_else(runtime_unavailable)
        .map_err(|error| ApiError::new(request_id, error))?;
    service
        .dashboard()
        .await
        .map(Json)
        .map_err(|error| ApiError::new(request_id, error))
}

async fn devices(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Path(flat_id): Path<String>,
) -> Result<Json<crate::domain::DeviceInventory>, ApiError> {
    validate_opaque_id(&flat_id).map_err(|error| ApiError::new(request_id, error))?;
    let flat_id = FlatId(flat_id);
    ensure_linked_flat(&state, request_id, &flat_id).await?;
    let service = state
        .device_service()
        .ok_or_else(runtime_unavailable)
        .map_err(|error| ApiError::new(request_id, error))?;
    service
        .inventory(&flat_id)
        .await
        .map(Json)
        .map_err(|error| ApiError::new(request_id, error))
}

async fn snapshot(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Path((flat_id, camera_id)): Path<(String, String)>,
) -> Result<Response<Body>, ApiError> {
    validate_opaque_id(&flat_id).map_err(|error| ApiError::new(request_id, error))?;
    validate_opaque_id(&camera_id).map_err(|error| ApiError::new(request_id, error))?;
    let flat_id = FlatId(flat_id);
    ensure_linked_flat(&state, request_id, &flat_id).await?;
    let service = state
        .media_service()
        .ok_or_else(runtime_unavailable)
        .map_err(|error| ApiError::new(request_id, error))?;
    let media = service
        .snapshot(&flat_id, &camera_id)
        .await
        .map_err(|error| ApiError::new(request_id, error))?;
    let content_length = HeaderValue::from_str(&media.bytes.len().to_string())
        .expect("usize is a valid Content-Length header value");
    let mut response = Response::new(Body::from(media.bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(media.content_type.as_str()),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert(header::CONTENT_LENGTH, content_length);
    Ok(response)
}

async fn ensure_linked_flat(
    state: &AppState,
    request_id: RequestId,
    flat_id: &FlatId,
) -> Result<(), ApiError> {
    let properties = state
        .property_service()
        .ok_or_else(runtime_unavailable)
        .map_err(|error| ApiError::new(request_id, error))?;
    properties
        .ensure_linked_flat(flat_id)
        .await
        .map_err(|error| ApiError::new(request_id, error))
}

fn validate_opaque_id(value: &str) -> Result<(), AppError> {
    if value.is_empty() || value.len() > MAX_OPAQUE_ID_BYTES || value.chars().any(char::is_control)
    {
        return Err(AppError::invalid_request("Invalid resource identifier."));
    }
    Ok(())
}

fn runtime_unavailable() -> AppError {
    AppError::new(
        ErrorCode::Internal,
        "Runtime service is unavailable.",
        false,
    )
}
