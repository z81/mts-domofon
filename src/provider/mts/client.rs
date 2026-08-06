use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use futures_util::StreamExt;
use reqwest::{
    Client, Method, Request, RequestBuilder, Response, StatusCode,
    header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderName, HeaderValue, RETRY_AFTER},
    redirect::Policy,
};
use secrecy::{ExposeSecret, SecretString};
use serde::de::DeserializeOwned;
use tokio::sync::Mutex;
use url::Url;
use uuid::Uuid;

use crate::{
    config::AppConfig,
    error::{AppError, ErrorCode},
};

const MAX_JSON_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_ERROR_BODY_BYTES: usize = 16 * 1024;
const MAX_EMPTY_BODY_BYTES: usize = 16 * 1024;
const MAX_CORRELATION_LEN: usize = 256;

const DEVICE_TOKEN: &str = "x-device-token";
const DEVICE_TYPE: &str = "x-device-type";
const REQUEST_ID: &str = "request-id";
const X_REQUEST_ID: &str = "x-request-id";

#[async_trait]
pub trait TokenSource: Send + Sync {
    async fn access_token(&self) -> Option<SecretString>;
    async fn device_token(&self) -> Option<SecretString>;
    async fn refresh_after_unauthorized(&self) -> Result<(), AppError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoTokens;

#[async_trait]
impl TokenSource for NoTokens {
    async fn access_token(&self) -> Option<SecretString> {
        None
    }

    async fn device_token(&self) -> Option<SecretString> {
        None
    }

    async fn refresh_after_unauthorized(&self) -> Result<(), AppError> {
        Err(session_expired())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderBase {
    Gateway,
    Resident,
}

#[derive(Clone)]
pub struct MtsClient {
    inner: Arc<Inner>,
}

struct Inner {
    http: Client,
    gateway: Url,
    resident: Url,
    tokens: Arc<dyn TokenSource>,
    refresh: Mutex<()>,
}

impl MtsClient {
    pub fn new(config: &AppConfig, tokens: Arc<dyn TokenSource>) -> Result<Self, AppError> {
        let http = Client::builder()
            .redirect(Policy::none())
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(transport_error)?;
        Ok(Self::from_parts(
            http,
            config.gateway.clone(),
            config.resident.clone(),
            tokens,
        ))
    }

    /// Explicit low-level constructor for local provider contract tests.
    /// Production callers should use [`Self::new`] so [`AppConfig`] host validation applies.
    #[doc(hidden)]
    pub fn new_for_test(
        http: Client,
        gateway: Url,
        resident: Url,
        tokens: Arc<dyn TokenSource>,
    ) -> Self {
        Self::from_parts(http, gateway, resident, tokens)
    }

    fn from_parts(http: Client, gateway: Url, resident: Url, tokens: Arc<dyn TokenSource>) -> Self {
        Self {
            inner: Arc::new(Inner {
                http,
                gateway,
                resident,
                tokens,
                refresh: Mutex::new(()),
            }),
        }
    }

    pub fn endpoint(&self, base: ProviderBase, path: &str) -> Result<Url, AppError> {
        if !path.starts_with('/') || path.contains("..") {
            return Err(AppError::invalid_request("invalid provider path"));
        }
        self.base(base)
            .join(path.trim_start_matches('/'))
            .map_err(AppError::url)
    }

    pub async fn authorized(
        &self,
        method: Method,
        base: ProviderBase,
        path: &str,
    ) -> Result<RequestBuilder, AppError> {
        let url = self.endpoint(base, path)?;
        let mut request = self.inner.http.request(method, url);
        let access = self
            .inner
            .tokens
            .access_token()
            .await
            .ok_or_else(session_expired)?;
        request = request.header(AUTHORIZATION, bearer_header(&access)?);
        if let Some(device) = self.inner.tokens.device_token().await {
            request = request.header(DEVICE_TOKEN, secret_header(&device)?);
        }
        Ok(common_headers(request))
    }

    pub fn anonymous_gateway(
        &self,
        method: Method,
        path: &str,
    ) -> Result<RequestBuilder, AppError> {
        if method != Method::GET || !matches!(path, "/config" | "/feature") {
            return Err(AppError::invalid_request(
                "anonymous provider path is not allowed",
            ));
        }
        let url = self.endpoint(ProviderBase::Gateway, path)?;
        Ok(common_headers(self.inner.http.request(method, url)))
    }

    pub async fn send_json<T>(&self, request: RequestBuilder) -> Result<T, AppError>
    where
        T: DeserializeOwned,
    {
        let response = self.execute(request).await?;
        let body = read_bounded(response, MAX_JSON_RESPONSE_BYTES).await?;
        serde_json::from_slice(&body).map_err(|_| schema_error())
    }

    pub async fn send_empty(&self, request: RequestBuilder) -> Result<(), AppError> {
        let response = self.execute(request).await?;
        let _ = read_bounded(response, MAX_EMPTY_BODY_BYTES).await?;
        Ok(())
    }

    pub async fn send_stream(&self, request: RequestBuilder) -> Result<Response, AppError> {
        self.execute(request).await
    }

    async fn execute(&self, builder: RequestBuilder) -> Result<Response, AppError> {
        let request = builder.build().map_err(request_error)?;
        self.validate_request(&request)?;
        let safe_read = matches!(*request.method(), Method::GET | Method::HEAD);
        let is_authorized = request.headers().contains_key(AUTHORIZATION);
        let failed_authorization = request.headers().get(AUTHORIZATION).cloned();
        let retry = request.try_clone();

        let response = self
            .inner
            .http
            .execute(request)
            .await
            .map_err(transport_error)?;
        if response.status() != StatusCode::UNAUTHORIZED || !safe_read || !is_authorized {
            return classify(response, false).await;
        }

        let mut retry = retry.ok_or_else(|| {
            AppError::new(
                ErrorCode::Internal,
                "Provider read could not be retried safely.",
                false,
            )
        })?;
        drop(response);
        let refresh_guard = self.inner.refresh.lock().await;
        if self
            .authorization_is_current(failed_authorization.as_ref())
            .await?
        {
            self.inner.tokens.refresh_after_unauthorized().await?;
        }
        self.replace_credentials(&mut retry).await?;
        drop(refresh_guard);
        let response = self
            .inner
            .http
            .execute(retry)
            .await
            .map_err(transport_error)?;
        classify(response, true).await
    }

    async fn authorization_is_current(
        &self,
        failed: Option<&HeaderValue>,
    ) -> Result<bool, AppError> {
        let current = self
            .inner
            .tokens
            .access_token()
            .await
            .ok_or_else(session_expired)?;
        Ok(failed == Some(&bearer_header(&current)?))
    }

    async fn replace_credentials(&self, request: &mut Request) -> Result<(), AppError> {
        let access = self
            .inner
            .tokens
            .access_token()
            .await
            .ok_or_else(session_expired)?;
        request
            .headers_mut()
            .insert(AUTHORIZATION, bearer_header(&access)?);
        if let Some(device) = self.inner.tokens.device_token().await {
            request.headers_mut().insert(
                HeaderName::from_static(DEVICE_TOKEN),
                secret_header(&device)?,
            );
        } else {
            request.headers_mut().remove(DEVICE_TOKEN);
        }
        request.headers_mut().insert(
            HeaderName::from_static(REQUEST_ID),
            HeaderValue::from_str(&Uuid::new_v4().to_string()).expect("UUID is a valid header"),
        );
        Ok(())
    }

    fn validate_request(&self, request: &Request) -> Result<(), AppError> {
        let url = request.url();
        let known_origin =
            same_origin(url, &self.inner.gateway) || same_origin(url, &self.inner.resident);
        if !known_origin
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(AppError::invalid_request("invalid provider URL"));
        }
        Ok(())
    }

    fn base(&self, base: ProviderBase) -> &Url {
        match base {
            ProviderBase::Gateway => &self.inner.gateway,
            ProviderBase::Resident => &self.inner.resident,
        }
    }
}

fn common_headers(request: RequestBuilder) -> RequestBuilder {
    request
        .header(DEVICE_TYPE, "ANDROID")
        .header(REQUEST_ID, Uuid::new_v4().to_string())
        .header(ACCEPT, "application/json")
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn bearer_header(token: &SecretString) -> Result<HeaderValue, AppError> {
    let value = format!("Bearer {}", token.expose_secret());
    let mut header = HeaderValue::from_str(&value).map_err(|_| invalid_credential())?;
    header.set_sensitive(true);
    Ok(header)
}

fn secret_header(token: &SecretString) -> Result<HeaderValue, AppError> {
    let mut header =
        HeaderValue::from_str(token.expose_secret()).map_err(|_| invalid_credential())?;
    header.set_sensitive(true);
    Ok(header)
}

fn invalid_credential() -> AppError {
    AppError::new(
        ErrorCode::Internal,
        "Provider credential cannot be encoded.",
        false,
    )
}

async fn classify(
    response: Response,
    unauthorized_after_refresh: bool,
) -> Result<Response, AppError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    let upstream_request_id = correlation_id(response.headers());
    let retry_after_seconds = retry_after(response.headers());
    let _ = drain_bounded(response, MAX_ERROR_BODY_BYTES).await;

    let mut error = match status {
        StatusCode::UNAUTHORIZED if unauthorized_after_refresh => session_expired(),
        StatusCode::UNAUTHORIZED => AppError::new(
            ErrorCode::UpstreamAuthenticationFailed,
            "Upstream authentication failed.",
            false,
        ),
        StatusCode::FORBIDDEN => AppError::new(
            ErrorCode::Forbidden,
            "Provider request was forbidden.",
            false,
        ),
        StatusCode::NOT_FOUND => AppError::new(
            ErrorCode::NotFound,
            "Provider resource was not found.",
            false,
        ),
        StatusCode::CONFLICT | StatusCode::UNPROCESSABLE_ENTITY => AppError::new(
            ErrorCode::InvalidRequest,
            "Provider rejected the request.",
            false,
        ),
        StatusCode::TOO_MANY_REQUESTS => AppError::new(
            ErrorCode::RateLimited,
            "Provider rate limit was reached.",
            true,
        ),
        status if status.is_server_error() => AppError::new(
            ErrorCode::UpstreamUnavailable,
            "Provider is unavailable.",
            true,
        ),
        _ => AppError::new(
            ErrorCode::UpstreamUnavailable,
            "Provider returned an unexpected response.",
            false,
        ),
    };
    error.upstream_request_id = upstream_request_id;
    error.retry_after_seconds = retry_after_seconds;
    Err(error)
}

fn correlation_id(headers: &HeaderMap) -> Option<String> {
    [REQUEST_ID, X_REQUEST_ID].into_iter().find_map(|name| {
        let value = headers.get(name)?.to_str().ok()?;
        (value.len() <= MAX_CORRELATION_LEN).then(|| value.to_owned())
    })
}

fn retry_after(headers: &HeaderMap) -> Option<u64> {
    headers.get(RETRY_AFTER)?.to_str().ok()?.parse().ok()
}

async fn read_bounded(response: Response, limit: usize) -> Result<Bytes, AppError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(schema_error());
    }
    let mut stream = response.bytes_stream();
    let mut body = BytesMut::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(transport_error)?;
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(schema_error());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body.freeze())
}

async fn drain_bounded(response: Response, limit: usize) -> Result<(), AppError> {
    let mut stream = response.bytes_stream();
    let mut read = 0_usize;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(transport_error)?;
        read = read.saturating_add(chunk.len());
        if read > limit {
            break;
        }
    }
    Ok(())
}

fn session_expired() -> AppError {
    AppError::new(
        ErrorCode::SessionExpired,
        "Provider session expired.",
        false,
    )
}

fn schema_error() -> AppError {
    AppError::new(
        ErrorCode::UpstreamSchemaChanged,
        "Provider response has an unsupported schema.",
        false,
    )
}

fn request_error(cause: reqwest::Error) -> AppError {
    AppError::invalid_request("invalid provider request").with_cause(cause)
}

fn transport_error(cause: reqwest::Error) -> AppError {
    AppError::new(
        ErrorCode::UpstreamUnavailable,
        "Provider request failed.",
        true,
    )
    .with_cause(cause)
}
