use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Extension, FromRequest, Path, Request, State, rejection::PathRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, de::DeserializeOwned};
use zeroize::Zeroizing;

use crate::{
    domain::{
        auto_open::{AutoOpenIntentId, AutoOpenMode},
        call::{CallId, CallRevision},
    },
    error::{ApiError, AppError, ErrorCode},
    request_id::RequestId,
    service::{
        auto_open::AutoOpenService,
        calls::{CallOpenIntentId, CallService, CallView, MAX_SDP_BYTES},
    },
    state::AppState,
};

const JSON_BODY_LIMIT: usize = MAX_SDP_BYTES + 4 * 1024;
const AUTO_OPEN_JSON_BODY_LIMIT: usize = 4 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/calls/current", get(current))
        .route(
            "/api/v1/calls/auto-open",
            get(auto_open_policy).delete(disable_auto_open),
        )
        .route(
            "/api/v1/calls/auto-open/intents",
            post(create_auto_open_intent),
        )
        .route(
            "/api/v1/calls/auto-open/intents/{intent_id}/execute",
            post(execute_auto_open_intent),
        )
        .route("/api/v1/calls/{call_id}/answer", post(answer))
        .route("/api/v1/calls/{call_id}/decline", post(decline))
        .route("/api/v1/calls/{call_id}/ignore", post(ignore))
        .route("/api/v1/calls/{call_id}/hangup", post(hangup))
        .route(
            "/api/v1/calls/{call_id}/open-intents",
            post(create_open_intent),
        )
        .route(
            "/api/v1/calls/{call_id}/open-intents/{intent_id}/execute",
            post(execute_open_intent),
        )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RevisionRequest {
    revision: CallRevision,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AnswerRequest {
    revision: CallRevision,
    sdp: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AutoOpenIntentRequest {
    mode: AutoOpenMode,
    dismiss_after_open: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecuteAutoOpenIntentRequest {}

struct CallJson<T>(T);
struct AutoOpenJson<T>(T);

impl<T, S> FromRequest<S> for CallJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, _state: &S) -> Result<Self, Self::Rejection> {
        parse_json(request, JSON_BODY_LIMIT).await.map(Self)
    }
}

impl<T, S> FromRequest<S> for AutoOpenJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, _state: &S) -> Result<Self, Self::Rejection> {
        parse_json(request, AUTO_OPEN_JSON_BODY_LIMIT)
            .await
            .map(Self)
    }
}

async fn parse_json<T>(request: Request, limit: usize) -> Result<T, ApiError>
where
    T: DeserializeOwned,
{
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
    let bytes = to_bytes(request.into_body(), limit).await.map_err(|_| {
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
    serde_json::from_slice(&bytes).map_err(|_| {
        ApiError::new(
            request_id,
            AppError::invalid_request("JSON request body is invalid."),
        )
    })
}

async fn auto_open_policy(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
) -> Result<Response, ApiError> {
    let policy = auto_open_service(&state, request_id)?
        .policy()
        .await
        .map_err(|error| ApiError::new(request_id, error))?;
    let mut response = Json(policy).into_response();
    no_store(response.headers_mut());
    Ok(response)
}

async fn create_auto_open_intent(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    AutoOpenJson(request): AutoOpenJson<AutoOpenIntentRequest>,
) -> Result<Response, ApiError> {
    let intent = auto_open_service(&state, request_id)?
        .create_intent(request.mode, request.dismiss_after_open)
        .await
        .map_err(|error| ApiError::new(request_id, error))?;
    let mut response = (StatusCode::CREATED, Json(intent)).into_response();
    no_store(response.headers_mut());
    Ok(response)
}

async fn execute_auto_open_intent(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    path: Result<Path<String>, PathRejection>,
    AutoOpenJson(_request): AutoOpenJson<ExecuteAutoOpenIntentRequest>,
) -> Result<Response, ApiError> {
    let Path(raw_intent_id) = path.map_err(|_| invalid_auto_open_identifier(request_id))?;
    let intent_id = raw_intent_id
        .parse::<AutoOpenIntentId>()
        .map_err(|_| invalid_auto_open_identifier(request_id))?;
    let policy = auto_open_service(&state, request_id)?
        .execute_intent(&intent_id)
        .await
        .map_err(|error| ApiError::new(request_id, error))?;
    let mut response = Json(policy).into_response();
    no_store(response.headers_mut());
    Ok(response)
}

async fn disable_auto_open(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
) -> Result<StatusCode, ApiError> {
    auto_open_service(&state, request_id)?
        .disable()
        .await
        .map_err(|error| ApiError::new(request_id, error))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn current(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
) -> Result<Response, ApiError> {
    let current = service(&state, request_id)?
        .current()
        .await
        .map_err(|error| ApiError::new(request_id, error))?;
    let mut response = match current {
        Some(call) => Json(call).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    };
    no_store(response.headers_mut());
    Ok(response)
}

async fn answer(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    path: Result<Path<String>, PathRejection>,
    CallJson(request): CallJson<AnswerRequest>,
) -> Result<Response, ApiError> {
    let call_id = parse_call_id(path, request_id)?;
    let view = service(&state, request_id)?
        .answer(&call_id, request.revision, &request.sdp)
        .await
        .map_err(|error| ApiError::new(request_id, error))?;
    call_view_response(view)
}

async fn decline(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    path: Result<Path<String>, PathRejection>,
    CallJson(request): CallJson<RevisionRequest>,
) -> Result<StatusCode, ApiError> {
    let call_id = parse_call_id(path, request_id)?;
    service(&state, request_id)?
        .decline(&call_id, request.revision)
        .await
        .map_err(|error| ApiError::new(request_id, error))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn ignore(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    path: Result<Path<String>, PathRejection>,
    CallJson(request): CallJson<RevisionRequest>,
) -> Result<StatusCode, ApiError> {
    let call_id = parse_call_id(path, request_id)?;
    service(&state, request_id)?
        .ignore(&call_id, request.revision)
        .await
        .map_err(|error| ApiError::new(request_id, error))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn hangup(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    path: Result<Path<String>, PathRejection>,
    CallJson(request): CallJson<RevisionRequest>,
) -> Result<StatusCode, ApiError> {
    let call_id = parse_call_id(path, request_id)?;
    service(&state, request_id)?
        .hangup(&call_id, request.revision)
        .await
        .map_err(|error| ApiError::new(request_id, error))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn create_open_intent(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    path: Result<Path<String>, PathRejection>,
    CallJson(request): CallJson<RevisionRequest>,
) -> Result<Response, ApiError> {
    let call_id = parse_call_id(path, request_id)?;
    let intent = service(&state, request_id)?
        .create_open_intent(&call_id, request.revision)
        .await
        .map_err(|error| ApiError::new(request_id, error))?;
    let mut response = (StatusCode::CREATED, Json(intent)).into_response();
    no_store(response.headers_mut());
    Ok(response)
}

async fn execute_open_intent(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    path: Result<Path<(String, String)>, PathRejection>,
    CallJson(request): CallJson<RevisionRequest>,
) -> Result<StatusCode, ApiError> {
    let Path((raw_call_id, raw_intent_id)) = path.map_err(|_| invalid_identifier(request_id))?;
    let call_id = CallId::parse(&raw_call_id).map_err(|_| invalid_identifier(request_id))?;
    let intent_id = raw_intent_id
        .parse::<CallOpenIntentId>()
        .map_err(|_| invalid_identifier(request_id))?;
    service(&state, request_id)?
        .execute_open_intent(&call_id, &intent_id, request.revision)
        .await
        .map_err(|error| ApiError::new(request_id, error))?;
    Ok(StatusCode::NO_CONTENT)
}

fn parse_call_id(
    path: Result<Path<String>, PathRejection>,
    request_id: RequestId,
) -> Result<CallId, ApiError> {
    let Path(raw_call_id) = path.map_err(|_| invalid_identifier(request_id))?;
    CallId::parse(&raw_call_id).map_err(|_| invalid_identifier(request_id))
}

fn call_view_response(view: CallView) -> Result<Response, ApiError> {
    let mut response = Json(view).into_response();
    no_store(response.headers_mut());
    Ok(response)
}

fn service(state: &AppState, request_id: RequestId) -> Result<&CallService, ApiError> {
    state.call_service().map(AsRef::as_ref).ok_or_else(|| {
        ApiError::new(
            request_id,
            AppError::new(ErrorCode::Internal, "Call service is unavailable.", false),
        )
    })
}

fn auto_open_service(
    state: &AppState,
    request_id: RequestId,
) -> Result<&AutoOpenService, ApiError> {
    state.auto_open_service().map(AsRef::as_ref).ok_or_else(|| {
        ApiError::new(
            request_id,
            AppError::new(
                ErrorCode::Internal,
                "Auto-open service is unavailable.",
                false,
            ),
        )
    })
}

fn no_store(headers: &mut HeaderMap) {
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
}

fn is_application_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("application/json"))
}

fn invalid_identifier(request_id: RequestId) -> ApiError {
    ApiError::new(
        request_id,
        AppError::invalid_request("Call resource identifier is invalid."),
    )
}

fn invalid_auto_open_identifier(request_id: RequestId) -> ApiError {
    ApiError::new(
        request_id,
        AppError::invalid_request("Auto-open resource identifier is invalid."),
    )
}
