use std::sync::Arc;

use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Extension, FromRequest, Path, Request, State, rejection::PathRejection},
    http::{HeaderMap, header},
    routing::{get, post},
};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, de::DeserializeOwned, de::IgnoredAny};
use zeroize::Zeroizing;

use crate::{
    auth::{AuthCoordinator, LogoutResult},
    domain::auth::{AuthFlowId, AuthProgress, ChallengeInput, SessionView},
    error::{ApiError, AppError, ErrorCode},
    request_id::RequestId,
    state::AppState,
};

const JSON_BODY_LIMIT: usize = 16 * 1024;
const CALLBACK_STRING_LIMIT: usize = 8 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/auth/start", post(start))
        .route(
            "/api/v1/auth/flows/{flow_id}/callback",
            post(complete_callback),
        )
        .route(
            "/api/v1/auth/flows/{flow_id}/challenge",
            post(submit_challenge),
        )
        .route("/api/v1/auth/legacy-sms/start", post(start_legacy_sms))
        .route(
            "/api/v1/auth/legacy-sms/flows/{flow_id}/confirm",
            post(confirm_legacy_sms),
        )
        .route("/api/v1/session", get(session).delete(logout))
        .route("/api/v1/session/refresh", post(refresh))
        .route("/api/login", post(compatibility_login))
        .route("/api/confirm", post(compatibility_confirm))
        .route("/api/session", get(session))
        .route("/api/refresh", post(refresh))
        .route("/api/logout", post(logout))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartRequest {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CallbackRequest {
    callback_url: SecretString,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacySmsStartRequest {
    phone: SecretString,
    accept_legacy_sms: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacySmsConfirmRequest {
    code: SecretString,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyLoginRequest {
    #[serde(default)]
    phone: Option<SecretString>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyConfirmRequest {
    phone: SecretString,
    code: SecretString,
    #[serde(default)]
    contract: Option<IgnoredAny>,
}

struct AuthJson<T>(T);

impl<T, S> FromRequest<S> for AuthJson<T>
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

fn is_application_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("application/json"))
}

async fn start(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    AuthJson(_request): AuthJson<StartRequest>,
) -> Result<Json<AuthProgress>, ApiError> {
    let coordinator = coordinator(&state, request_id)?;
    coordinator
        .start()
        .await
        .map(Json)
        .map_err(|error| ApiError::new(request_id, error))
}

async fn complete_callback(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    path: Result<Path<String>, PathRejection>,
    AuthJson(request): AuthJson<CallbackRequest>,
) -> Result<Json<AuthProgress>, ApiError> {
    let flow_id = parse_flow_id(path, request_id)?;
    if request.callback_url.expose_secret().len() > CALLBACK_STRING_LIMIT {
        return Err(ApiError::new(
            request_id,
            AppError::invalid_request("Callback URL is too long."),
        ));
    }
    let coordinator = coordinator(&state, request_id)?;
    coordinator
        .complete_callback(&flow_id, request.callback_url)
        .await
        .map(Json)
        .map_err(|error| ApiError::new(request_id, map_flow_error(error)))
}

async fn submit_challenge(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    path: Result<Path<String>, PathRejection>,
    AuthJson(input): AuthJson<ChallengeInput>,
) -> Result<Json<AuthProgress>, ApiError> {
    let flow_id = parse_flow_id(path, request_id)?;
    let coordinator = coordinator(&state, request_id)?;
    coordinator
        .submit(&flow_id, input)
        .await
        .map(Json)
        .map_err(|error| ApiError::new(request_id, map_challenge_error(error)))
}

async fn start_legacy_sms(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    AuthJson(request): AuthJson<LegacySmsStartRequest>,
) -> Result<Json<AuthProgress>, ApiError> {
    if !request.accept_legacy_sms {
        return Err(ApiError::new(
            request_id,
            AppError::invalid_request("Legacy SMS authentication requires explicit consent."),
        ));
    }
    let legacy = state.legacy_sms_auth().cloned().ok_or_else(|| {
        ApiError::new(
            request_id,
            AppError::new(
                ErrorCode::Internal,
                "Legacy SMS authentication is unavailable.",
                false,
            ),
        )
    })?;
    legacy
        .start(request.phone)
        .await
        .map(Json)
        .map_err(|error| ApiError::new(request_id, error))
}

async fn confirm_legacy_sms(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    path: Result<Path<String>, PathRejection>,
    AuthJson(request): AuthJson<LegacySmsConfirmRequest>,
) -> Result<Json<AuthProgress>, ApiError> {
    let flow_id = parse_flow_id(path, request_id)?;
    let legacy = state.legacy_sms_auth().cloned().ok_or_else(|| {
        ApiError::new(
            request_id,
            AppError::new(
                ErrorCode::Internal,
                "Legacy SMS authentication is unavailable.",
                false,
            ),
        )
    })?;
    let coordinator = coordinator(&state, request_id)?;
    let confirmation = coordinator.prepare_legacy_confirmation().await;
    coordinator
        .complete_legacy_sms(confirmation, &legacy, &flow_id, request.code)
        .await
        .map(Json)
        .map_err(|error| ApiError::new(request_id, error))
}

async fn session(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
) -> Result<Json<SessionView>, ApiError> {
    let coordinator = coordinator(&state, request_id)?;
    Ok(Json(coordinator.session().await))
}

async fn refresh(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
) -> Result<Json<SessionView>, ApiError> {
    let coordinator = coordinator(&state, request_id)?;
    coordinator
        .refresh()
        .await
        .map(Json)
        .map_err(|error| ApiError::new(request_id, error))
}

async fn logout(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
) -> Result<Json<LogoutResult>, ApiError> {
    let coordinator = coordinator(&state, request_id)?;
    coordinator
        .logout()
        .await
        .map(Json)
        .map_err(|error| ApiError::new(request_id, error))
}

async fn compatibility_login(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    AuthJson(request): AuthJson<LegacyLoginRequest>,
) -> Result<Json<AuthProgress>, ApiError> {
    drop(request.phone);
    let coordinator = coordinator(&state, request_id)?;
    coordinator
        .start()
        .await
        .map(Json)
        .map_err(|error| ApiError::new(request_id, error))
}

async fn compatibility_confirm(
    Extension(request_id): Extension<RequestId>,
    AuthJson(request): AuthJson<LegacyConfirmRequest>,
) -> Result<Json<AuthProgress>, ApiError> {
    let LegacyConfirmRequest {
        phone,
        code,
        contract,
    } = request;
    drop((phone, code, contract));
    Err(ApiError::new(request_id, challenge_capture_required()))
}

pub(super) async fn method_not_allowed(Extension(request_id): Extension<RequestId>) -> ApiError {
    ApiError::new(
        request_id,
        AppError::new(ErrorCode::MethodNotAllowed, "Method not allowed.", false),
    )
}

fn coordinator(state: &AppState, request_id: RequestId) -> Result<Arc<AuthCoordinator>, ApiError> {
    state.auth_coordinator().cloned().ok_or_else(|| {
        ApiError::new(
            request_id,
            AppError::new(
                ErrorCode::Internal,
                "Authentication service is unavailable.",
                false,
            ),
        )
    })
}

fn parse_flow_id(
    path: Result<Path<String>, PathRejection>,
    request_id: RequestId,
) -> Result<AuthFlowId, ApiError> {
    let Path(raw_flow_id) = path.map_err(|_| {
        ApiError::new(
            request_id,
            AppError::invalid_request("Authentication flow ID is invalid."),
        )
    })?;
    raw_flow_id.parse().map_err(|_| {
        ApiError::new(
            request_id,
            AppError::invalid_request("Authentication flow ID is invalid."),
        )
    })
}

fn map_flow_error(error: AppError) -> AppError {
    if matches!(
        error.code,
        ErrorCode::SessionExpired | ErrorCode::AuthFlowNotFound
    ) {
        auth_flow_not_found()
    } else {
        error
    }
}

fn map_challenge_error(error: AppError) -> AppError {
    match error.code {
        ErrorCode::SessionExpired | ErrorCode::AuthFlowNotFound => auth_flow_not_found(),
        ErrorCode::UpstreamSchemaChanged | ErrorCode::AuthenticationChallengeRequired => {
            challenge_capture_required()
        }
        _ => error,
    }
}

fn auth_flow_not_found() -> AppError {
    AppError::new(
        ErrorCode::AuthFlowNotFound,
        "Authentication flow was not found.",
        false,
    )
}

fn challenge_capture_required() -> AppError {
    AppError::new(
        ErrorCode::AuthenticationChallengeRequired,
        "MTS browser challenge capture is required before challenge submission is supported.",
        false,
    )
}
