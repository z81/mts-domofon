use async_trait::async_trait;
use reqwest::Method;
use serde::Serialize;

use crate::error::{AppError, ErrorCode};

use super::{MtsClient, ProviderBase};

const INTERCOM_PREFIX: &str = "/vdome-smart-facade/devices/api/v1/intercoms";
const MAX_OPAQUE_ID_BYTES: usize = 512;

#[async_trait]
pub trait DoorOpenProvider: Send + Sync {
    async fn open(&self, access_point_id: &str, lock_number: &str) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct MtsDoorOpenProvider {
    client: MtsClient,
}

impl MtsDoorOpenProvider {
    pub fn new(client: MtsClient) -> Self {
        Self { client }
    }
}

#[derive(Serialize)]
struct OpenDoorRequest {
    action: &'static str,
}

#[async_trait]
impl DoorOpenProvider for MtsDoorOpenProvider {
    async fn open(&self, access_point_id: &str, lock_number: &str) -> Result<(), AppError> {
        validate_id(access_point_id)?;
        validate_id(lock_number)?;
        let request = self
            .client
            .authorized_with_two_path_segments(
                Method::POST,
                ProviderBase::Gateway,
                INTERCOM_PREFIX,
                access_point_id,
                "lock",
                lock_number,
            )?
            .json(&OpenDoorRequest { action: "open" })?;
        self.client
            .send_empty(request)
            .await
            .map_err(map_open_error)
    }
}

fn map_open_error(mut error: AppError) -> AppError {
    if error.code == ErrorCode::UpstreamUnavailable {
        let mut outcome = AppError::new(
            ErrorCode::MutationOutcomeUnknown,
            "Door-open outcome is unknown.",
            false,
        );
        outcome.upstream_request_id = error.upstream_request_id.take();
        return outcome;
    }
    // The one-use intent was consumed before dispatch, so retrying this local request is never a
    // valid recovery even when the provider supplied a retry delay.
    error.retryable = false;
    error
}

fn validate_id(value: &str) -> Result<(), AppError> {
    if value.is_empty()
        || value.len() > MAX_OPAQUE_ID_BYTES
        || matches!(value, "." | "..")
        || value.chars().any(char::is_control)
    {
        return Err(AppError::invalid_request(
            "invalid door resource identifier",
        ));
    }
    Ok(())
}
