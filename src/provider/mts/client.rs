use std::{
    fmt,
    sync::Arc,
    time::{Duration, SystemTime},
};

use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use futures_util::{Stream, StreamExt, pin_mut};
use reqwest::{
    Client, Method, Request, RequestBuilder, Response, StatusCode,
    header::{
        ACCEPT, AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderValue, RETRY_AFTER,
    },
    redirect::Policy,
};
use secrecy::{ExposeSecret, SecretString};
use serde::{Serialize, de::DeserializeOwned};
use tokio::sync::Mutex;
use url::Url;
use uuid::Uuid;

use crate::{
    config::AppConfig,
    error::{AppError, ErrorCode},
};

const MAX_JSON_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_JSON_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_ERROR_BODY_BYTES: usize = 16 * 1024;
const MAX_EMPTY_BODY_BYTES: usize = 16 * 1024;
const MAX_CORRELATION_LEN: usize = 256;
const MAX_RETRY_AFTER_SECONDS: u64 = 60 * 60;

const DEVICE_TOKEN: &str = "x-device-token";
const DEVICE_TYPE: &str = "x-device-type";
const REQUEST_ID: &str = "request-id";
const X_AUTH_TOKEN: &str = "x-auth-token";
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResponseAccept {
    Json,
    ImageWebp,
}

impl ResponseAccept {
    const fn as_header_value(self) -> &'static str {
        match self {
            Self::Json => "application/json",
            Self::ImageWebp => "image/webp",
        }
    }
}

#[derive(Clone)]
pub struct PreparedRequest {
    owner: Uuid,
    method: Method,
    base: ProviderBase,
    target: PreparedTarget,
    authorization: Authorization,
    response_accept: ResponseAccept,
    body: PreparedBody,
}

impl PreparedRequest {
    pub fn json<T>(mut self, value: &T) -> Result<Self, AppError>
    where
        T: Serialize + ?Sized,
    {
        let body = serde_json::to_vec(value).map_err(|_| invalid_body())?;
        if body.len() > MAX_JSON_REQUEST_BYTES {
            return Err(AppError::invalid_request(
                "provider request body is too large",
            ));
        }
        self.body = PreparedBody::Json(body);
        Ok(self)
    }

    pub fn response_accept(mut self, response_accept: ResponseAccept) -> Self {
        self.response_accept = response_accept;
        self
    }

    pub fn explicit_empty_body(mut self) -> Self {
        self.body = PreparedBody::ExplicitEmpty;
        self
    }
}

impl fmt::Debug for PreparedRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedRequest")
            .field("method", &self.method)
            .field("base", &self.base)
            .field("authorization", &self.authorization)
            .field("response_accept", &self.response_accept)
            .field("has_body", &!matches!(self.body, PreparedBody::Empty))
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Authorization {
    AnonymousBootstrap,
    Authorized,
}

#[derive(Clone)]
enum PreparedBody {
    Empty,
    ExplicitEmpty,
    Json(Vec<u8>),
}

#[derive(Clone)]
enum PreparedTarget {
    ValidatedRawPath(String),
    ClientBuiltUrl(Url),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DispatchKind {
    Initial,
    RetryAfterUnauthorized,
}

#[derive(Clone)]
pub struct MtsClient {
    inner: Arc<Inner>,
}

struct Inner {
    id: Uuid,
    http: Client,
    gateway: Url,
    resident: Url,
    tokens: Arc<dyn TokenSource>,
    refresh: Mutex<()>,
}

impl MtsClient {
    pub fn new(config: &AppConfig, tokens: Arc<dyn TokenSource>) -> Result<Self, AppError> {
        let http = provider_http_client()?;
        Ok(Self::from_parts(
            http,
            config.gateway.clone(),
            config.resident.clone(),
            tokens,
        ))
    }

    #[cfg(feature = "test-support")]
    pub fn new_for_test(
        gateway: Url,
        resident: Url,
        tokens: Arc<dyn TokenSource>,
    ) -> Result<Self, AppError> {
        Ok(Self::from_parts(
            provider_http_client()?,
            gateway,
            resident,
            tokens,
        ))
    }

    fn from_parts(http: Client, gateway: Url, resident: Url, tokens: Arc<dyn TokenSource>) -> Self {
        Self {
            inner: Arc::new(Inner {
                id: Uuid::new_v4(),
                http,
                gateway,
                resident,
                tokens,
                refresh: Mutex::new(()),
            }),
        }
    }

    pub fn endpoint(&self, base: ProviderBase, path: &str) -> Result<Url, AppError> {
        validate_provider_path(path)?;
        let url = self
            .base(base)
            .join(path.trim_start_matches('/'))
            .map_err(AppError::url)?;
        if !same_origin(&url, self.base(base))
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(AppError::invalid_request("invalid provider path"));
        }
        Ok(url)
    }

    pub fn authorized(
        &self,
        method: Method,
        base: ProviderBase,
        path: &str,
    ) -> Result<PreparedRequest, AppError> {
        self.prepare(method, base, path, Authorization::Authorized)
    }

    /// Prepares an authorized request whose provider-owned identifier is one opaque URL segment.
    ///
    /// `fixed_prefix` remains subject to the strict raw-path validator. `fixed_suffix`, when
    /// present, must itself be one plain fixed segment. The opaque value is encoded by `Url` and
    /// never reinterpreted by raw-path validation.
    pub fn authorized_with_path_segment(
        &self,
        method: Method,
        base: ProviderBase,
        fixed_prefix: &str,
        segment: &str,
        fixed_suffix: Option<&str>,
    ) -> Result<PreparedRequest, AppError> {
        if fixed_prefix.contains('?') || segment.is_empty() || matches!(segment, "." | "..") {
            return Err(AppError::invalid_request("invalid provider path segment"));
        }
        if fixed_suffix.is_some_and(|suffix| !is_fixed_path_segment(suffix)) {
            return Err(AppError::invalid_request("invalid provider path segment"));
        }

        let mut url = self.endpoint(base, fixed_prefix)?;
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| AppError::invalid_request("invalid provider path"))?;
            path.pop_if_empty().push(segment);
            if let Some(suffix) = fixed_suffix {
                path.push(suffix);
            }
        }
        if !same_origin(&url, self.base(base)) || url.query().is_some() || url.fragment().is_some()
        {
            return Err(AppError::invalid_request("invalid provider path"));
        }

        Ok(PreparedRequest {
            owner: self.inner.id,
            method,
            base,
            target: PreparedTarget::ClientBuiltUrl(url),
            authorization: Authorization::Authorized,
            response_accept: ResponseAccept::Json,
            body: PreparedBody::Empty,
        })
    }

    /// Prepares an authorized request containing two provider-owned opaque path segments with a
    /// fixed segment between them. Opaque values are encoded by `Url` and are never interpreted as
    /// raw paths.
    pub fn authorized_with_two_path_segments(
        &self,
        method: Method,
        base: ProviderBase,
        fixed_prefix: &str,
        first_segment: &str,
        fixed_middle: &str,
        second_segment: &str,
    ) -> Result<PreparedRequest, AppError> {
        if fixed_prefix.contains('?')
            || first_segment.is_empty()
            || second_segment.is_empty()
            || matches!(first_segment, "." | "..")
            || matches!(second_segment, "." | "..")
            || !is_fixed_path_segment(fixed_middle)
        {
            return Err(AppError::invalid_request("invalid provider path segment"));
        }

        let mut url = self.endpoint(base, fixed_prefix)?;
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| AppError::invalid_request("invalid provider path"))?;
            path.pop_if_empty()
                .push(first_segment)
                .push(fixed_middle)
                .push(second_segment);
        }
        if !same_origin(&url, self.base(base)) || url.query().is_some() || url.fragment().is_some()
        {
            return Err(AppError::invalid_request("invalid provider path"));
        }

        Ok(PreparedRequest {
            owner: self.inner.id,
            method,
            base,
            target: PreparedTarget::ClientBuiltUrl(url),
            authorization: Authorization::Authorized,
            response_accept: ResponseAccept::Json,
            body: PreparedBody::Empty,
        })
    }

    pub fn anonymous_gateway(
        &self,
        method: Method,
        path: &str,
    ) -> Result<PreparedRequest, AppError> {
        if method != Method::GET || !matches!(path, "/config" | "/feature") {
            return Err(AppError::invalid_request(
                "anonymous provider path is not allowed",
            ));
        }
        self.prepare(
            method,
            ProviderBase::Gateway,
            path,
            Authorization::AnonymousBootstrap,
        )
    }

    pub async fn send_json<T>(&self, request: PreparedRequest) -> Result<T, AppError>
    where
        T: DeserializeOwned,
    {
        let response = self.execute(request).await?;
        let body = read_bounded(response, MAX_JSON_RESPONSE_BYTES).await?;
        serde_json::from_slice(&body).map_err(|_| schema_error())
    }

    pub async fn send_empty(&self, request: PreparedRequest) -> Result<(), AppError> {
        let response = self.execute(request).await?;
        let _ = drain_bounded(response, MAX_EMPTY_BODY_BYTES).await;
        Ok(())
    }

    pub async fn send_stream(&self, request: PreparedRequest) -> Result<Response, AppError> {
        self.execute(request).await
    }

    async fn execute(&self, prepared: PreparedRequest) -> Result<Response, AppError> {
        self.validate_owner(&prepared)?;
        let safe_read = matches!(prepared.method, Method::GET | Method::HEAD);
        let is_authorized = prepared.authorization == Authorization::Authorized;
        let (response, failed_authorization) =
            self.dispatch(&prepared, DispatchKind::Initial).await?;
        if response.status() != StatusCode::UNAUTHORIZED || !safe_read || !is_authorized {
            return classify(response, false).await;
        }

        drop(response);
        let refresh_guard = self.inner.refresh.lock().await;
        if self
            .authorization_is_current(failed_authorization.as_ref())
            .await?
        {
            self.inner.tokens.refresh_after_unauthorized().await?;
        }
        drop(refresh_guard);
        let (response, _) = self
            .dispatch(&prepared, DispatchKind::RetryAfterUnauthorized)
            .await?;
        classify(response, true).await
    }

    fn prepare(
        &self,
        method: Method,
        base: ProviderBase,
        path: &str,
        authorization: Authorization,
    ) -> Result<PreparedRequest, AppError> {
        let _ = self.endpoint(base, path)?;
        Ok(PreparedRequest {
            owner: self.inner.id,
            method,
            base,
            target: PreparedTarget::ValidatedRawPath(path.to_owned()),
            authorization,
            response_accept: ResponseAccept::Json,
            body: PreparedBody::Empty,
        })
    }

    async fn dispatch(
        &self,
        prepared: &PreparedRequest,
        kind: DispatchKind,
    ) -> Result<(Response, Option<HeaderValue>), AppError> {
        self.validate_owner(prepared)?;
        let url = match &prepared.target {
            PreparedTarget::ValidatedRawPath(path) => self.endpoint(prepared.base, path)?,
            PreparedTarget::ClientBuiltUrl(url) => url.clone(),
        };
        let mut builder = self.inner.http.request(prepared.method.clone(), url);
        match &prepared.body {
            PreparedBody::Empty => {}
            PreparedBody::ExplicitEmpty => {
                builder = builder
                    .header(CONTENT_LENGTH, HeaderValue::from_static("0"))
                    .body(Vec::new());
            }
            PreparedBody::Json(body) => {
                builder = builder
                    .header(CONTENT_TYPE, "application/json")
                    .body(body.clone());
            }
        }

        let mut dispatched_authorization = None;
        if prepared.authorization == Authorization::Authorized {
            let access = self
                .inner
                .tokens
                .access_token()
                .await
                .ok_or_else(session_expired)?;
            let authorization = bearer_header(&access)?;
            builder = builder.header(AUTHORIZATION, authorization.clone());
            dispatched_authorization = Some(authorization);
            if kind == DispatchKind::RetryAfterUnauthorized {
                builder = retry_compatibility_header(builder, &access)?;
            }
            if let Some(device) = self.inner.tokens.device_token().await {
                builder = builder.header(DEVICE_TOKEN, secret_header(&device)?);
            }
        }

        let request = common_headers(builder, prepared.response_accept)
            .build()
            .map_err(request_error)?;
        self.validate_request(&request)?;
        let response = self
            .inner
            .http
            .execute(request)
            .await
            .map_err(transport_error)?;
        Ok((response, dispatched_authorization))
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

    fn validate_owner(&self, prepared: &PreparedRequest) -> Result<(), AppError> {
        if prepared.owner != self.inner.id {
            return Err(AppError::invalid_request(
                "provider request belongs to another client",
            ));
        }
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

fn provider_http_client() -> Result<Client, AppError> {
    Client::builder()
        .retry(reqwest::retry::never())
        .redirect(Policy::none())
        .cookie_store(false)
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(transport_error)
}

fn common_headers(request: RequestBuilder, response_accept: ResponseAccept) -> RequestBuilder {
    request
        .header(DEVICE_TYPE, "ANDROID")
        .header(REQUEST_ID, Uuid::new_v4().to_string())
        .header(ACCEPT, response_accept.as_header_value())
}

fn retry_compatibility_header(
    request: RequestBuilder,
    token: &SecretString,
) -> Result<RequestBuilder, AppError> {
    Ok(request.header(X_AUTH_TOKEN, secret_header(token)?))
}

fn validate_provider_path(path: &str) -> Result<(), AppError> {
    if !path.starts_with('/') || path.starts_with("//") || path.contains(['\\', '#']) {
        return Err(AppError::invalid_request("invalid provider path"));
    }

    let path_only = path.split_once('?').map_or(path, |(path, _)| path);
    for segment in path_only.split('/').skip(1) {
        let decoded = repeatedly_percent_decode(segment.as_bytes())?;
        if decoded == b"."
            || decoded == b".."
            || decoded.contains(&b'/')
            || decoded.contains(&b'\\')
            || decoded.iter().any(|byte| byte.is_ascii_control())
        {
            return Err(AppError::invalid_request("invalid provider path"));
        }
    }
    Ok(())
}

fn is_fixed_path_segment(segment: &str) -> bool {
    !segment.is_empty()
        && !matches!(segment, "." | "..")
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn repeatedly_percent_decode(segment: &[u8]) -> Result<Vec<u8>, AppError> {
    let mut decoded = segment.to_vec();
    for _ in 0..4 {
        if !decoded.contains(&b'%') {
            return Ok(decoded);
        }
        decoded = percent_decode_once(&decoded)?;
    }
    if decoded.contains(&b'%') {
        return Err(AppError::invalid_request("invalid provider path"));
    }
    Ok(decoded)
}

fn percent_decode_once(input: &[u8]) -> Result<Vec<u8>, AppError> {
    let mut decoded = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        if input[index] != b'%' {
            decoded.push(input[index]);
            index += 1;
            continue;
        }
        if index + 2 >= input.len() {
            return Err(AppError::invalid_request("invalid provider path"));
        }
        let high = hex_value(input[index + 1])?;
        let low = hex_value(input[index + 2])?;
        decoded.push((high << 4) | low);
        index += 3;
    }
    Ok(decoded)
}

fn hex_value(value: u8) -> Result<u8, AppError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(AppError::invalid_request("invalid provider path")),
    }
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

fn invalid_body() -> AppError {
    AppError::invalid_request("provider request body could not be encoded")
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
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?;
    let seconds = value.parse().ok().or_else(|| {
        let deadline = httpdate::parse_http_date(value).ok()?;
        Some(
            deadline
                .duration_since(SystemTime::now())
                .unwrap_or(Duration::ZERO)
                .as_secs(),
        )
    })?;
    Some(seconds.min(MAX_RETRY_AFTER_SECONDS))
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
    let _ = drain_stream_bounded(response.bytes_stream(), limit).await?;
    Ok(())
}

async fn drain_stream_bounded<S>(stream: S, limit: usize) -> Result<usize, AppError>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>>,
{
    pin_mut!(stream);
    let mut consumed = 0_usize;
    while consumed < limit {
        let Some(chunk) = stream.next().await else {
            break;
        };
        let chunk = chunk.map_err(transport_error)?;
        let remaining = limit - consumed;
        consumed += chunk.len().min(remaining);
        if chunk.len() >= remaining {
            break;
        }
    }
    Ok(consumed)
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

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        task::Poll,
    };

    use bytes::Bytes;
    use futures_util::stream;
    use reqwest::Client;
    use secrecy::SecretString;

    use super::{X_AUTH_TOKEN, drain_stream_bounded, retry_compatibility_header};

    #[test]
    fn retry_compatibility_header_is_sensitive_and_redacted_from_debug() {
        let token = SecretString::from("retry-secret-marker");
        let request = retry_compatibility_header(Client::new().get("http://127.0.0.1/"), &token)
            .unwrap()
            .build()
            .unwrap();
        let header = &request.headers()[X_AUTH_TOKEN];

        assert_eq!(header, "retry-secret-marker");
        assert!(header.is_sensitive());
        assert!(!format!("{:?}", request.headers()).contains("retry-secret-marker"));
    }

    #[tokio::test]
    async fn bounded_error_reader_stops_polling_once_the_limit_is_reached() {
        let polls = Arc::new(AtomicUsize::new(0));
        let observed_polls = Arc::clone(&polls);
        let mut chunks = VecDeque::from([
            Bytes::from_static(b"1234"),
            Bytes::from_static(b"5678"),
            Bytes::from_static(b"must-not-be-polled"),
        ]);
        let body = stream::poll_fn(move |_| {
            observed_polls.fetch_add(1, Ordering::SeqCst);
            Poll::Ready(chunks.pop_front().map(Ok::<_, reqwest::Error>))
        });

        let consumed = drain_stream_bounded(body, 5).await.unwrap();

        assert_eq!(consumed, 5);
        assert_eq!(polls.load(Ordering::SeqCst), 2);
    }
}
