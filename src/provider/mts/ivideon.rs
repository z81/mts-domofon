use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::{Client, StatusCode, header::ACCEPT, redirect::Policy};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use tokio::time::{Instant, sleep, timeout_at};
#[cfg(feature = "test-support")]
use url::Host;
use url::Url;
use zeroize::Zeroizing;

use crate::error::{AppError, ErrorCode};

const API_BASE: &str = "https://openapi-alpha-eu01.ivideon.com/";
const API_HOST: &str = "openapi-alpha-eu01.ivideon.com";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const RESOLUTION_TIMEOUT: Duration = Duration::from_secs(30);
const RETRY_DELAY: Duration = Duration::from_millis(250);
const MAX_RETRIES: usize = 3;
const MAX_RESPONSE_BYTES: usize = 16 * 1024;
const MAX_CAMERA_ID_BYTES: usize = 512;
const MAX_ACCESS_TOKEN_BYTES: usize = 16 * 1024;
const MAX_LIVE_URL_BYTES: usize = 4_096;

const QUALITY_HIGH: &str = "2";
const VIDEO_CODECS: &str = "h265,h264";
const AUDIO_CODECS: &str = "pcmu,pcma,aac,mp3";

#[async_trait]
pub trait IvideonLiveUrlResolver: Send + Sync {
    async fn resolve_live_url(
        &self,
        camera_id: &str,
        access_token: &SecretString,
    ) -> Result<SecretString, AppError>;
}

#[derive(Clone)]
pub struct IvideonLiveResolver {
    http: Option<Client>,
    base: Option<Url>,
}

impl IvideonLiveResolver {
    pub(crate) fn production() -> Self {
        let base = Url::parse(API_BASE).ok().filter(valid_production_base);
        Self {
            http: resolver_http_client().ok(),
            base,
        }
    }

    #[cfg(feature = "test-support")]
    pub fn new_for_test(base: Url) -> Result<Self, AppError> {
        if !valid_test_base(&base) {
            return Err(AppError::invalid_request("invalid Ivideon test base URL"));
        }
        Ok(Self {
            http: Some(resolver_http_client()?),
            base: Some(base),
        })
    }

    fn endpoint(&self, camera_id: &str) -> Result<Url, AppError> {
        validate_credential(camera_id, MAX_CAMERA_ID_BYTES)?;
        let base = self.base.as_ref().ok_or_else(ivideon_unavailable)?;
        let mut target = base.clone();
        {
            let mut segments = target
                .path_segments_mut()
                .map_err(|_| ivideon_unavailable())?;
            segments.clear();
            segments.push("cameras");
            segments.push(camera_id);
            segments.push("live_stream");
        }
        target
            .query_pairs_mut()
            .append_pair("expect", "body")
            .append_pair("q", QUALITY_HIGH)
            .append_pair("video_codecs", VIDEO_CODECS)
            .append_pair("audio_codecs", AUDIO_CODECS);
        if !same_origin(&target, base) || target.fragment().is_some() {
            return Err(ivideon_unavailable());
        }
        Ok(target)
    }

    async fn request_live_url(&self, target: Url) -> Result<SecretString, AppError> {
        let http = self.http.as_ref().ok_or_else(ivideon_unavailable)?;
        let response = http
            .get(target)
            .header(ACCEPT, "application/json")
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|_| ivideon_unavailable())?;
        let status = response.status();
        if !status.is_success() {
            return Err(status_error(status));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return Err(ivideon_schema_error());
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !content_type.starts_with("application/json") {
            return Err(ivideon_schema_error());
        }

        let mut body = Zeroizing::new(Vec::new());
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| ivideon_unavailable())?;
            if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err(ivideon_schema_error());
            }
            body.extend_from_slice(&chunk);
        }
        let payload: VideoUrl =
            serde_json::from_slice(&body).map_err(|_| ivideon_schema_error())?;
        validate_live_url(payload.url.expose_secret())
    }
}

#[async_trait]
impl IvideonLiveUrlResolver for IvideonLiveResolver {
    async fn resolve_live_url(
        &self,
        camera_id: &str,
        access_token: &SecretString,
    ) -> Result<SecretString, AppError> {
        validate_credential(access_token.expose_secret(), MAX_ACCESS_TOKEN_BYTES)?;
        let mut target = self.endpoint(camera_id)?;
        target
            .query_pairs_mut()
            .append_pair("access_token", access_token.expose_secret());
        let deadline = Instant::now() + RESOLUTION_TIMEOUT;
        for attempt in 0..=MAX_RETRIES {
            match timeout_at(deadline, self.request_live_url(target.clone()))
                .await
                .map_err(|_| ivideon_unavailable())?
            {
                Ok(source) => return Ok(source),
                Err(error) if !error.retryable || attempt == MAX_RETRIES => return Err(error),
                Err(_) => {
                    timeout_at(deadline, sleep(RETRY_DELAY))
                        .await
                        .map_err(|_| ivideon_unavailable())?;
                }
            }
        }
        Err(ivideon_unavailable())
    }
}

#[derive(Deserialize)]
struct VideoUrl {
    url: SecretString,
}

fn resolver_http_client() -> Result<Client, AppError> {
    Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .redirect(Policy::none())
        .referer(false)
        .build()
        .map_err(|_| ivideon_unavailable())
}

fn valid_production_base(base: &Url) -> bool {
    base.scheme() == "https"
        && base.host_str() == Some(API_HOST)
        && base.port().is_none()
        && base.path() == "/"
        && base.query().is_none()
        && base.fragment().is_none()
        && base.username().is_empty()
        && base.password().is_none()
}

#[cfg(feature = "test-support")]
fn valid_test_base(base: &Url) -> bool {
    base.scheme() == "http"
        && matches!(base.host(), Some(Host::Ipv4(address)) if address.is_loopback())
        && base.port().is_some()
        && base.path() == "/"
        && base.query().is_none()
        && base.fragment().is_none()
        && base.username().is_empty()
        && base.password().is_none()
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn validate_credential(value: &str, max_bytes: usize) -> Result<(), AppError> {
    if value.trim().is_empty() || value.len() > max_bytes || value.chars().any(char::is_control) {
        return Err(ivideon_schema_error());
    }
    Ok(())
}

fn validate_live_url(value: &str) -> Result<SecretString, AppError> {
    validate_credential(value, MAX_LIVE_URL_BYTES)?;
    let url = Url::parse(value).map_err(|_| ivideon_schema_error())?;
    let valid_transport = matches!(url.scheme(), "rtsp" | "rtsps")
        || (url.scheme() == "https" && url.username().is_empty() && url.password().is_none());
    if !valid_transport || url.host().is_none() || url.fragment().is_some() {
        return Err(ivideon_schema_error());
    }
    Ok(SecretString::from(value.to_owned()))
}

fn status_error(status: StatusCode) -> AppError {
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        AppError::new(
            ErrorCode::UpstreamAuthenticationFailed,
            "Ivideon authentication failed.",
            false,
        )
    } else if status.is_redirection() {
        AppError::new(
            ErrorCode::UpstreamUnavailable,
            "Ivideon live stream is temporarily unavailable.",
            false,
        )
    } else if status.is_server_error() {
        ivideon_unavailable()
    } else {
        ivideon_schema_error()
    }
}

fn ivideon_unavailable() -> AppError {
    AppError::new(
        ErrorCode::UpstreamUnavailable,
        "Ivideon live stream is temporarily unavailable.",
        true,
    )
}

fn ivideon_schema_error() -> AppError {
    AppError::new(
        ErrorCode::UpstreamSchemaChanged,
        "Ivideon live stream response has an unexpected schema.",
        false,
    )
}
