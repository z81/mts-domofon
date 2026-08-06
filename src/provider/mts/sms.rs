use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex},
    time::Duration,
};

use futures_util::StreamExt;
use reqwest::{Client, Method, Response, StatusCode, Url, redirect::Policy};
use secrecy::{ExposeSecret, SecretString};
use subtle::ConstantTimeEq;
use tokio::time::Instant;
use zeroize::Zeroizing;

use crate::{
    domain::auth::{AuthChallenge, AuthFlowId, AuthProgress},
    error::{AppError, ErrorCode},
};

const LEGACY_AUTH_ORIGIN: &str = "https://resident-rest.vdome.mts.ru/";
const INIT_PATH: &str = "user-service/api/v2/auth/init";
const LOGIN_PATH: &str = "user-service/api/v2/auth/login";
const MAX_BODY_LEN: usize = 64 * 1024;
const MAX_FLOWS: usize = 5;
const FLOW_LIFETIME: Duration = Duration::from_secs(10 * 60);
const RESEND_AFTER: Duration = Duration::from_secs(60);
const MAX_OTP_ATTEMPTS: u8 = 3;

#[derive(Clone)]
pub struct MtsLegacySmsAuth {
    inner: Arc<Inner>,
}

struct Inner {
    http: Client,
    origin: Url,
    flows: Mutex<HashMap<AuthFlowId, SmsFlow>>,
}

struct SmsFlow {
    phone: SecretString,
    created_at: Instant,
    expires_at: Instant,
    attempts: u8,
    in_flight: bool,
}

pub struct LegacyTokenPair {
    pub access_token: SecretString,
    pub refresh_token: SecretString,
}

#[cfg(feature = "test-support")]
pub struct LegacySmsTestConfig {
    origin: Url,
}

#[cfg(feature = "test-support")]
impl LegacySmsTestConfig {
    pub fn new(origin: Url) -> Self {
        Self { origin }
    }
}

impl MtsLegacySmsAuth {
    pub fn new() -> Result<Self, AppError> {
        Self::from_origin(Url::parse(LEGACY_AUTH_ORIGIN).expect("legacy MTS auth origin is valid"))
    }

    #[cfg(feature = "test-support")]
    pub fn new_for_test(config: LegacySmsTestConfig) -> Result<Self, AppError> {
        Self::from_origin(config.origin)
    }

    fn from_origin(origin: Url) -> Result<Self, AppError> {
        if origin.host_str().is_none()
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.path() != "/"
            || origin.query().is_some()
            || origin.fragment().is_some()
        {
            return Err(AppError::invalid_request("invalid legacy MTS auth origin"));
        }
        #[cfg(not(feature = "test-support"))]
        if origin.as_str() != LEGACY_AUTH_ORIGIN {
            return Err(AppError::invalid_request(
                "unapproved legacy MTS auth origin",
            ));
        }
        let http = Client::builder()
            .redirect(Policy::none())
            .cookie_store(false)
            .timeout(Duration::from_secs(45))
            .build()
            .map_err(transport_error)?;
        Ok(Self {
            inner: Arc::new(Inner {
                http,
                origin,
                flows: Mutex::new(HashMap::new()),
            }),
        })
    }

    pub async fn start(&self, phone: SecretString) -> Result<AuthProgress, AppError> {
        validate_phone(&phone)?;
        let now = Instant::now();
        let flow_id = {
            let mut flows = self.inner.flows.lock().map_err(|_| state_error())?;
            flows.retain(|_, flow| flow.expires_at > now);
            if let Some((existing_id, existing)) = flows
                .iter()
                .find(|(_, flow)| secret_eq(&flow.phone, &phone))
            {
                let retry_after = existing.created_at + RESEND_AFTER;
                if existing.in_flight || retry_after > now {
                    return Err(rate_limited(retry_after.saturating_duration_since(now)));
                }
                let existing_id = existing_id.clone();
                flows.remove(&existing_id);
            }
            if flows.len() >= MAX_FLOWS {
                return Err(rate_limited(RESEND_AFTER));
            }
            let flow_id = AuthFlowId::fresh();
            flows.insert(
                flow_id.clone(),
                SmsFlow {
                    phone: phone.clone(),
                    created_at: now,
                    expires_at: now + FLOW_LIFETIME,
                    attempts: 0,
                    in_flight: true,
                },
            );
            flow_id
        };

        let result = self.init(&phone).await;
        let mut flows = self.inner.flows.lock().map_err(|_| state_error())?;
        if let Some(flow) = flows.get_mut(&flow_id) {
            flow.in_flight = false;
        }
        result?;
        Ok(AuthProgress::Challenge {
            flow_id,
            challenge: AuthChallenge::SmsOtp {
                resend_after_seconds: RESEND_AFTER.as_secs(),
            },
        })
    }

    pub async fn confirm(
        &self,
        flow_id: &AuthFlowId,
        code: SecretString,
    ) -> Result<LegacyTokenPair, AppError> {
        validate_code(&code)?;
        let phone = {
            let now = Instant::now();
            let mut flows = self.inner.flows.lock().map_err(|_| state_error())?;
            let Some(flow) = flows.get_mut(flow_id) else {
                return Err(flow_error());
            };
            if flow.expires_at <= now || flow.in_flight || flow.attempts >= MAX_OTP_ATTEMPTS {
                flows.remove(flow_id);
                return Err(flow_error());
            }
            flow.in_flight = true;
            flow.phone.clone()
        };

        let result = self.login(&phone, &code).await;
        let mut flows = self.inner.flows.lock().map_err(|_| state_error())?;
        match result {
            Ok(tokens) => {
                flows.remove(flow_id);
                Ok(tokens)
            }
            Err(error) => {
                if let Some(flow) = flows.get_mut(flow_id) {
                    flow.in_flight = false;
                    if error.code == ErrorCode::UpstreamAuthenticationFailed {
                        flow.attempts = flow.attempts.saturating_add(1);
                        if flow.attempts >= MAX_OTP_ATTEMPTS {
                            flows.remove(flow_id);
                        }
                    }
                }
                Err(error)
            }
        }
    }

    fn endpoint(&self, path: &str) -> Result<Url, AppError> {
        self.inner.origin.join(path).map_err(AppError::url)
    }

    async fn init(&self, phone: &SecretString) -> Result<(), AppError> {
        let response: CommonResponse<InitResponse> = self
            .send_json(
                INIT_PATH,
                &PhoneRequest {
                    phone: phone.expose_secret(),
                },
            )
            .await?;
        let result = response.result.ok_or_else(authentication_error)?;
        result.validate()
    }

    async fn login(
        &self,
        phone: &SecretString,
        code: &SecretString,
    ) -> Result<LegacyTokenPair, AppError> {
        let response: CommonResponse<LoginResponse> = self
            .send_json(
                LOGIN_PATH,
                &LoginRequest {
                    phone: phone.expose_secret(),
                    code: code.expose_secret(),
                },
            )
            .await?;
        response
            .result
            .ok_or_else(authentication_error)?
            .into_tokens()
    }

    async fn send_json<T, B>(&self, path: &str, body: &B) -> Result<T, AppError>
    where
        T: serde::de::DeserializeOwned,
        B: serde::Serialize + ?Sized,
    {
        let response = self
            .inner
            .http
            .request(Method::POST, self.endpoint(path)?)
            .header(reqwest::header::ACCEPT, "application/json")
            .json(body)
            .send()
            .await
            .map_err(transport_error)?;
        let response = classify(response).await?;
        let bytes = read_bounded(response).await?;
        serde_json::from_slice(&bytes).map_err(|_| schema_error())
    }
}

impl fmt::Debug for MtsLegacySmsAuth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MtsLegacySmsAuth")
            .field(
                "active_flows",
                &self
                    .inner
                    .flows
                    .lock()
                    .map(|flows| flows.len())
                    .unwrap_or(0),
            )
            .field("origin", &self.inner.origin.origin().ascii_serialization())
            .finish_non_exhaustive()
    }
}

#[derive(serde::Serialize)]
struct PhoneRequest<'a> {
    phone: &'a str,
}

#[derive(serde::Serialize)]
struct LoginRequest<'a> {
    phone: &'a str,
    code: &'a str,
}

#[derive(serde::Deserialize)]
struct CommonResponse<T> {
    #[serde(alias = "Result")]
    result: Option<T>,
    #[serde(default, alias = "Message")]
    _message: Option<Zeroizing<String>>,
    #[serde(default, alias = "Errors")]
    _errors: Option<serde_json::Value>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitResponse {
    status: Zeroizing<String>,
    can_skip_sms: bool,
    refresh_token: InitRefreshToken,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitRefreshToken {
    token: SecretString,
    expired_at: i64,
    time_zone: Zeroizing<String>,
}

impl InitResponse {
    fn validate(self) -> Result<(), AppError> {
        if self.status.is_empty()
            || self.refresh_token.token.expose_secret().is_empty()
            || self.refresh_token.time_zone.is_empty()
        {
            return Err(schema_error());
        }
        let _ = (self.can_skip_sms, self.refresh_token.expired_at);
        Ok(())
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginResponse {
    access_token: SecretString,
    refresh_token: SecretString,
}

impl LoginResponse {
    fn into_tokens(self) -> Result<LegacyTokenPair, AppError> {
        if self.access_token.expose_secret().is_empty()
            || self.refresh_token.expose_secret().is_empty()
        {
            return Err(schema_error());
        }
        Ok(LegacyTokenPair {
            access_token: self.access_token,
            refresh_token: self.refresh_token,
        })
    }
}

async fn classify(response: Response) -> Result<Response, AppError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    let _ = read_bounded(response).await;
    let mut error = match status {
        StatusCode::TOO_MANY_REQUESTS => {
            rate_limited(Duration::from_secs(retry_after.unwrap_or(60)))
        }
        status if status.is_client_error() => authentication_error(),
        _ => AppError::new(
            ErrorCode::UpstreamUnavailable,
            "MTS legacy authentication service is unavailable.",
            true,
        ),
    };
    if error.code == ErrorCode::RateLimited {
        error.retry_after_seconds = retry_after.or(Some(60));
    }
    Err(error)
}

async fn read_bounded(response: Response) -> Result<Zeroizing<Vec<u8>>, AppError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BODY_LEN as u64)
    {
        return Err(schema_error());
    }
    let mut body = Zeroizing::new(Vec::new());
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(transport_error)?;
        if body
            .len()
            .checked_add(chunk.len())
            .is_none_or(|length| length > MAX_BODY_LEN)
        {
            return Err(schema_error());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn validate_phone(phone: &SecretString) -> Result<(), AppError> {
    let value = phone.expose_secret().as_bytes();
    if value.len() == 12 && value.starts_with(b"+7") && value[2..].iter().all(u8::is_ascii_digit) {
        Ok(())
    } else {
        Err(AppError::invalid_request(
            "Phone must use +7XXXXXXXXXX format.",
        ))
    }
}

fn validate_code(code: &SecretString) -> Result<(), AppError> {
    let value = code.expose_secret().as_bytes();
    if value.len() == 6 && value.iter().all(u8::is_ascii_digit) {
        Ok(())
    } else {
        Err(AppError::invalid_request(
            "SMS code must contain exactly six digits.",
        ))
    }
}

fn secret_eq(left: &SecretString, right: &SecretString) -> bool {
    left.expose_secret()
        .as_bytes()
        .ct_eq(right.expose_secret().as_bytes())
        .into()
}

fn rate_limited(after: Duration) -> AppError {
    let mut error = AppError::new(
        ErrorCode::RateLimited,
        "Legacy SMS authentication is rate limited.",
        false,
    );
    error.retry_after_seconds = Some(after.as_secs().max(1));
    error
}

fn flow_error() -> AppError {
    AppError::new(
        ErrorCode::AuthFlowNotFound,
        "Legacy SMS authentication flow was not found.",
        false,
    )
}

fn authentication_error() -> AppError {
    AppError::new(
        ErrorCode::UpstreamAuthenticationFailed,
        "MTS legacy authentication failed.",
        false,
    )
}

fn schema_error() -> AppError {
    AppError::new(
        ErrorCode::UpstreamSchemaChanged,
        "MTS legacy authentication response was invalid.",
        false,
    )
}

fn transport_error(_error: reqwest::Error) -> AppError {
    AppError::new(
        ErrorCode::UpstreamUnavailable,
        "MTS legacy authentication service is unavailable.",
        true,
    )
}

fn state_error() -> AppError {
    AppError::new(
        ErrorCode::Internal,
        "Legacy SMS authentication state is unavailable.",
        false,
    )
}
