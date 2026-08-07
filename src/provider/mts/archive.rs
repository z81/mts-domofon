use async_trait::async_trait;
use chrono::{DateTime, NaiveDateTime, SecondsFormat, Utc};
use reqwest::Method;
use secrecy::SecretString;
use serde::Deserialize;
use url::Url;

use crate::{
    domain::archive::ArchiveInterval,
    error::{AppError, ErrorCode},
};

use super::{MtsClient, ProviderBase};

const CAMERA_PREFIX: &str = "/domofon/api/intercom/v1/cameras";
const MAX_CAMERA_ID_BYTES: usize = 512;
const MAX_RESPONSE_CAMERAS: usize = 128;
const MAX_INTERVALS: usize = 2_048;
const MAX_SOURCE_BYTES: usize = 4_096;

#[async_trait]
pub trait ArchiveProvider: Send + Sync {
    async fn distribution(&self, camera_id: &str) -> Result<Vec<ArchiveInterval>, AppError>;

    async fn source(
        &self,
        camera_id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<SecretString, AppError>;
}

#[derive(Clone)]
pub struct MtsArchiveProvider {
    client: MtsClient,
}

impl MtsArchiveProvider {
    pub fn new(client: MtsClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ArchiveProvider for MtsArchiveProvider {
    async fn distribution(&self, camera_id: &str) -> Result<Vec<ArchiveInterval>, AppError> {
        validate_camera_id(camera_id)?;
        let request = self.client.authorized_with_two_path_segments(
            Method::GET,
            ProviderBase::Gateway,
            CAMERA_PREFIX,
            camera_id,
            "archive",
            "distribution",
        )?;
        let response: DistributionResponse = self.client.send_json(request).await?;
        response.for_camera(camera_id)
    }

    async fn source(
        &self,
        camera_id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<SecretString, AppError> {
        validate_camera_id(camera_id)?;
        if start >= end {
            return Err(AppError::invalid_request("invalid archive range"));
        }
        let start = provider_time(start);
        let end = provider_time(end);
        let request = self
            .client
            .authorized_with_path_segment(
                Method::GET,
                ProviderBase::Gateway,
                CAMERA_PREFIX,
                camera_id,
                Some("archive"),
            )?
            .query_pairs(&[("start", &start), ("end", &end)])?;
        let response: SourceResponse = self.client.send_json(request).await?;
        response.for_camera(camera_id)
    }
}

#[derive(Deserialize)]
struct DistributionResponse {
    list: Vec<DistributionCamera>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct DistributionCamera {
    id: Option<String>,
    distribution: Vec<DistributionInterval>,
}

#[derive(Deserialize)]
struct DistributionInterval {
    from: Option<String>,
    to: Option<String>,
}

impl DistributionResponse {
    fn for_camera(self, camera_id: &str) -> Result<Vec<ArchiveInterval>, AppError> {
        validate_application_error(self.error)?;
        if self.list.len() > MAX_RESPONSE_CAMERAS
            || self
                .list
                .iter()
                .map(|camera| camera.distribution.len())
                .try_fold(0_usize, usize::checked_add)
                .is_none_or(|count| count > MAX_INTERVALS)
        {
            return Err(schema_error());
        }
        let mut matching = self
            .list
            .into_iter()
            .filter(|camera| camera.id.as_deref() == Some(camera_id));
        let camera = matching.next().ok_or_else(archive_not_found)?;
        if matching.next().is_some() {
            return Err(schema_error());
        }
        let mut intervals = camera
            .distribution
            .into_iter()
            .map(|interval| {
                let from = interval
                    .from
                    .as_deref()
                    .and_then(parse_provider_time)
                    .ok_or_else(schema_error)?;
                let to = interval
                    .to
                    .as_deref()
                    .and_then(parse_provider_time)
                    .ok_or_else(schema_error)?;
                if from >= to {
                    return Err(schema_error());
                }
                Ok(ArchiveInterval { from, to })
            })
            .collect::<Result<Vec<_>, AppError>>()?;
        intervals.sort_by_key(|interval| (interval.from, interval.to));
        if intervals.windows(2).any(|pair| pair[0].to > pair[1].from) {
            return Err(schema_error());
        }
        Ok(intervals)
    }
}

#[derive(Deserialize)]
struct SourceResponse {
    list: Vec<SourceCamera>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct SourceCamera {
    id: Option<String>,
    rtsp: Option<String>,
}

impl SourceResponse {
    fn for_camera(self, camera_id: &str) -> Result<SecretString, AppError> {
        validate_application_error(self.error)?;
        if self.list.len() > MAX_RESPONSE_CAMERAS {
            return Err(schema_error());
        }
        let mut matching = self
            .list
            .into_iter()
            .filter(|camera| camera.id.as_deref() == Some(camera_id));
        let camera = matching.next().ok_or_else(archive_not_found)?;
        if matching.next().is_some() {
            return Err(schema_error());
        }
        let source = camera.rtsp.ok_or_else(schema_error)?;
        validate_source(&source)?;
        Ok(SecretString::from(source))
    }
}

fn validate_camera_id(camera_id: &str) -> Result<(), AppError> {
    if camera_id.is_empty()
        || camera_id.len() > MAX_CAMERA_ID_BYTES
        || matches!(camera_id, "." | "..")
        || camera_id.chars().any(char::is_control)
    {
        return Err(AppError::invalid_request("invalid camera ID"));
    }
    Ok(())
}

fn validate_source(source: &str) -> Result<(), AppError> {
    if source.is_empty()
        || source.len() > MAX_SOURCE_BYTES
        || source.contains(['\'', '\\'])
        || source.chars().any(char::is_control)
    {
        return Err(schema_error());
    }
    let url = Url::parse(source).map_err(|_| schema_error())?;
    if !matches!(url.scheme(), "https" | "rtsp" | "rtsps")
        || url.host().is_none()
        || url.fragment().is_some()
        || (url.scheme() == "https" && (!url.username().is_empty() || url.password().is_some()))
    {
        return Err(schema_error());
    }
    Ok(())
}

fn provider_time(value: DateTime<Utc>) -> String {
    value
        .to_rfc3339_opts(SecondsFormat::Millis, true)
        .trim_end_matches('Z')
        .to_owned()
}

fn parse_provider_time(value: &str) -> Option<DateTime<Utc>> {
    if value.is_empty() || value.len() > 64 || value.chars().any(char::is_control) {
        return None;
    }
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f")
                .map(|value| value.and_utc())
                .ok()
        })
}

fn validate_application_error(error: Option<String>) -> Result<(), AppError> {
    if error.as_deref().is_some_and(|error| !error.is_empty()) {
        return Err(AppError::new(
            ErrorCode::UpstreamUnavailable,
            "Provider archive is unavailable.",
            true,
        ));
    }
    Ok(())
}

fn archive_not_found() -> AppError {
    AppError::new(ErrorCode::NotFound, "Camera archive was not found.", false)
}

fn schema_error() -> AppError {
    AppError::new(
        ErrorCode::UpstreamSchemaChanged,
        "Provider archive response was invalid.",
        false,
    )
}
