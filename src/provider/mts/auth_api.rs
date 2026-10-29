use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::{
    Client, Method, RequestBuilder, Response, StatusCode,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue, RETRY_AFTER},
    redirect::Policy,
};
use secrecy::{ExposeSecret, SecretString};
use url::Url;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{
    config::AppConfig,
    domain::auth::AuthFlavor,
    error::{AppError, ErrorCode},
};

const KEYCLOAK_REFRESH_PATH: &str = "/user-service/api/v2/auth/refresh";
const WEB_SSO_REFRESH_PATH: &str = "/user-service/api/auth/websso/refresh";
const LOGOUT_PATH: &str = "/user-service/api/v2/auth/logout";
const MAX_RESPONSE_BODY_LEN: usize = 64 * 1024;
const MAX_RETRY_AFTER_SECONDS: u64 = 60 * 60;
const DEVICE_TOKEN_HEADER: &str = "x-device-token";
const DEVICE_TYPE_HEADER: &str = "x-device-type";
const REQUEST_ID_HEADER: &str = "request-id";

pub struct KeycloakRotation {
    pub access_token: SecretString,
    pub refresh_token: SecretString,
}

#[async_trait]
pub trait ProviderAuthApi: Send + Sync {
    async fn rotate_keycloak(
        &self,
        refresh_token: &SecretString,
        device_token: Option<&SecretString>,
    ) -> Result<KeycloakRotation, AppError>;

    async fn rotate_web_sso(
        &self,
        access_token: &SecretString,
        device_token: Option<&SecretString>,
    ) -> Result<SecretString, AppError>;

    async fn logout(
        &self,
        refresh_token: &SecretString,
        device_token: Option<&SecretString>,
        auth_flavor: AuthFlavor,
    ) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct MtsAuthApi {
    inner: Arc<Inner>,
}

struct Inner {
    http: Client,
    gateway: Url,
    resident: Url,
}

impl MtsAuthApi {
    pub fn new(config: &AppConfig) -> Result<Self, AppError> {
        Self::from_origins(config.gateway.clone(), config.resident.clone())
    }

    #[cfg(feature = "test-support")]
    pub fn new_for_test(gateway: Url) -> Result<Self, AppError> {
        Self::new_for_test_with_origins(gateway.clone(), gateway)
    }

    #[cfg(feature = "test-support")]
    pub fn new_for_test_with_origins(gateway: Url, resident: Url) -> Result<Self, AppError> {
        validate_origin(&gateway, "invalid MTS test gateway")?;
        validate_origin(&resident, "invalid MTS test resident origin")?;
        Self::from_origins(gateway, resident)
    }

    fn from_origins(gateway: Url, resident: Url) -> Result<Self, AppError> {
        let http = Client::builder()
            .redirect(Policy::none())
            .retry(reqwest::retry::never())
            .cookie_store(false)
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(transport_error)?;
        Ok(Self {
            inner: Arc::new(Inner {
                http,
                gateway,
                resident,
            }),
        })
    }

    fn gateway_request(&self, method: Method, path: &str) -> Result<RequestBuilder, AppError> {
        let endpoint = endpoint(&self.inner.gateway, path)?;
        Ok(self
            .inner
            .http
            .request(method, endpoint)
            .header(DEVICE_TYPE_HEADER, "ANDROID")
            .header(REQUEST_ID_HEADER, Uuid::new_v4().to_string())
            .header(ACCEPT, "application/json"))
    }

    fn legacy_request(&self, method: Method, path: &str) -> Result<RequestBuilder, AppError> {
        Ok(self
            .inner
            .http
            .request(method, endpoint(&self.inner.resident, path)?))
    }
}

#[async_trait]
impl ProviderAuthApi for MtsAuthApi {
    async fn rotate_keycloak(
        &self,
        refresh_token: &SecretString,
        device_token: Option<&SecretString>,
    ) -> Result<KeycloakRotation, AppError> {
        let body = RefreshRequest {
            refresh_token: refresh_token.expose_secret(),
        };
        let mut request = self
            .gateway_request(Method::PUT, KEYCLOAK_REFRESH_PATH)?
            .header(CONTENT_TYPE, "application/json")
            .json(&body);
        if let Some(device_token) = device_token {
            request = request.header(DEVICE_TOKEN_HEADER, secret_header(device_token)?);
        }
        send("keycloak_refresh", request, |body| {
            decode::<KeycloakRefreshResponse>(body)?.into_rotation()
        })
        .await
    }

    async fn rotate_web_sso(
        &self,
        access_token: &SecretString,
        device_token: Option<&SecretString>,
    ) -> Result<SecretString, AppError> {
        let mut request = self
            .gateway_request(Method::PUT, WEB_SSO_REFRESH_PATH)?
            .header(AUTHORIZATION, bearer_header(access_token)?)
            .body(Vec::new());
        if let Some(device_token) = device_token {
            request = request.header(DEVICE_TOKEN_HEADER, secret_header(device_token)?);
        }
        send("web_sso_refresh", request, |body| {
            decode::<WebSsoRefreshResponse>(body)?.into_token()
        })
        .await
    }

    async fn logout(
        &self,
        refresh_token: &SecretString,
        device_token: Option<&SecretString>,
        auth_flavor: AuthFlavor,
    ) -> Result<(), AppError> {
        let body = RefreshRequest {
            refresh_token: refresh_token.expose_secret(),
        };
        let request = match auth_flavor {
            AuthFlavor::WebSso => {
                let mut request = self
                    .gateway_request(Method::POST, LOGOUT_PATH)?
                    .header(CONTENT_TYPE, "application/json")
                    .json(&body);
                if let Some(device_token) = device_token {
                    request = request.header(DEVICE_TOKEN_HEADER, secret_header(device_token)?);
                }
                request
            }
            AuthFlavor::LegacySms => self
                .legacy_request(Method::POST, LOGOUT_PATH)?
                .header(CONTENT_TYPE, "application/json")
                .json(&body),
        };
        send("logout", request, |_| Ok(())).await
    }
}

#[cfg(feature = "test-support")]
fn validate_origin(origin: &Url, message: &'static str) -> Result<(), AppError> {
    if origin.host_str().is_none()
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
    {
        Err(AppError::invalid_request(message))
    } else {
        Ok(())
    }
}

fn endpoint(origin: &Url, path: &str) -> Result<Url, AppError> {
    let endpoint = origin
        .join(path.trim_start_matches('/'))
        .map_err(AppError::url)?;
    if !same_origin(&endpoint, origin)
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(AppError::invalid_request("invalid MTS auth endpoint"));
    }
    Ok(endpoint)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RefreshRequest<'token> {
    refresh_token: &'token str,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeycloakRefreshResponse {
    access_token: SecretString,
    refresh_token: SecretString,
}

impl KeycloakRefreshResponse {
    fn into_rotation(self) -> Result<KeycloakRotation, AppError> {
        if self.access_token.expose_secret().is_empty()
            || self.refresh_token.expose_secret().is_empty()
        {
            return Err(schema_error());
        }
        Ok(KeycloakRotation {
            access_token: self.access_token,
            refresh_token: self.refresh_token,
        })
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebSsoRefreshResponse {
    access_token: SecretString,
}

impl WebSsoRefreshResponse {
    fn into_token(self) -> Result<SecretString, AppError> {
        if self.access_token.expose_secret().is_empty() {
            return Err(schema_error());
        }
        Ok(self.access_token)
    }
}

async fn send<T>(
    operation: &'static str,
    request: RequestBuilder,
    parse: impl FnOnce(&[u8]) -> Result<T, AppError>,
) -> Result<T, AppError> {
    let response = request.send().await.map_err(|error| {
        tracing::warn!(
            operation,
            is_timeout = error.is_timeout(),
            is_connect = error.is_connect(),
            "MTS authentication request failed"
        );
        transport_error(error)
    })?;
    let status = response.status();
    let result = match classify(response).await {
        Ok(response) => read_bounded(response).await.and_then(|body| parse(&body)),
        Err(error) => Err(error),
    };
    if let Err(error) = &result {
        tracing::warn!(
            operation,
            status = status.as_u16(),
            code = ?error.code,
            "MTS authentication request failed"
        );
    }
    result
}

fn decode<T>(body: &[u8]) -> Result<T, AppError>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_slice(body).map_err(|_| schema_error())
}

async fn classify(response: Response) -> Result<Response, AppError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let error = match status {
        StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            AppError::new(
                ErrorCode::UpstreamAuthenticationFailed,
                "MTS authentication operation failed.",
                false,
            )
        }
        StatusCode::TOO_MANY_REQUESTS => {
            let mut error = AppError::new(
                ErrorCode::RateLimited,
                "MTS authentication rate limit was reached.",
                true,
            );
            error.retry_after_seconds = retry_after(response.headers());
            error
        }
        status if status.is_server_error() => AppError::new(
            ErrorCode::UpstreamUnavailable,
            "MTS authentication service is unavailable.",
            true,
        ),
        _ => AppError::new(
            ErrorCode::UpstreamUnavailable,
            "MTS authentication service returned an unexpected response.",
            false,
        ),
    };
    let _ = read_bounded(response).await;
    Err(error)
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

async fn read_bounded(response: Response) -> Result<Zeroizing<Vec<u8>>, AppError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BODY_LEN as u64)
    {
        return Err(schema_error());
    }
    let mut body = Zeroizing::new(Vec::new());
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(transport_error)?;
        let next_len = body
            .len()
            .checked_add(chunk.len())
            .ok_or_else(schema_error)?;
        if next_len > MAX_RESPONSE_BODY_LEN {
            return Err(schema_error());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn bearer_header(token: &SecretString) -> Result<HeaderValue, AppError> {
    let mut encoded = Zeroizing::new(Vec::with_capacity(7 + token.expose_secret().len()));
    encoded.extend_from_slice(b"Bearer ");
    encoded.extend_from_slice(token.expose_secret().as_bytes());
    let mut value = HeaderValue::from_bytes(&encoded).map_err(|_| invalid_credential())?;
    value.set_sensitive(true);
    Ok(value)
}

fn secret_header(token: &SecretString) -> Result<HeaderValue, AppError> {
    let mut value = HeaderValue::from_bytes(token.expose_secret().as_bytes())
        .map_err(|_| invalid_credential())?;
    value.set_sensitive(true);
    Ok(value)
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn invalid_credential() -> AppError {
    AppError::new(
        ErrorCode::Internal,
        "MTS credential could not be encoded.",
        false,
    )
}

fn transport_error(_error: reqwest::Error) -> AppError {
    AppError::new(
        ErrorCode::UpstreamUnavailable,
        "MTS authentication service is unavailable.",
        true,
    )
}

fn schema_error() -> AppError {
    AppError::new(
        ErrorCode::UpstreamSchemaChanged,
        "MTS authentication response was invalid.",
        false,
    )
}
