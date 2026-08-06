use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Extension, FromRequest, Path, Request, State, rejection::PathRejection},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::{Deserialize, de::DeserializeOwned};
use zeroize::Zeroizing;

use crate::{
    domain::{DoorOpenIntentId, DoorOpenIntentView, FlatId},
    error::{ApiError, AppError, ErrorCode},
    request_id::RequestId,
    service::door::DoorOpenService,
    state::AppState,
};

const JSON_BODY_LIMIT: usize = 4 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/access/intents", post(create))
        .route("/api/v1/access/intents/{intent_id}/execute", post(execute))
        .route("/api/v1/door-open-intents", post(create))
        .route("/api/v1/door-open-intents/confirm", post(confirm))
        .route("/api/open-door", post(compatibility_open_door))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateIntentRequest {
    flat_id: String,
    access_point_id: String,
    lock_number: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfirmIntentRequest {
    intent_id: DoorOpenIntentId,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CompatibilityOpenDoorRequest {
    Create(CreateIntentRequest),
    Confirm(ConfirmIntentRequest),
}

struct DoorJson<T>(T);

impl<T, S> FromRequest<S> for DoorJson<T>
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
        if !is_application_json(request.headers()) {
            return Err(ApiError::new(
                request_id,
                AppError::new(
                    ErrorCode::UnsupportedMediaType,
                    "Content-Type must be application/json.",
                    false,
                ),
            ));
        }
        let bytes = to_bytes(request.into_body(), JSON_BODY_LIMIT)
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
        let bytes = Zeroizing::new(bytes.to_vec());
        let value = serde_json::from_slice(&bytes).map_err(|_| {
            ApiError::new(
                request_id,
                AppError::invalid_request("JSON request body is invalid."),
            )
        })?;
        Ok(Self(value))
    }
}

async fn create(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    DoorJson(request): DoorJson<CreateIntentRequest>,
) -> Result<(StatusCode, Json<DoorOpenIntentView>), ApiError> {
    create_intent(&state, request_id, request)
        .await
        .map(|intent| (StatusCode::CREATED, Json(intent)))
}

async fn confirm(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    DoorJson(request): DoorJson<ConfirmIntentRequest>,
) -> Result<StatusCode, ApiError> {
    confirm_intent(&state, request_id, &request.intent_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn execute(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    path: Result<Path<String>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    let Path(raw_intent_id) = path.map_err(|_| invalid_intent_id(request_id))?;
    let intent_id = raw_intent_id
        .parse()
        .map_err(|_| invalid_intent_id(request_id))?;
    confirm_intent(&state, request_id, &intent_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn compatibility_open_door(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    DoorJson(request): DoorJson<CompatibilityOpenDoorRequest>,
) -> Result<Response, ApiError> {
    match request {
        CompatibilityOpenDoorRequest::Create(request) => create_intent(&state, request_id, request)
            .await
            .map(|intent| (StatusCode::CREATED, Json(intent)).into_response()),
        CompatibilityOpenDoorRequest::Confirm(request) => {
            confirm_intent(&state, request_id, &request.intent_id).await?;
            Ok(StatusCode::NO_CONTENT.into_response())
        }
    }
}

async fn create_intent(
    state: &AppState,
    request_id: RequestId,
    request: CreateIntentRequest,
) -> Result<DoorOpenIntentView, ApiError> {
    service(state, request_id)?
        .create_intent(
            &FlatId(request.flat_id),
            &request.access_point_id,
            &request.lock_number,
        )
        .await
        .map_err(|error| ApiError::new(request_id, error))
}

async fn confirm_intent(
    state: &AppState,
    request_id: RequestId,
    intent_id: &DoorOpenIntentId,
) -> Result<(), ApiError> {
    service(state, request_id)?
        .confirm(intent_id)
        .await
        .map_err(|error| ApiError::new(request_id, error))
}

fn service(state: &AppState, request_id: RequestId) -> Result<&DoorOpenService, ApiError> {
    state.door_open_service().map(AsRef::as_ref).ok_or_else(|| {
        ApiError::new(
            request_id,
            AppError::new(
                ErrorCode::Internal,
                "Door-open service is unavailable.",
                false,
            ),
        )
    })
}

fn is_application_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("application/json"))
}

fn invalid_intent_id(request_id: RequestId) -> ApiError {
    ApiError::new(
        request_id,
        AppError::invalid_request("Door-open confirmation intent ID is invalid."),
    )
}
