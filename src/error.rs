use std::fmt;

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::request_id::RequestId;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    AuthenticationChallengeRequired,
    UpstreamAuthenticationFailed,
    SessionExpired,
    Forbidden,
    NotFound,
    RateLimited,
    UpstreamUnavailable,
    UpstreamSchemaChanged,
    StorageFailure,
    Internal,
}

impl ErrorCode {
    fn status(self) -> StatusCode {
        match self {
            Self::InvalidRequest => StatusCode::BAD_REQUEST,
            Self::AuthenticationChallengeRequired
            | Self::UpstreamAuthenticationFailed
            | Self::SessionExpired => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::UpstreamUnavailable | Self::StorageFailure => StatusCode::SERVICE_UNAVAILABLE,
            Self::UpstreamSchemaChanged => StatusCode::BAD_GATEWAY,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

pub struct AppError {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
    pub upstream_request_id: Option<String>,
    pub retry_after_seconds: Option<u64>,
    cause: Option<String>,
}

impl AppError {
    pub fn new(code: ErrorCode, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
            upstream_request_id: None,
            retry_after_seconds: None,
            cause: None,
        }
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidRequest, message, false)
    }

    pub fn url(cause: impl fmt::Display) -> Self {
        Self::new(ErrorCode::InvalidRequest, "Invalid provider URL.", false).with_cause(cause)
    }

    pub fn upstream_auth(cause: impl Into<String>, upstream_request_id: Option<String>) -> Self {
        Self {
            code: ErrorCode::UpstreamAuthenticationFailed,
            message: "Upstream authentication failed.".into(),
            retryable: false,
            upstream_request_id,
            retry_after_seconds: None,
            cause: Some(cause.into()),
        }
    }

    pub fn with_cause(mut self, cause: impl fmt::Display) -> Self {
        self.cause = Some(cause.to_string());
        self
    }

    #[cfg(test)]
    pub(crate) fn retained_cause_for_test(&self) -> Option<&str> {
        self.cause.as_deref()
    }

    fn log_redacted(&self) {
        tracing::warn!(
            code = ?self.code,
            retryable = self.retryable,
            cause = %Redacted(self.cause.as_deref()),
            upstream_request_id = %Redacted(self.upstream_request_id.as_deref()),
            "request failed"
        );
    }
}

impl fmt::Debug for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AppError")
            .field("code", &self.code)
            .field("message", &self.message)
            .field("retryable", &self.retryable)
            .field(
                "upstream_request_id",
                &Redacted(self.upstream_request_id.as_deref()),
            )
            .field("retry_after_seconds", &self.retry_after_seconds)
            .field("cause", &Redacted(self.cause.as_deref()))
            .finish()
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AppError {}

pub struct ApiError {
    pub request_id: RequestId,
    pub source: AppError,
}

impl ApiError {
    pub fn new(request_id: RequestId, source: AppError) -> Self {
        Self { request_id, source }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        self.source.log_redacted();
        let status = self.source.code.status();
        let envelope = ErrorEnvelope {
            code: self.source.code,
            message: self.source.message,
            retryable: self.source.retryable,
            request_id: self.request_id.to_string(),
            retry_after_seconds: self.source.retry_after_seconds,
        };
        (status, Json(envelope)).into_response()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorEnvelope {
    code: ErrorCode,
    message: String,
    retryable: bool,
    request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_after_seconds: Option<u64>,
}

struct Redacted<'a>(Option<&'a str>);

impl fmt::Display for Redacted<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(if self.0.is_some() {
            "[REDACTED]"
        } else {
            "[NONE]"
        })
    }
}

impl fmt::Debug for Redacted<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}
