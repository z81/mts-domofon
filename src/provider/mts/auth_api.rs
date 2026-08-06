use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::{
    Client, Method, RequestBuilder, Response, StatusCode,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderValue},
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
        let response: KeycloakRefreshResponse = send_json(request).await?;
        response.into_rotation()
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
        let response: WebSsoRefreshResponse = send_json(request).await?;
        response.into_token()
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
        let response = match auth_flavor {
            AuthFlavor::WebSso => {
                let mut request = self
                    .gateway_request(Method::POST, LOGOUT_PATH)?
                    .header(CONTENT_TYPE, "application/json")
                    .json(&body);
                if let Some(device_token) = device_token {
                    request = request.header(DEVICE_TOKEN_HEADER, secret_header(device_token)?);
                }
                request.send().await
            }
            AuthFlavor::LegacySms => {
                self.legacy_request(Method::POST, LOGOUT_PATH)?
                    .header(CONTENT_TYPE, "application/json")
                    .json(&body)
                    .send()
                    .await
            }
        }
        .map_err(transport_error)?;
        let response = classify(response).await?;
        let _ = read_bounded(response).await?;
        Ok(())
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

async fn send_json<T>(request: RequestBuilder) -> Result<T, AppError>
where
    T: serde::de::DeserializeOwned,
{
    let response = request.send().await.map_err(transport_error)?;
    let response = classify(response).await?;
    let body = read_bounded(response).await?;
    serde_json::from_slice(&body).map_err(|_| schema_error())
}

async fn classify(response: Response) -> Result<Response, AppError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let _ = read_bounded(response).await;
    Err(match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => AppError::new(
            ErrorCode::UpstreamAuthenticationFailed,
            "MTS authentication operation failed.",
            false,
        ),
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
    })
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
