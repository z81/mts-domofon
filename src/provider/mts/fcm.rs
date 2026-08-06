use std::{fmt, sync::Arc, time::Duration};

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::Method;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use tokio::{io::AsyncWriteExt, sync::mpsc};
use tokio_util::sync::CancellationToken;

use crate::error::{AppError, ErrorCode};

use super::{MtsClient, ProviderBase};

const MAX_FCM_BODY_BYTES: usize = 64 * 1024;
const MAX_ID_BYTES: usize = 256;
const MAX_LABEL_BYTES: usize = 512;
const MAX_SECRET_BYTES: usize = 4 * 1024;
const MAX_CAMERA_STREAMS: usize = 16;
const MAX_URL_BYTES: usize = 4 * 1024;
const MAX_SELF_USERS: usize = 16;
const MAX_SECRET_STATE_BYTES: usize = 256 * 1024;
const MAX_PERSISTENT_IDS: usize = 128;
const MAX_PERSISTENT_ID_BYTES: usize = 512;
const MAX_CONSECUTIVE_STARTUP_FAILURES: u8 = 3;

const CURRENT_USER_PATH: &str = "/domofon/api/security/v1/users/self";
const USERS_PATH: &str = "/domofon/api/security/v1/users";

#[derive(Clone)]
pub struct IncomingCallBootstrap {
    intercom_id: String,
    caller_name: String,
    call_session_id: SecretString,
    camera_streams: Vec<SecretString>,
    websocket_url: SecretString,
    sip: SipBootstrapCredentials,
    timestamp_millis: u64,
}

impl IncomingCallBootstrap {
    pub fn intercom_id(&self) -> &str {
        &self.intercom_id
    }

    pub fn caller_name(&self) -> &str {
        &self.caller_name
    }

    pub fn call_session_id(&self) -> &SecretString {
        &self.call_session_id
    }

    pub fn camera_streams(&self) -> &[SecretString] {
        &self.camera_streams
    }

    pub fn websocket_url(&self) -> &SecretString {
        &self.websocket_url
    }

    pub fn sip(&self) -> &SipBootstrapCredentials {
        &self.sip
    }

    pub fn timestamp_millis(&self) -> u64 {
        self.timestamp_millis
    }
}

impl fmt::Debug for IncomingCallBootstrap {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IncomingCallBootstrap")
            .field("intercom_id", &Redacted)
            .field("caller_name", &Redacted)
            .field("call_session_id", &Redacted)
            .field("camera_streams", &Redacted)
            .field("websocket_url", &Redacted)
            .field("sip", &self.sip)
            .field("timestamp_millis", &self.timestamp_millis)
            .finish()
    }
}

#[derive(Clone)]
pub struct SipBootstrapCredentials {
    registrar: String,
    username: String,
    secret: SecretString,
}

impl SipBootstrapCredentials {
    pub fn registrar(&self) -> &str {
        &self.registrar
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn secret(&self) -> &SecretString {
        &self.secret
    }
}

impl fmt::Debug for SipBootstrapCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SipBootstrapCredentials")
            .field("registrar", &Redacted)
            .field("username", &Redacted)
            .field("secret", &Redacted)
            .finish()
    }
}

struct Redacted;

impl fmt::Debug for Redacted {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum FcmPayloadError {
    #[error("FCM payload exceeds its size bound")]
    TooLarge,
    #[error("FCM payload has an unsupported schema")]
    InvalidSchema,
}

#[derive(Deserialize)]
struct Envelope {
    data: serde_json::Value,
}

#[derive(Deserialize)]
struct WakeUpData {
    action: String,
    caller: String,
    call: String,
    camera: String,
    ts: String,
    #[serde(rename = "sipServer")]
    sip_server: String,
    #[serde(rename = "webRTC")]
    web_rtc: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CallerData {
    id: String,
    number: Option<String>,
    name: Option<String>,
    r#type: String,
    #[serde(default)]
    session_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CallData {
    session_id: String,
}

#[derive(Deserialize)]
struct CameraData {
    streams: Vec<CameraStreamData>,
}

#[derive(Deserialize)]
struct CameraStreamData {
    url: String,
}

#[derive(Deserialize)]
struct WebRtcData {
    ws: String,
}

#[derive(Deserialize)]
struct SipServerData {
    ip: String,
    sip: String,
    secret: String,
}

pub fn parse_incoming_call_bootstrap(
    body: &[u8],
) -> Result<Option<IncomingCallBootstrap>, FcmPayloadError> {
    if body.len() > MAX_FCM_BODY_BYTES {
        return Err(FcmPayloadError::TooLarge);
    }
    let envelope: Envelope =
        serde_json::from_slice(body).map_err(|_| FcmPayloadError::InvalidSchema)?;
    let action = envelope
        .data
        .get("action")
        .and_then(serde_json::Value::as_str)
        .ok_or(FcmPayloadError::InvalidSchema)?;
    if action != "wake_up" {
        return Ok(None);
    }

    let data: WakeUpData =
        serde_json::from_value(envelope.data).map_err(|_| FcmPayloadError::InvalidSchema)?;
    if data.action != "wake_up" {
        return Ok(None);
    }
    let caller: CallerData = parse_nested(&data.caller)?;
    let call: CallData = parse_nested(&data.call)?;
    let camera: CameraData = parse_nested(&data.camera)?;
    let web_rtc: WebRtcData = parse_nested(&data.web_rtc)?;
    let sip_server: SipServerData = parse_nested(&data.sip_server)?;

    validate_text(&caller.id, MAX_ID_BYTES)?;
    validate_optional_text(caller.number.as_deref(), MAX_LABEL_BYTES)?;
    validate_optional_text(caller.name.as_deref(), MAX_LABEL_BYTES)?;
    validate_text(&caller.r#type, MAX_LABEL_BYTES)?;
    if let Some(session_id) = &caller.session_id {
        validate_text(session_id, MAX_ID_BYTES)?;
    }
    validate_text(&call.session_id, MAX_ID_BYTES)?;
    validate_text(&web_rtc.ws, MAX_URL_BYTES)?;
    let websocket_url = normalize_websocket_url(&web_rtc.ws)?;
    validate_text(&sip_server.ip, MAX_LABEL_BYTES)?;
    validate_text(&sip_server.sip, MAX_LABEL_BYTES)?;
    validate_text(&sip_server.secret, MAX_SECRET_BYTES)?;
    if camera.streams.len() > MAX_CAMERA_STREAMS {
        return Err(FcmPayloadError::TooLarge);
    }
    let camera_streams = camera
        .streams
        .into_iter()
        .map(|stream| {
            validate_text(&stream.url, MAX_URL_BYTES)?;
            Ok(SecretString::from(stream.url))
        })
        .collect::<Result<Vec<_>, FcmPayloadError>>()?;
    let timestamp_millis = data
        .ts
        .parse::<u64>()
        .ok()
        .filter(|timestamp| *timestamp > 0)
        .ok_or(FcmPayloadError::InvalidSchema)?;
    let caller_name = caller_display_name(caller.name.as_deref(), caller.number.as_deref());

    Ok(Some(IncomingCallBootstrap {
        intercom_id: caller.id,
        caller_name,
        call_session_id: SecretString::from(call.session_id),
        camera_streams,
        websocket_url: SecretString::from(websocket_url),
        sip: SipBootstrapCredentials {
            registrar: sip_server.ip,
            username: sip_server.sip,
            secret: SecretString::from(sip_server.secret),
        },
        timestamp_millis,
    }))
}

fn parse_nested<T>(json: &str) -> Result<T, FcmPayloadError>
where
    T: for<'de> Deserialize<'de>,
{
    if json.len() > MAX_FCM_BODY_BYTES {
        return Err(FcmPayloadError::TooLarge);
    }
    serde_json::from_str(json).map_err(|_| FcmPayloadError::InvalidSchema)
}

fn validate_text(value: &str, max_bytes: usize) -> Result<(), FcmPayloadError> {
    if value.is_empty() || value.len() > max_bytes || value.chars().any(char::is_control) {
        return Err(if value.len() > max_bytes {
            FcmPayloadError::TooLarge
        } else {
            FcmPayloadError::InvalidSchema
        });
    }
    Ok(())
}

fn validate_optional_text(value: Option<&str>, max_bytes: usize) -> Result<(), FcmPayloadError> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.len() > max_bytes || value.chars().any(char::is_control) {
        return Err(if value.len() > max_bytes {
            FcmPayloadError::TooLarge
        } else {
            FcmPayloadError::InvalidSchema
        });
    }
    Ok(())
}

fn caller_display_name(name: Option<&str>, number: Option<&str>) -> String {
    [name, number]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|value| !value.is_empty())
        .unwrap_or("Домофон")
        .to_owned()
}

fn normalize_websocket_url(value: &str) -> Result<String, FcmPayloadError> {
    let mut url = url::Url::parse(value).map_err(|_| FcmPayloadError::InvalidSchema)?;
    if !matches!(url.scheme(), "https" | "wss")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(FcmPayloadError::InvalidSchema);
    }
    if url.scheme() == "https" {
        url.set_scheme("wss")
            .map_err(|_| FcmPayloadError::InvalidSchema)?;
    }
    Ok(url.into())
}

#[derive(Deserialize)]
struct CurrentUserResponse {
    #[serde(rename = "list")]
    users: Vec<CurrentUser>,
}

#[derive(Deserialize)]
struct CurrentUser {
    id: String,
}

#[derive(Serialize)]
struct RegisterFcmDeviceTokenRequest<'a> {
    payload: RegisterFcmPayload,
    fcm_token: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RegisterFcmPayload {
    platform: &'static str,
    brand: &'static str,
    manufacturer: &'static str,
    model: &'static str,
    system_version: &'static str,
    #[serde(rename = "app_version")]
    app_version: RegisterFcmAppVersion,
    #[serde(rename = "BundleId")]
    bundle_id: &'static str,
    #[serde(rename = "is_debug")]
    is_debug: &'static str,
}

#[derive(Serialize)]
struct RegisterFcmAppVersion {
    #[serde(rename = "versionName")]
    version_name: &'static str,
}

impl Default for RegisterFcmPayload {
    fn default() -> Self {
        Self {
            platform: "Android",
            brand: "VDome Web",
            manufacturer: "domru-mts",
            model: "Rust Service",
            system_version: "server",
            app_version: RegisterFcmAppVersion {
                version_name: "3.0.1",
            },
            bundle_id: "ru.mts.vdome.resident",
            is_debug: "false",
        }
    }
}

impl MtsClient {
    pub async fn current_user_id(&self) -> Result<String, AppError> {
        let request = self.authorized(Method::GET, ProviderBase::Gateway, CURRENT_USER_PATH)?;
        let response: CurrentUserResponse = self.send_json(request).await?;
        if response.users.is_empty() || response.users.len() > MAX_SELF_USERS {
            return Err(user_schema_error());
        }
        let id = response.users.into_iter().next().expect("checked above").id;
        if id.is_empty() || id.len() > MAX_ID_BYTES || id.chars().any(char::is_control) {
            return Err(user_schema_error());
        }
        Ok(id)
    }

    pub async fn register_fcm_device_token(
        &self,
        user_id: &str,
        fcm_token: &SecretString,
    ) -> Result<(), AppError> {
        if fcm_token.expose_secret().is_empty()
            || fcm_token.expose_secret().len() > MAX_SECRET_BYTES
        {
            return Err(AppError::invalid_request("invalid FCM token"));
        }
        let request = self
            .authorized_with_path_segment(
                Method::PUT,
                ProviderBase::Gateway,
                USERS_PATH,
                user_id,
                None,
            )?
            .json(&RegisterFcmDeviceTokenRequest {
                payload: RegisterFcmPayload::default(),
                fcm_token: fcm_token.expose_secret(),
            })?;
        self.send_empty(request).await
    }
}

fn user_schema_error() -> AppError {
    AppError::new(
        ErrorCode::UpstreamSchemaChanged,
        "Provider user response has an unsupported schema.",
        false,
    )
}

#[derive(Clone, Serialize, Deserialize)]
pub struct FcmSecretState {
    registration: fcm_push_listener::Registration,
    persistent_ids: Vec<String>,
}

impl FcmSecretState {
    pub fn new(
        registration: fcm_push_listener::Registration,
        persistent_ids: Vec<String>,
    ) -> Result<Self, FcmServiceError> {
        let state = Self {
            registration,
            persistent_ids,
        };
        state.validate()?;
        Ok(state)
    }

    pub fn fcm_token(&self) -> &str {
        &self.registration.fcm_token
    }

    pub fn persistent_ids(&self) -> &[String] {
        &self.persistent_ids
    }

    /// Serializes secret FCM registration material for an encrypted secret store.
    pub fn serialize_secret(&self) -> Result<Vec<u8>, FcmServiceError> {
        let bytes = serde_json::to_vec(self).map_err(|_| FcmServiceError::Persistence)?;
        if bytes.len() > MAX_SECRET_STATE_BYTES {
            return Err(FcmServiceError::Persistence);
        }
        Ok(bytes)
    }

    /// Deserializes secret FCM registration material read from an encrypted secret store.
    pub fn deserialize_secret(bytes: &[u8]) -> Result<Self, FcmServiceError> {
        if bytes.len() > MAX_SECRET_STATE_BYTES {
            return Err(FcmServiceError::Persistence);
        }
        let state: Self =
            serde_json::from_slice(bytes).map_err(|_| FcmServiceError::Persistence)?;
        state.validate()?;
        Ok(state)
    }

    fn validate(&self) -> Result<(), FcmServiceError> {
        if self.registration.fcm_token.is_empty()
            || self.registration.fcm_token.len() > MAX_SECRET_BYTES
            || self.persistent_ids.len() > MAX_PERSISTENT_IDS
            || self
                .persistent_ids
                .iter()
                .any(|id| id.is_empty() || id.len() > MAX_PERSISTENT_ID_BYTES)
        {
            return Err(FcmServiceError::Persistence);
        }
        Ok(())
    }

    fn remember_persistent_id(&mut self, id: String) -> Result<bool, FcmServiceError> {
        if id.is_empty() || id.len() > MAX_PERSISTENT_ID_BYTES {
            return Err(FcmServiceError::Persistence);
        }
        if self.persistent_ids.iter().any(|existing| existing == &id) {
            return Ok(false);
        }
        if self.persistent_ids.len() == MAX_PERSISTENT_IDS {
            self.persistent_ids.remove(0);
        }
        self.persistent_ids.push(id);
        Ok(true)
    }
}

impl fmt::Debug for FcmSecretState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FcmSecretState")
            .field("registration", &Redacted)
            .field("persistent_ids", &Redacted)
            .finish()
    }
}

pub struct FirebaseConfig {
    app_id: String,
    project_id: String,
    api_key: SecretString,
}

impl FirebaseConfig {
    pub fn new(
        app_id: impl Into<String>,
        project_id: impl Into<String>,
        api_key: SecretString,
    ) -> Result<Self, FcmServiceError> {
        let config = Self {
            app_id: app_id.into(),
            project_id: project_id.into(),
            api_key,
        };
        if config.app_id.is_empty()
            || config.app_id.len() > MAX_LABEL_BYTES
            || config.project_id.is_empty()
            || config.project_id.len() > MAX_LABEL_BYTES
            || config.api_key.expose_secret().is_empty()
            || config.api_key.expose_secret().len() > MAX_SECRET_BYTES
        {
            return Err(FcmServiceError::Configuration);
        }
        Ok(config)
    }

    /// Firebase public project configuration extracted from VDome 3.0.1 resources.
    pub fn vdome() -> Self {
        Self::new(
            "1:830027474549:android:d2e4bd536e9d0845",
            "digital-mc-8e089",
            SecretString::from("AIzaSyBrtvigKkw9b6uB01yppWeFC87f50PO_P8"),
        )
        .expect("embedded VDome Firebase configuration is valid")
    }
}

impl fmt::Debug for FirebaseConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FirebaseConfig")
            .field("app_id", &self.app_id)
            .field("project_id", &self.project_id)
            .field("api_key", &Redacted)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum FcmServiceError {
    #[error("FCM transport failed")]
    Transport,
    #[error("FCM secret persistence failed")]
    Persistence,
    #[error("FCM configuration is invalid")]
    Configuration,
    #[error("FCM service channel closed")]
    ChannelClosed,
    #[error("FCM registration may be stale")]
    RegistrationStale,
}

pub enum FcmTransportEvent {
    Data {
        body: Vec<u8>,
        persistent_id: Option<String>,
    },
    RefreshedSession(fcm_push_listener::Session),
}

impl FcmTransportEvent {
    pub fn data(body: Vec<u8>, persistent_id: Option<String>) -> Self {
        Self::Data {
            body,
            persistent_id,
        }
    }
}

impl fmt::Debug for FcmTransportEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Data { .. } => formatter
                .debug_struct("FcmTransportEvent::Data")
                .field("body", &Redacted)
                .field("persistent_id", &Redacted)
                .finish(),
            Self::RefreshedSession(_) => formatter.write_str("RefreshedSession([REDACTED])"),
        }
    }
}

#[async_trait]
/// Persistence boundary for registration keys and acknowledged MCS persistent IDs.
///
/// Implementations must treat the complete state as secret material. The coordinator should
/// back this trait with the application's encrypted secret store before starting the supervisor.
pub trait FcmSecretStore: Send + Sync {
    async fn load(&self) -> Result<Option<FcmSecretState>, FcmServiceError>;
    async fn save(&self, state: &FcmSecretState) -> Result<(), FcmServiceError>;
    async fn clear(&self) -> Result<(), FcmServiceError>;
}

#[async_trait]
/// Injectable registration/listening boundary used by the supervisor and deterministic tests.
pub trait FcmTransport: Send + Sync {
    async fn register(&self, config: &FirebaseConfig) -> Result<FcmSecretState, FcmServiceError>;

    async fn listen(
        &self,
        state: &FcmSecretState,
        events: mpsc::Sender<FcmTransportEvent>,
        cancellation: CancellationToken,
    ) -> Result<(), FcmServiceError>;
}

pub struct FcmBootstrapService {
    transport: Arc<dyn FcmTransport>,
    store: Arc<dyn FcmSecretStore>,
    config: FirebaseConfig,
    reconnect_delay: Duration,
}

impl FcmBootstrapService {
    /// Builds an unstarted listener. Integration owns the task, output channel, and cancellation
    /// token; no global application state is mutated by this slice.
    pub fn new(
        transport: Arc<dyn FcmTransport>,
        store: Arc<dyn FcmSecretStore>,
        config: FirebaseConfig,
        reconnect_delay: Duration,
    ) -> Self {
        Self {
            transport,
            store,
            config,
            reconnect_delay,
        }
    }

    pub async fn run(
        &self,
        output: mpsc::Sender<IncomingCallBootstrap>,
        cancellation: CancellationToken,
    ) -> Result<(), FcmServiceError> {
        let mut state = tokio::select! {
            () = cancellation.cancelled() => return Ok(()),
            result = self.ensure_registration() => result?,
        };
        let mut consecutive_startup_failures = 0_u8;

        loop {
            if cancellation.is_cancelled() || output.is_closed() {
                return Ok(());
            }
            let connection_cancel = cancellation.child_token();
            let (event_tx, mut event_rx) = mpsc::channel(16);
            let connection_state = state.clone();
            let listen =
                self.transport
                    .listen(&connection_state, event_tx, connection_cancel.clone());
            tokio::pin!(listen);
            let mut observed_event = false;
            let connection_result = loop {
                tokio::select! {
                    biased;
                    () = cancellation.cancelled() => {
                        connection_cancel.cancel();
                        return Ok(());
                    }
                    () = output.closed() => {
                        connection_cancel.cancel();
                        return Ok(());
                    }
                    event = event_rx.recv() => {
                        let Some(event) = event else {
                            break listen.await;
                        };
                        observed_event = true;
                        self.handle_event(&mut state, event, &output, &cancellation).await?;
                    }
                    result = &mut listen => break result,
                }
            };
            connection_cancel.cancel();
            if matches!(connection_result, Err(FcmServiceError::Transport)) && !observed_event {
                consecutive_startup_failures = consecutive_startup_failures.saturating_add(1);
                if consecutive_startup_failures >= MAX_CONSECUTIVE_STARTUP_FAILURES {
                    return Err(FcmServiceError::RegistrationStale);
                }
            } else {
                consecutive_startup_failures = 0;
            }
            tokio::select! {
                () = cancellation.cancelled() => return Ok(()),
                () = output.closed() => return Ok(()),
                () = tokio::time::sleep(self.reconnect_delay) => {}
            }
        }
    }

    /// Loads a persisted registration or creates and persists one through the async transport.
    pub async fn ensure_registration(&self) -> Result<FcmSecretState, FcmServiceError> {
        match self.store.load().await? {
            Some(state) => Ok(state),
            None => {
                let state = self.transport.register(&self.config).await?;
                self.store.save(&state).await?;
                Ok(state)
            }
        }
    }

    async fn handle_event(
        &self,
        state: &mut FcmSecretState,
        event: FcmTransportEvent,
        output: &mpsc::Sender<IncomingCallBootstrap>,
        cancellation: &CancellationToken,
    ) -> Result<(), FcmServiceError> {
        match event {
            FcmTransportEvent::RefreshedSession(session) => {
                state.registration.gcm = session;
                self.store.save(state).await?;
            }
            FcmTransportEvent::Data {
                body,
                persistent_id,
            } => {
                if let Some(id) = persistent_id {
                    if !state.remember_persistent_id(id)? {
                        return Ok(());
                    }
                    self.store.save(state).await?;
                }
                let Ok(Some(bootstrap)) = parse_incoming_call_bootstrap(&body) else {
                    return Ok(());
                };
                tokio::select! {
                    () = cancellation.cancelled() => return Ok(()),
                    result = output.send(bootstrap) => {
                        result.map_err(|_| FcmServiceError::ChannelClosed)?;
                    }
                }
            }
        }
        Ok(())
    }
}

pub struct VendoredFcmTransport {
    http: reqwest::Client,
}

pub async fn acknowledge_heartbeat<W>(writer: &mut W) -> Result<(), FcmServiceError>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    writer
        .write_all(&fcm_push_listener::new_heartbeat_ack())
        .await
        .map_err(|_| FcmServiceError::Transport)
}

impl VendoredFcmTransport {
    pub fn new() -> Result<Self, FcmServiceError> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| FcmServiceError::Transport)?;
        Ok(Self { http })
    }
}

#[async_trait]
impl FcmTransport for VendoredFcmTransport {
    async fn register(&self, config: &FirebaseConfig) -> Result<FcmSecretState, FcmServiceError> {
        let registration = fcm_push_listener::register(
            &self.http,
            &config.app_id,
            &config.project_id,
            config.api_key.expose_secret(),
            None,
        )
        .await
        .map_err(|_| FcmServiceError::Transport)?;
        FcmSecretState::new(registration, Vec::new())
    }

    async fn listen(
        &self,
        state: &FcmSecretState,
        events: mpsc::Sender<FcmTransportEvent>,
        cancellation: CancellationToken,
    ) -> Result<(), FcmServiceError> {
        let checked = state
            .registration
            .gcm
            .checkin(&self.http)
            .await
            .map_err(|_| FcmServiceError::Transport)?;
        if checked.changed(&state.registration.gcm) {
            events
                .send(FcmTransportEvent::RefreshedSession((*checked).clone()))
                .await
                .map_err(|_| FcmServiceError::ChannelClosed)?;
        }
        let connection = checked
            .new_connection(state.persistent_ids.clone())
            .await
            .map_err(|_| FcmServiceError::Transport)?;
        let mut stream =
            fcm_push_listener::MessageStream::wrap(connection, &state.registration.keys);

        loop {
            let message = tokio::select! {
                () = cancellation.cancelled() => return Ok(()),
                message = stream.next() => message,
            };
            match message {
                Some(Ok(fcm_push_listener::Message::HeartbeatPing)) => {
                    tokio::select! {
                        () = cancellation.cancelled() => return Ok(()),
                        result = acknowledge_heartbeat(&mut *stream) => {
                            result?;
                        }
                    }
                }
                Some(Ok(fcm_push_listener::Message::Data(message))) => {
                    events
                        .send(FcmTransportEvent::data(message.body, message.persistent_id))
                        .await
                        .map_err(|_| FcmServiceError::ChannelClosed)?;
                }
                Some(Ok(fcm_push_listener::Message::Other(_, _))) => {}
                Some(Err(_)) | None => return Err(FcmServiceError::Transport),
            }
        }
    }
}
