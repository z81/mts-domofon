use async_trait::async_trait;
use reqwest::Method;
use secrecy::{ExposeSecret, SecretString};
use serde::Serialize;

use crate::{
    error::{AppError, ErrorCode},
    service::calls::CallOpenProvider,
};

use super::{MtsClient, ProviderBase};

const INTERCOM_PREFIX: &str = "/vdome-smart-facade/devices/api/v1/intercoms";
const CALLS_PREFIX: &str = "/domofon/api/intercom/v1/calls";
const MAX_OPAQUE_ID_BYTES: usize = 2 * 1024;

#[derive(Clone)]
pub struct MtsCallOpenProvider {
    client: MtsClient,
}

impl MtsCallOpenProvider {
    pub fn new(client: MtsClient) -> Self {
        Self { client }
    }
}

#[derive(Serialize)]
struct OpenRequest {
    action: &'static str,
}

#[async_trait]
impl CallOpenProvider for MtsCallOpenProvider {
    async fn open(
        &self,
        intercom_id: &SecretString,
        call_session_id: &SecretString,
    ) -> Result<(), AppError> {
        let intercom_id = validate_secret_id(intercom_id)?;
        validate_secret_id(call_session_id)?;
        let request = self
            .client
            .authorized_with_path_segment(
                Method::POST,
                ProviderBase::Gateway,
                INTERCOM_PREFIX,
                intercom_id,
                Some("lock"),
            )?
            .call_session_id_header(call_session_id)?
            .json(&OpenRequest { action: "open" })?;
        self.client
            .send_empty(request)
            .await
            .map_err(map_physical_mutation_error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteCallAction {
    Ignore,
    Unwanted,
}

impl RemoteCallAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ignore => "ignore",
            Self::Unwanted => "unwanted",
        }
    }
}

#[async_trait]
pub trait CallActionProvider: Send + Sync {
    async fn action(
        &self,
        call_session_id: &SecretString,
        action: RemoteCallAction,
    ) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct MtsCallActionProvider {
    client: MtsClient,
}

impl MtsCallActionProvider {
    pub fn new(client: MtsClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl CallActionProvider for MtsCallActionProvider {
    async fn action(
        &self,
        call_session_id: &SecretString,
        action: RemoteCallAction,
    ) -> Result<(), AppError> {
        let call_session_id = validate_secret_id(call_session_id)?;
        let request = self
            .client
            .authorized_with_two_path_segments(
                Method::POST,
                ProviderBase::Gateway,
                CALLS_PREFIX,
                call_session_id,
                "action",
                action.as_str(),
            )?
            .explicit_empty_body();
        self.client
            .send_empty(request)
            .await
            .map_err(map_non_retrying_mutation_error)
    }
}

fn validate_secret_id(value: &SecretString) -> Result<&str, AppError> {
    let value = value.expose_secret();
    if value.is_empty()
        || value.len() > MAX_OPAQUE_ID_BYTES
        || matches!(value, "." | "..")
        || value.chars().any(char::is_control)
    {
        return Err(AppError::invalid_request(
            "invalid provider call identifier",
        ));
    }
    Ok(value)
}

fn map_physical_mutation_error(mut error: AppError) -> AppError {
    if error.code == ErrorCode::UpstreamUnavailable {
        let mut outcome = AppError::new(
            ErrorCode::MutationOutcomeUnknown,
            "Call-scoped door-open outcome is unknown.",
            false,
        );
        outcome.upstream_request_id = error.upstream_request_id.take();
        return outcome;
    }
    error.retryable = false;
    error
}

fn map_non_retrying_mutation_error(mut error: AppError) -> AppError {
    error.retryable = false;
    error
}
