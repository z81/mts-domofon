use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Extension, FromRequest, Path, Query, Request, State},
    http::{HeaderMap, HeaderValue, Response, StatusCode, header},
    response::IntoResponse,
    routing::{delete, get, post},
};
use secrecy::SecretString;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    domain::FlatId,
    error::{ApiError, AppError, ErrorCode},
    request_id::RequestId,
    service::live::{LiveSessionManager, LiveSessionView},
    state::AppState,
};

const MAX_OPAQUE_ID_BYTES: usize = 512;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/dashboard", get(dashboard))
        .route("/api/v1/devices", get(devices_scoped))
        .route("/api/v1/cameras/stream", get(start_default_live))
        .route("/api/v1/cameras/{camera_id}/snapshot", get(snapshot_scoped))
        .route("/api/v1/cameras/{camera_id}/live", post(start_live))
        .route(
            "/api/v1/cameras/{camera_id}/archive/distribution",
            get(archive_distribution),
        )
        .route("/api/v1/cameras/{camera_id}/archive", post(start_archive))
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

const ARCHIVE_JSON_LIMIT: usize = 4 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchiveStartRequest {
    start: chrono::DateTime<chrono::Utc>,
}

struct ArchiveJson<T>(T);

impl<T, S> FromRequest<S> for ArchiveJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, _state: &S) -> Result<Self, Self::Rejection> {
        let request_id = request
            .extensions()
            .get::<RequestId>()
            .copied()
            .unwrap_or_else(RequestId::generate);
        if !request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
        {
            return Err(ApiError::new(
                request_id,
                AppError::new(
                    ErrorCode::UnsupportedMediaType,
                    "Content-Type must be application/json.",
                    false,
                ),
            ));
        }
        let bytes = to_bytes(request.into_body(), ARCHIVE_JSON_LIMIT)
            .await
            .map_err(|_| {
                ApiError::new(
                    request_id,
                    AppError::new(
                        ErrorCode::PayloadTooLarge,
                        "JSON request body is too large.",
                        false,
                    ),
                )
            })?;
        serde_json::from_slice(&bytes)
            .map(ArchiveJson)
            .map_err(|_| {
                ApiError::new(
                    request_id,
                    AppError::invalid_request("JSON request body is invalid."),
                )
            })
    }
}

async fn archive_distribution(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Path(camera_id): Path<String>,
    Query(scope): Query<FlatScope>,
) -> Result<axum::response::Response, ApiError> {
    validate_opaque_id(&scope.flat_id).map_err(|error| ApiError::new(request_id, error))?;
    validate_opaque_id(&camera_id).map_err(|error| ApiError::new(request_id, error))?;
    let distribution = state
        .archive_service()
        .ok_or_else(runtime_unavailable)
        .map_err(|error| ApiError::new(request_id, error))?
        .distribution(&FlatId(scope.flat_id), &camera_id)
        .await
        .map_err(|error| ApiError::new(request_id, error))?;
    let mut response = Json(distribution).into_response();
    no_store(response.headers_mut());
    Ok(response)
}

async fn start_archive(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Path(camera_id): Path<String>,
    Query(scope): Query<FlatScope>,
    ArchiveJson(request): ArchiveJson<ArchiveStartRequest>,
) -> Result<axum::response::Response, ApiError> {
    validate_opaque_id(&scope.flat_id).map_err(|error| ApiError::new(request_id, error))?;
    validate_opaque_id(&camera_id).map_err(|error| ApiError::new(request_id, error))?;
    let session = state
        .archive_service()
        .ok_or_else(runtime_unavailable)
        .map_err(|error| ApiError::new(request_id, error))?
        .start(&FlatId(scope.flat_id), &camera_id, request.start)
        .await
        .map_err(|error| ApiError::new(request_id, error))?;
    let mut response = Json(session).into_response();
    no_store(response.headers_mut());
    Ok(response)
}

fn no_store(headers: &mut HeaderMap) {
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
}

async fn start_default_live(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
) -> Result<Response<Body>, ApiError> {
    let flat_id = state
        .property_service()
        .ok_or_else(runtime_unavailable)
        .map_err(|error| ApiError::new(request_id, error))?
        .default_flat()
        .await
        .map_err(|error| ApiError::new(request_id, error))?
        .ok_or_else(|| ApiError::new(request_id, linked_flat_not_found()))?;
    let camera = state
        .device_service()
        .ok_or_else(runtime_unavailable)
        .map_err(|error| ApiError::new(request_id, error))?
        .first_live_camera(&flat_id)
        .await
        .map_err(|error| ApiError::new(request_id, error))?
        .ok_or_else(|| ApiError::new(request_id, live_video_not_found()))?;
    let sessions = live_session_manager(&state, request_id)?;
    let camera_key = live_camera_key(&flat_id, &camera.id);
    let view = match sessions.existing(&camera_key).await {
        Some(view) => view,
        None => {
            let source = live_source(&state, request_id, &flat_id, &camera.id).await?;
            start_live_session(sessions, request_id, camera_key, source).await?
        }
    };
    temporary_live_redirect(request_id, view)
}

fn live_session_manager(
    state: &AppState,
    request_id: RequestId,
) -> Result<&LiveSessionManager, ApiError> {
    state
        .live_session_manager()
        .map(AsRef::as_ref)
        .ok_or_else(runtime_unavailable)
        .map_err(|error| ApiError::new(request_id, error))
}

/// Live sessions are shared per flat and camera; the length prefix keeps the key unambiguous.
fn live_camera_key(flat_id: &FlatId, camera_id: &str) -> String {
    format!("{:04x}{}{}", flat_id.0.len(), flat_id.0, camera_id)
}

async fn live_source(
    state: &AppState,
    request_id: RequestId,
    flat_id: &FlatId,
    camera_id: &str,
) -> Result<SecretString, ApiError> {
    state
        .live_source_provider()
        .ok_or_else(runtime_unavailable)
        .map_err(|error| ApiError::new(request_id, error))?
        .live_source(flat_id, camera_id)
        .await
        .map_err(|error| ApiError::new(request_id, error))
}

async fn start_live_session(
    sessions: &LiveSessionManager,
    request_id: RequestId,
    camera_key: String,
    source: SecretString,
) -> Result<LiveSessionView, ApiError> {
    sessions
        .start(camera_key, source)
        .await
        .map_err(|error| ApiError::new(request_id, error))
}

fn temporary_live_redirect(
    request_id: RequestId,
    view: LiveSessionView,
) -> Result<Response<Body>, ApiError> {
    let location = format!("/api/v1/live/{}/index.m3u8", view.session_id);
    if view.playlist_url != location {
        return Err(ApiError::new(request_id, runtime_unavailable()));
    }
    Response::builder()
        .status(StatusCode::TEMPORARY_REDIRECT)
        .header(header::LOCATION, location)
        .header(header::CACHE_CONTROL, "private, no-store")
        .body(Body::empty())
        .map_err(|_| ApiError::new(request_id, runtime_unavailable()))
}

async fn start_live(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Path(camera_id): Path<String>,
    Query(scope): Query<FlatScope>,
) -> Result<Json<LiveSessionView>, ApiError> {
    validate_opaque_id(&scope.flat_id).map_err(|error| ApiError::new(request_id, error))?;
    validate_opaque_id(&camera_id).map_err(|error| ApiError::new(request_id, error))?;
    let flat_id = FlatId(scope.flat_id);
    let sessions = live_session_manager(&state, request_id)?;
    let camera_key = live_camera_key(&flat_id, &camera_id);
    // A running session was created only after this flat and camera passed every check below.
    if let Some(view) = sessions.existing(&camera_key).await {
        return Ok(Json(view));
    }
    // The source lookup reads the flat's camera list and fails for cameras outside the flat or
    // without live video, so it runs alongside the linked-flat check rather than after it.
    let (linked, source) = tokio::join!(
        ensure_linked_flat(&state, request_id, &flat_id),
        live_source(&state, request_id, &flat_id, &camera_id),
    );
    linked?;
    let view = start_live_session(sessions, request_id, camera_key, source?).await?;
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

fn linked_flat_not_found() -> AppError {
    AppError::new(ErrorCode::NotFound, "Linked flat was not found.", false)
}

fn live_video_not_found() -> AppError {
    AppError::new(ErrorCode::NotFound, "Live video is unavailable.", false)
}
