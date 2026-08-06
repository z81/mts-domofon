use std::{
    collections::HashMap,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::StreamExt;
use http::{HeaderMap, HeaderValue};
use rand::RngCore;
use reqwest::{Client, cookie::Jar, redirect::Policy};
use secrecy::{ExposeSecret, SecretString};
use subtle::ConstantTimeEq;
use tokio::time::Instant;
use url::Url;
use zeroize::Zeroizing;

use crate::{
    domain::auth::{AuthFlowId, AuthProgress, ChallengeInput, TokenBundle},
    error::{AppError, ErrorCode},
};

const AUTHORIZE_URL: &str = "https://login.mts.ru/amserver/oauth2/authorize";
const CALLBACK_URL: &str = "https://gateway.vdome.mts.ru/user-service/api/auth/callback";
#[cfg(feature = "test-support")]
const CALLBACK_PATH: &str = "/user-service/api/auth/callback";
const SCOPE: &str = "account address sso profile ssom openid phone";
const DEFAULT_MAX_FLOWS: usize = 5;
const DEFAULT_FLOW_LIFETIME: Duration = Duration::from_secs(10 * 60);
const DEFAULT_MAX_CALLBACK_URL_LEN: usize = 4 * 1024;
const DEFAULT_MAX_CALLBACK_BODY_LEN: usize = 64 * 1024;

pub struct MtsSsoTransport {
    device_id: String,
    flows: Mutex<HashMap<AuthFlowId, BrowserFlowState>>,
    max_flows: usize,
    flow_lifetime: Duration,
    max_callback_url_len: usize,
    max_callback_body_len: usize,
    callback_target: CallbackTarget,
    next_flow_sequence: AtomicU64,
}

struct BrowserFlow {
    request: SsoRequest,
    metadata: FlowMetadata,
    client: Client,
    _cookie_jar: Arc<Jar>,
}

enum BrowserFlowState {
    Pending(BrowserFlow),
    InFlight(FlowMetadata),
}

#[derive(Clone, Copy)]
struct FlowMetadata {
    created_sequence: u64,
    expires_at: Instant,
}

impl BrowserFlowState {
    fn metadata(&self) -> FlowMetadata {
        match self {
            Self::Pending(flow) => flow.metadata,
            Self::InFlight(metadata) => *metadata,
        }
    }

    fn is_pending(&self) -> bool {
        matches!(self, Self::Pending(_))
    }
}

struct InFlightGuard<'transport> {
    flows: &'transport Mutex<HashMap<AuthFlowId, BrowserFlowState>>,
    flow_id: AuthFlowId,
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut flows) = self.flows.lock()
            && matches!(
                flows.get(&self.flow_id),
                Some(BrowserFlowState::InFlight(_))
            )
        {
            flows.remove(&self.flow_id);
        }
    }
}

#[derive(Clone)]
struct CallbackTarget {
    url: Url,
}

#[cfg(feature = "test-support")]
pub struct MtsSsoTestConfig {
    pub callback_url: Url,
    pub max_flows: usize,
    pub flow_lifetime: Duration,
    pub max_callback_url_len: usize,
    pub max_callback_body_len: usize,
}

#[cfg(feature = "test-support")]
impl MtsSsoTestConfig {
    pub fn new(callback_url: Url) -> Self {
        Self {
            callback_url,
            max_flows: DEFAULT_MAX_FLOWS,
            flow_lifetime: DEFAULT_FLOW_LIFETIME,
            max_callback_url_len: DEFAULT_MAX_CALLBACK_URL_LEN,
            max_callback_body_len: DEFAULT_MAX_CALLBACK_BODY_LEN,
        }
    }
}

impl fmt::Debug for MtsSsoTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let active_flows = self.flows.lock().map(|flows| flows.len()).unwrap_or(0);
        formatter
            .debug_struct("MtsSsoTransport")
            .field("active_flows", &active_flows)
            .field("max_flows", &self.max_flows)
            .field("flow_lifetime", &self.flow_lifetime)
            .field("callback_target", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl MtsSsoTransport {
    pub fn new(device_id: impl Into<String>) -> Self {
        Self {
            device_id: device_id.into(),
            flows: Mutex::new(HashMap::new()),
            max_flows: DEFAULT_MAX_FLOWS,
            flow_lifetime: DEFAULT_FLOW_LIFETIME,
            max_callback_url_len: DEFAULT_MAX_CALLBACK_URL_LEN,
            max_callback_body_len: DEFAULT_MAX_CALLBACK_BODY_LEN,
            callback_target: CallbackTarget {
                url: Url::parse(CALLBACK_URL).expect("MTS callback URL is valid"),
            },
            next_flow_sequence: AtomicU64::new(0),
        }
    }

    #[cfg(feature = "test-support")]
    pub fn new_for_test(
        device_id: impl Into<String>,
        config: MtsSsoTestConfig,
    ) -> Result<Self, AppError> {
        validate_test_callback_target(&config.callback_url)?;
        if config.max_flows == 0
            || config.max_callback_url_len == 0
            || config.max_callback_body_len == 0
        {
            return Err(AppError::invalid_request(
                "invalid MTS test transport limits",
            ));
        }
        Ok(Self {
            device_id: device_id.into(),
            flows: Mutex::new(HashMap::new()),
            max_flows: config.max_flows,
            flow_lifetime: config.flow_lifetime,
            max_callback_url_len: config.max_callback_url_len,
            max_callback_body_len: config.max_callback_body_len,
            callback_target: CallbackTarget {
                url: config.callback_url,
            },
            next_flow_sequence: AtomicU64::new(0),
        })
    }

    pub async fn begin(&self) -> Result<AuthProgress, AppError> {
        let request = SsoRequest::with_fresh_state(self.device_id.clone());
        let authorization_url = request.authorize_url();
        if authorization_url.as_str().len() > self.max_callback_url_len {
            return Err(AppError::invalid_request(
                "MTS authorization URL is too long",
            ));
        }
        let cookie_jar = Arc::new(Jar::default());
        let client = Client::builder()
            .cookie_provider(cookie_jar.clone())
            .redirect(Policy::none())
            .build()
            .map_err(internal_transport_error)?;
        let now = Instant::now();
        let mut flows = self.flows.lock().map_err(|_| internal_flow_error())?;
        flows.retain(|_, flow| !flow.is_pending() || flow.metadata().expires_at > now);
        if flows.len() >= self.max_flows
            && let Some(oldest) = flows
                .iter()
                .filter(|(_, flow)| flow.is_pending())
                .min_by_key(|(_, flow)| flow.metadata().created_sequence)
                .map(|(flow_id, _)| flow_id.clone())
        {
            flows.remove(&oldest);
        }
        if flows.len() >= self.max_flows {
            return Err(flow_capacity_error());
        }

        let flow_id = loop {
            let candidate = AuthFlowId::fresh();
            if !flows.contains_key(&candidate) {
                break candidate;
            }
        };
        let created_sequence = self.next_flow_sequence.fetch_add(1, Ordering::Relaxed);
        let metadata = FlowMetadata {
            created_sequence,
            expires_at: now + self.flow_lifetime,
        };
        flows.insert(
            flow_id.clone(),
            BrowserFlowState::Pending(BrowserFlow {
                request,
                metadata,
                client,
                _cookie_jar: cookie_jar,
            }),
        );

        Ok(AuthProgress::BrowserRedirect {
            flow_id,
            authorization_url,
        })
    }

    pub async fn complete_callback(
        &self,
        flow_id: &AuthFlowId,
        raw_callback: SecretString,
    ) -> Result<TokenBundle, AppError> {
        let callback = raw_callback.expose_secret();
        if callback.len() > self.max_callback_url_len {
            return Err(AppError::invalid_request("MTS callback URL is too long"));
        }

        let flow = {
            let now = Instant::now();
            let mut flows = self.flows.lock().map_err(|_| internal_flow_error())?;
            let Some(flow_state) = flows.get(flow_id) else {
                return Err(inactive_flow_error());
            };
            let BrowserFlowState::Pending(flow) = flow_state else {
                return Err(inactive_flow_error());
            };
            if flow.metadata.expires_at <= now {
                flows.remove(flow_id);
                return Err(inactive_flow_error());
            }
            flow.request
                .validate_callback_target(callback, &self.callback_target.url)?;
            let flow_state = flows
                .get_mut(flow_id)
                .expect("validated authentication flow remains present");
            let metadata = flow_state.metadata();
            let BrowserFlowState::Pending(flow) =
                std::mem::replace(flow_state, BrowserFlowState::InFlight(metadata))
            else {
                unreachable!("validated authentication flow changed while locked")
            };
            flow
        };
        let _in_flight = InFlightGuard {
            flows: &self.flows,
            flow_id: flow_id.clone(),
        };
        let callback_work = async {
            let response = flow
                .client
                .get(callback)
                .send()
                .await
                .map_err(upstream_unavailable)?;
            if response.status() != reqwest::StatusCode::OK {
                return Err(AppError::new(
                    ErrorCode::UpstreamAuthenticationFailed,
                    "MTS callback authentication failed.",
                    false,
                ));
            }
            if response
                .content_length()
                .is_some_and(|length| length > self.max_callback_body_len as u64)
            {
                return Err(callback_schema_error());
            }

            let mut body = Zeroizing::new(Vec::new());
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(upstream_unavailable)?;
                let next_len = body
                    .len()
                    .checked_add(chunk.len())
                    .ok_or_else(callback_schema_error)?;
                if next_len > self.max_callback_body_len {
                    return Err(callback_schema_error());
                }
                body.extend_from_slice(&chunk);
            }

            let callback_response: CallbackResponse =
                serde_json::from_slice(&body).map_err(|_| callback_schema_error())?;
            callback_response.into_tokens()
        };
        match tokio::time::timeout_at(flow.metadata.expires_at, callback_work).await {
            Ok(result) => result,
            Err(_) => Err(inactive_flow_error()),
        }
    }

    pub async fn submit(
        &self,
        flow_id: &AuthFlowId,
        _input: ChallengeInput,
    ) -> Result<AuthProgress, AppError> {
        let now = Instant::now();
        let mut flows = self.flows.lock().map_err(|_| internal_flow_error())?;
        let Some(flow_state) = flows.get(flow_id) else {
            return Err(inactive_flow_error());
        };
        let BrowserFlowState::Pending(flow) = flow_state else {
            return Err(inactive_flow_error());
        };
        if flow.metadata.expires_at <= now {
            flows.remove(flow_id);
            return Err(inactive_flow_error());
        }
        Err(AppError::new(
            ErrorCode::UpstreamSchemaChanged,
            "MTS browser challenge capture is required before challenge submission is supported.",
            false,
        ))
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CallbackResponse {
    web_sso_access_token: SecretString,
    keycloak_token_pair: KeycloakTokenPair,
    id_token: Zeroizing<String>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeycloakTokenPair {
    access_token: SecretString,
    refresh_token: SecretString,
}

impl CallbackResponse {
    fn into_tokens(self) -> Result<TokenBundle, AppError> {
        if self.web_sso_access_token.expose_secret().is_empty()
            || self
                .keycloak_token_pair
                .access_token
                .expose_secret()
                .is_empty()
            || self
                .keycloak_token_pair
                .refresh_token
                .expose_secret()
                .is_empty()
            || self.id_token.is_empty()
        {
            return Err(callback_schema_error());
        }

        let Self {
            web_sso_access_token,
            keycloak_token_pair,
            id_token,
        } = self;
        drop(id_token);
        Ok(TokenBundle {
            access_token: keycloak_token_pair.access_token,
            refresh_token: keycloak_token_pair.refresh_token,
            web_sso_access_token,
            device_token: None,
        })
    }
}

fn internal_transport_error(error: impl fmt::Display) -> AppError {
    AppError::new(
        ErrorCode::Internal,
        "Failed to initialize MTS authentication.",
        false,
    )
    .with_cause(error)
}

fn internal_flow_error() -> AppError {
    AppError::new(
        ErrorCode::Internal,
        "Authentication flow state is unavailable.",
        false,
    )
}

fn inactive_flow_error() -> AppError {
    AppError::new(
        ErrorCode::SessionExpired,
        "Authentication flow is unavailable or expired.",
        false,
    )
}

fn flow_capacity_error() -> AppError {
    AppError::new(
        ErrorCode::RateLimited,
        "Too many authentication flows are active.",
        true,
    )
}

fn upstream_unavailable(_error: reqwest::Error) -> AppError {
    AppError::new(
        ErrorCode::UpstreamUnavailable,
        "MTS authentication service is unavailable.",
        true,
    )
}

fn callback_schema_error() -> AppError {
    AppError::new(
        ErrorCode::UpstreamSchemaChanged,
        "MTS callback response was invalid.",
        false,
    )
}

#[cfg(feature = "test-support")]
fn validate_test_callback_target(target: &Url) -> Result<(), AppError> {
    if !target.username().is_empty()
        || target.password().is_some()
        || target.host_str().is_none()
        || target.path() != CALLBACK_PATH
        || target.query().is_some()
        || target.fragment().is_some()
    {
        return Err(AppError::invalid_request("invalid test callback target"));
    }
    Ok(())
}

/// A server-side MTS ID authorization flow request.
pub struct SsoRequest {
    device_id: String,
    state: String,
    state_bytes: [u8; 32],
}

impl SsoRequest {
    pub fn new(device_id: impl Into<String>, state: impl Into<String>) -> Result<Self, AppError> {
        let state = state.into();
        let state_bytes = decode_state(&state)
            .ok_or_else(|| AppError::invalid_request("invalid MTS authorization state"))?;

        Ok(Self {
            device_id: device_id.into(),
            state,
            state_bytes,
        })
    }

    pub fn with_fresh_state(device_id: impl Into<String>) -> Self {
        Self::new(device_id, generate_state()).expect("generated MTS state is valid")
    }

    pub fn state(&self) -> &str {
        &self.state
    }

    pub fn authorize_url(&self) -> Url {
        let mut url = Url::parse(AUTHORIZE_URL).expect("MTS authorization URL is valid");
        url.query_pairs_mut()
            .append_pair("client_id", "VDome")
            .append_pair("redirect_uri", CALLBACK_URL)
            .append_pair("response_type", "code")
            .append_pair("scope", SCOPE)
            .append_pair("state", &self.state)
            .append_pair("device_id", &self.device_id);
        url
    }

    pub fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::with_capacity(5);
        headers.insert("X-SDK-APPVersion", HeaderValue::from_static("3.0.1"));
        headers.insert("X-SDK-Name", HeaderValue::from_static("Login"));
        headers.insert("X-SDK-ClientId", HeaderValue::from_static("VDome"));
        headers.insert("X-SDK-Platform", HeaderValue::from_static("Android"));
        headers.insert("X-SDK-Version", HeaderValue::from_static("5.8.0"));
        headers
    }

    pub fn validate_callback(&self, callback_raw: &str) -> Result<(), AppError> {
        let target = Url::parse(CALLBACK_URL).expect("MTS callback URL is valid");
        self.validate_callback_target(callback_raw, &target)
    }

    fn validate_callback_target(&self, callback_raw: &str, target: &Url) -> Result<(), AppError> {
        if !has_exact_raw_callback_target(callback_raw, target.as_str()) {
            return Err(AppError::invalid_request("invalid MTS callback URL"));
        }

        let callback = Url::parse(callback_raw)
            .map_err(|_| AppError::invalid_request("invalid MTS callback URL"))?;
        let is_expected_callback = callback.scheme() == target.scheme()
            && callback.username().is_empty()
            && callback.password().is_none()
            && callback.host_str() == target.host_str()
            && callback.port() == target.port()
            && callback.path() == target.path()
            && callback.fragment().is_none();
        if !is_expected_callback {
            return Err(AppError::invalid_request("invalid MTS callback URL"));
        }

        let mut state_count = 0_u8;
        let mut received_state = None;
        for (name, value) in callback.query_pairs() {
            if name == "state" {
                state_count = state_count.saturating_add(1);
                if received_state.is_none() {
                    received_state = Some(value);
                }
            }
        }

        let mut received_bytes = [0_u8; 32];
        let mut decoded = 0_u8;
        if let Some(value) = received_state.and_then(|value| decode_state(&value)) {
            received_bytes = value;
            decoded = 1;
        }
        let matches = self.state_bytes.ct_eq(&received_bytes).unwrap_u8();
        if state_count != 1 || decoded & matches != 1 {
            return Err(AppError::invalid_request("invalid MTS callback state"));
        }

        Ok(())
    }
}

pub fn generate_state() -> String {
    let mut bytes = [0_u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn has_exact_raw_callback_target(callback: &str, expected_target: &str) -> bool {
    let target_end = callback
        .split(['?', '#'])
        .next()
        .expect("split always returns at least one segment")
        .len();
    callback[..target_end] == *expected_target
}

fn decode_state(state: &str) -> Option<[u8; 32]> {
    if state.len() != 43
        || !state
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return None;
    }

    URL_SAFE_NO_PAD.decode(state).ok()?.try_into().ok()
}

#[cfg(test)]
mod tests {
    use std::{
        error::Error,
        io::Write,
        sync::{Arc, Mutex},
    };

    use axum::response::IntoResponse;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
    };
    use tracing_subscriber::fmt::MakeWriter;

    use super::*;
    use crate::{error::ApiError, request_id::RequestId};

    const CALLBACK_SENTINEL: &str = "callback-query-secret-sentinel";

    #[derive(Clone)]
    struct BufferWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for BufferWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            std::io::Write::write(&mut *self.0.lock().expect("log buffer lock"), bytes)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'writer> MakeWriter<'writer> for BufferWriter {
        type Writer = Self;

        fn make_writer(&'writer self) -> Self::Writer {
            self.clone()
        }
    }

    async fn read_request_headers(stream: &mut TcpStream) {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 512];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let count = stream.read(&mut buffer).await.expect("read request");
            assert!(count > 0, "client closed before request headers completed");
            request.extend_from_slice(&buffer[..count]);
        }
    }

    async fn transport_and_callback(
        listener: &TcpListener,
    ) -> (MtsSsoTransport, AuthFlowId, SecretString) {
        let callback_target = Url::parse(&format!(
            "http://{}/user-service/api/auth/callback",
            listener.local_addr().expect("listener address")
        ))
        .expect("callback target");
        let mut transport = MtsSsoTransport::new("synthetic-device");
        transport.callback_target = CallbackTarget {
            url: callback_target.clone(),
        };
        let AuthProgress::BrowserRedirect {
            flow_id,
            authorization_url,
        } = transport.begin().await.expect("begin flow")
        else {
            panic!("begin must return browser redirect")
        };
        let state = authorization_url
            .query_pairs()
            .find_map(|(name, value)| (name == "state").then(|| value.into_owned()))
            .expect("authorization state");
        let callback = SecretString::from(format!(
            "{callback_target}?code=opaque&state={state}&sentinel={CALLBACK_SENTINEL}"
        ));
        (transport, flow_id, callback)
    }

    fn assert_callback_error_is_fully_redacted(error: AppError) {
        assert!(
            error.retained_cause_for_test().is_none(),
            "callback Reqwest errors must not be retained"
        );
        assert!(!error.to_string().contains(CALLBACK_SENTINEL));
        assert!(!format!("{error:?}").contains(CALLBACK_SENTINEL));
        let mut source = Error::source(&error);
        while let Some(cause) = source {
            assert!(!cause.to_string().contains(CALLBACK_SENTINEL));
            assert!(!format!("{cause:?}").contains(CALLBACK_SENTINEL));
            source = cause.source();
        }

        let output = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_writer(BufferWriter(Arc::clone(&output)))
            .finish();
        let dispatch = tracing::Dispatch::new(subscriber);
        tracing::dispatcher::with_default(&dispatch, || {
            let api_error = ApiError::new(
                RequestId::parse("e7411f58-d5e9-4a43-b25d-88ddf2014713").expect("request ID"),
                error,
            );
            let _response = api_error.into_response();
        });
        let formatted =
            String::from_utf8(output.lock().expect("log buffer lock").clone()).expect("UTF-8 logs");
        assert!(!formatted.contains(CALLBACK_SENTINEL));
    }

    #[tokio::test]
    async fn callback_send_failure_retains_no_url_or_query_secret() {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind listener");
        let (transport, flow_id, callback) = transport_and_callback(&listener).await;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept callback");
            read_request_headers(&mut stream).await;
        });

        let error = match transport.complete_callback(&flow_id, callback).await {
            Ok(_) => panic!("reset callback unexpectedly succeeded"),
            Err(error) => error,
        };
        server.await.expect("server task");

        assert_eq!(error.code, ErrorCode::UpstreamUnavailable);
        assert_callback_error_is_fully_redacted(error);
    }

    #[tokio::test]
    async fn callback_body_stream_failure_retains_no_url_or_query_secret() {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind listener");
        let (transport, flow_id, callback) = transport_and_callback(&listener).await;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept callback");
            read_request_headers(&mut stream).await;
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 128\r\nConnection: close\r\n\r\n{",
                )
                .await
                .expect("write incomplete response");
        });

        let error = match transport.complete_callback(&flow_id, callback).await {
            Ok(_) => panic!("incomplete callback body unexpectedly succeeded"),
            Err(error) => error,
        };
        server.await.expect("server task");

        assert_eq!(error.code, ErrorCode::UpstreamUnavailable);
        assert_callback_error_is_fully_redacted(error);
    }
}
