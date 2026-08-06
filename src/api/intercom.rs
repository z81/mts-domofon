use axum::{
    Json, Router,
    body::Body,
    extract::{Extension, Path, Query, State},
    http::{HeaderValue, Response, StatusCode, header},
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};

use crate::{
    domain::{Capability, FlatId},
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
        .route("/api/v1/cameras/{camera_id}/live", post(start_live))
        .route("/api/v1/live/{session_id}/{asset}", get(read_live_asset))
        .route("/api/v1/live/{session_id}", delete(stop_live))
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

async fn start_live(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Path(camera_id): Path<String>,
    Query(scope): Query<FlatScope>,
) -> Result<Json<crate::service::live::LiveSessionView>, ApiError> {
    validate_opaque_id(&scope.flat_id).map_err(|error| ApiError::new(request_id, error))?;
    validate_opaque_id(&camera_id).map_err(|error| ApiError::new(request_id, error))?;
    let flat_id = FlatId(scope.flat_id);
    ensure_linked_flat(&state, request_id, &flat_id).await?;
    let devices = state
        .device_service()
        .ok_or_else(runtime_unavailable)
        .map_err(|error| ApiError::new(request_id, error))?;
    let camera = devices
        .camera(&flat_id, &camera_id)
        .await
        .map_err(|error| ApiError::new(request_id, error))?;
    if !matches!(camera.capabilities.live, Capability::Available) {
        return Err(ApiError::new(
            request_id,
            AppError::new(ErrorCode::NotFound, "Live video is unavailable.", false),
        ));
    }
    let camera_key = format!("{:04x}{}{}", flat_id.0.len(), flat_id.0, camera_id);
    let source = state
        .live_source_provider()
        .ok_or_else(runtime_unavailable)
        .map_err(|error| ApiError::new(request_id, error))?
        .live_source(&flat_id, &camera_id)
        .await
        .map_err(|error| ApiError::new(request_id, error))?;
    let view = state
        .live_session_manager()
        .ok_or_else(runtime_unavailable)
        .map_err(|error| ApiError::new(request_id, error))?
        .start(camera_key, source)
        .await
        .map_err(|error| ApiError::new(request_id, error))?;
    Ok(Json(view))
}

async fn read_live_asset(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Path((session_id, asset)): Path<(String, String)>,
) -> Result<Response<Body>, ApiError> {
    let session_id = uuid::Uuid::parse_str(&session_id).map_err(|_| {
        ApiError::new(
            request_id,
            AppError::new(ErrorCode::NotFound, "Live session was not found.", false),
        )
    })?;
    let asset = state
        .live_session_manager()
        .ok_or_else(runtime_unavailable)
        .map_err(|error| ApiError::new(request_id, error))?
        .read(session_id, &asset)
        .await
        .map_err(|error| ApiError::new(request_id, error))?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, asset.content_type)
        .header(header::CACHE_CONTROL, "private, no-store")
        .header("X-Content-Type-Options", "nosniff")
        .body(Body::from(asset.bytes))
        .map_err(|_| ApiError::new(request_id, runtime_unavailable()))
}

async fn stop_live(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let session_id = uuid::Uuid::parse_str(&session_id).map_err(|_| {
        ApiError::new(
            request_id,
            AppError::new(ErrorCode::NotFound, "Live session was not found.", false),
        )
    })?;
    state
        .live_session_manager()
        .ok_or_else(runtime_unavailable)
        .map_err(|error| ApiError::new(request_id, error))?
        .stop(session_id)
        .await
        .map_err(|error| ApiError::new(request_id, error))?;
    Ok(StatusCode::NO_CONTENT)
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
