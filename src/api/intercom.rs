use axum::{
    Json, Router,
    body::Body,
    extract::{Extension, Path, Query, State},
    http::{HeaderValue, Response, header},
    routing::get,
};
use serde::{Deserialize, Serialize};

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
        .route("/api/v1/devices", get(devices_scoped))
        .route("/api/v1/cameras/{camera_id}/snapshot", get(snapshot_scoped))
        .route("/api/v1/flats/{flat_id}/devices", get(devices_nested))
        .route(
            "/api/v1/flats/{flat_id}/cameras/{camera_id}/snapshot",
            get(snapshot_nested),
        )
        .route("/api/places", get(compat_places))
        .route(
            "/api/places/{flat_id}/accesscontrols",
            get(compat_access_points),
        )
        .route("/api/places/{flat_id}/cameras", get(compat_cameras))
        .route("/api/snapshot/{flat_id}/{camera_id}", get(snapshot_nested))
        .route("/api/snapshot/{camera_id}", get(snapshot_scoped))
}

#[derive(Deserialize)]
struct FlatScope {
    #[serde(rename = "flatId")]
    flat_id: String,
}

#[derive(Serialize)]
struct CompatibilityData<T> {
    data: T,
}

async fn dashboard(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
) -> Result<Json<crate::domain::PropertyDashboard>, ApiError> {
    load_dashboard(&state, request_id).await.map(Json)
}

async fn load_dashboard(
    state: &AppState,
    request_id: RequestId,
) -> Result<crate::domain::PropertyDashboard, ApiError> {
    let service = state
        .property_service()
        .ok_or_else(runtime_unavailable)
        .map_err(|error| ApiError::new(request_id, error))?;
    service
        .dashboard()
        .await
        .map_err(|error| ApiError::new(request_id, error))
}

async fn devices_scoped(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Query(scope): Query<FlatScope>,
) -> Result<Json<crate::domain::DeviceInventory>, ApiError> {
    load_inventory(&state, request_id, scope.flat_id)
        .await
        .map(Json)
}

async fn devices_nested(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Path(flat_id): Path<String>,
) -> Result<Json<crate::domain::DeviceInventory>, ApiError> {
    load_inventory(&state, request_id, flat_id).await.map(Json)
}

async fn load_inventory(
    state: &AppState,
    request_id: RequestId,
    flat_id: String,
) -> Result<crate::domain::DeviceInventory, ApiError> {
    validate_opaque_id(&flat_id).map_err(|error| ApiError::new(request_id, error))?;
    let flat_id = FlatId(flat_id);
    ensure_linked_flat(state, request_id, &flat_id).await?;
    let service = state
        .device_service()
        .ok_or_else(runtime_unavailable)
        .map_err(|error| ApiError::new(request_id, error))?;
    service
        .inventory(&flat_id)
        .await
        .map_err(|error| ApiError::new(request_id, error))
}

async fn snapshot_scoped(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Path(camera_id): Path<String>,
    Query(scope): Query<FlatScope>,
) -> Result<Response<Body>, ApiError> {
    load_snapshot(&state, request_id, scope.flat_id, camera_id).await
}

async fn snapshot_nested(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Path((flat_id, camera_id)): Path<(String, String)>,
) -> Result<Response<Body>, ApiError> {
    load_snapshot(&state, request_id, flat_id, camera_id).await
}

async fn load_snapshot(
    state: &AppState,
    request_id: RequestId,
    flat_id: String,
    camera_id: String,
) -> Result<Response<Body>, ApiError> {
    validate_opaque_id(&flat_id).map_err(|error| ApiError::new(request_id, error))?;
    validate_opaque_id(&camera_id).map_err(|error| ApiError::new(request_id, error))?;
    let flat_id = FlatId(flat_id);
    ensure_linked_flat(state, request_id, &flat_id).await?;
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

async fn compat_places(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
) -> Result<Json<CompatibilityData<Vec<crate::domain::Flat>>>, ApiError> {
    let dashboard = load_dashboard(&state, request_id).await?;
    Ok(Json(CompatibilityData {
        data: dashboard.flats,
    }))
}

async fn compat_access_points(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Path(flat_id): Path<String>,
) -> Result<Json<CompatibilityData<Vec<crate::domain::AccessPoint>>>, ApiError> {
    let inventory = load_inventory(&state, request_id, flat_id).await?;
    Ok(Json(CompatibilityData {
        data: inventory.access_points,
    }))
}

async fn compat_cameras(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Path(flat_id): Path<String>,
) -> Result<Json<CompatibilityData<Vec<crate::domain::Camera>>>, ApiError> {
    let inventory = load_inventory(&state, request_id, flat_id).await?;
    Ok(Json(CompatibilityData {
        data: inventory.cameras,
    }))
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
