use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use http::header::{HeaderValue, SEC_WEBSOCKET_PROTOCOL};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::{
    CloseFrame, WebSocketConfig, frame::coding::CloseCode,
};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async_with_config};
use url::Url;

use super::{SipError, SipLimits, SipMessage, parse_frame};

const SIP_SUBPROTOCOL: &str = "sip";
const MAX_WSS_MESSAGE_BYTES: usize = 256 * 1024;
const MAX_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone)]
pub struct WssTransportConfig {
    url: Url,
    sip_limits: SipLimits,
    connect_timeout: Duration,
    read_timeout: Duration,
    write_timeout: Duration,
}

impl WssTransportConfig {
    pub fn new(url: Url, sip_limits: SipLimits) -> Result<Self, TransportError> {
        if url.scheme() != "wss"
            || url.host_str().is_none()
            || url.username() != ""
            || url.password().is_some()
            || !valid_sip_limits(sip_limits)
            || sip_limits.max_message_bytes > MAX_WSS_MESSAGE_BYTES
        {
            return Err(TransportError::InvalidConfig);
        }
        Ok(Self {
            url,
            sip_limits,
            connect_timeout: Duration::from_secs(10),
            read_timeout: Duration::from_secs(45),
            write_timeout: Duration::from_secs(10),
        })
    }

    pub fn with_timeouts(
        mut self,
        connect_timeout: Duration,
        read_timeout: Duration,
        write_timeout: Duration,
    ) -> Result<Self, TransportError> {
        if [connect_timeout, read_timeout, write_timeout]
            .into_iter()
            .any(|value| value.is_zero() || value > MAX_TIMEOUT)
        {
            return Err(TransportError::InvalidConfig);
        }
        self.connect_timeout = connect_timeout;
        self.read_timeout = read_timeout;
        self.write_timeout = write_timeout;
        Ok(self)
    }

    pub fn connect_timeout(&self) -> Duration {
        self.connect_timeout
    }

    pub fn read_timeout(&self) -> Duration {
        self.read_timeout
    }

    pub fn write_timeout(&self) -> Duration {
        self.write_timeout
    }

    pub fn handshake_request(&self) -> Result<http::Request<()>, TransportError> {
        let mut request = self
            .url
            .as_str()
            .into_client_request()
            .map_err(|_| TransportError::InvalidConfig)?;
        request.headers_mut().insert(
            SEC_WEBSOCKET_PROTOCOL,
            HeaderValue::from_static(SIP_SUBPROTOCOL),
        );
        Ok(request)
    }
}

impl fmt::Debug for WssTransportConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WssTransportConfig")
            .field("url", &"[REDACTED]")
            .field("sip_limits", &self.sip_limits)
            .field("connect_timeout", &self.connect_timeout)
            .field("read_timeout", &self.read_timeout)
            .field("write_timeout", &self.write_timeout)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReconnectPolicy {
    max_attempts: u8,
    initial_delay: Duration,
    max_delay: Duration,
}

impl ReconnectPolicy {
    pub fn new(
        max_attempts: u8,
        initial_delay: Duration,
        max_delay: Duration,
    ) -> Result<Self, TransportError> {
        if max_attempts == 0
            || max_attempts > 10
            || initial_delay.is_zero()
            || initial_delay > max_delay
            || max_delay > Duration::from_secs(60)
        {
            return Err(TransportError::InvalidConfig);
        }
        Ok(Self {
            max_attempts,
            initial_delay,
            max_delay,
        })
    }

    pub fn delay_for(&self, attempt: u8) -> Option<Duration> {
        if attempt == 0 || attempt > self.max_attempts {
            return None;
        }
        let multiplier = 1_u32 << u32::from(attempt - 1);
        Some(
            self.initial_delay
                .saturating_mul(multiplier)
                .min(self.max_delay),
        )
    }
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 2,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(10),
        }
    }
}

#[async_trait]
pub trait SipTransport: Send {
    async fn send(&mut self, message: &SipMessage) -> Result<(), TransportError>;
    async fn receive(&mut self) -> Result<SipMessage, TransportError>;
    async fn close(&mut self) -> Result<(), TransportError>;
}

pub struct WssSipTransport {
    stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
    sip_limits: SipLimits,
    read_timeout: Duration,
    write_timeout: Duration,
}

impl WssSipTransport {
    pub async fn connect(config: WssTransportConfig) -> Result<Self, TransportError> {
        let request = config.handshake_request()?;
        let websocket_config = WebSocketConfig::default()
            .read_buffer_size(16 * 1024)
            .write_buffer_size(16 * 1024)
            .max_write_buffer_size(config.sip_limits.max_message_bytes + 32 * 1024)
            .max_message_size(Some(config.sip_limits.max_message_bytes))
            .max_frame_size(Some(config.sip_limits.max_message_bytes));
        let (stream, response) = timeout(
            config.connect_timeout,
            connect_async_with_config(request, Some(websocket_config), false),
        )
        .await
        .map_err(|_| TransportError::ConnectTimeout)?
        .map_err(|_| TransportError::HandshakeFailed)?;
        if response
            .headers()
            .get(SEC_WEBSOCKET_PROTOCOL)
            .and_then(|value| value.to_str().ok())
            != Some(SIP_SUBPROTOCOL)
        {
            return Err(TransportError::SubprotocolRejected);
        }
        Ok(Self {
            stream,
            sip_limits: config.sip_limits,
            read_timeout: config.read_timeout,
            write_timeout: config.write_timeout,
        })
    }
}

impl fmt::Debug for WssSipTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WssSipTransport")
            .field("sip_limits", &self.sip_limits)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl SipTransport for WssSipTransport {
    async fn send(&mut self, message: &SipMessage) -> Result<(), TransportError> {
        let wire = message.to_bytes(self.sip_limits)?;
        let text = String::from_utf8(wire).map_err(|_| TransportError::NonTextSipMessage)?;
        timeout(self.write_timeout, self.stream.send(Message::text(text)))
            .await
            .map_err(|_| TransportError::WriteTimeout)?
            .map_err(|_| TransportError::ConnectionFailed)
    }

    async fn receive(&mut self) -> Result<SipMessage, TransportError> {
        loop {
            let message = timeout(self.read_timeout, self.stream.next())
                .await
                .map_err(|_| TransportError::ReadTimeout)?
                .ok_or(TransportError::Closed)?
                .map_err(|_| TransportError::ConnectionFailed)?;
            match message {
                Message::Text(text) => {
                    if text.len() > self.sip_limits.max_message_bytes {
                        return Err(TransportError::FrameTooLarge);
                    }
                    return parse_frame(text.as_bytes(), self.sip_limits)
                        .map_err(TransportError::Sip);
                }
                Message::Ping(payload) => {
                    timeout(self.write_timeout, self.stream.send(Message::Pong(payload)))
                        .await
                        .map_err(|_| TransportError::WriteTimeout)?
                        .map_err(|_| TransportError::ConnectionFailed)?;
                }
                Message::Pong(_) => {}
                Message::Close(_) => return Err(TransportError::Closed),
                Message::Binary(_) | Message::Frame(_) => {
                    return Err(TransportError::NonTextFrame);
                }
            }
        }
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        let frame = CloseFrame {
            code: CloseCode::Normal,
            reason: "Manual".into(),
        };
        timeout(
            self.write_timeout,
            self.stream.send(Message::Close(Some(frame))),
        )
        .await
        .map_err(|_| TransportError::WriteTimeout)?
        .map_err(|_| TransportError::ConnectionFailed)
    }
}

fn valid_sip_limits(limits: SipLimits) -> bool {
    limits.max_message_bytes > 0
        && limits.max_header_bytes >= 4
        && limits.max_header_count > 0
        && limits.max_line_bytes > 0
        && limits.max_body_bytes <= limits.max_message_bytes
        && limits.max_header_bytes <= limits.max_message_bytes
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    #[error("SIP WebSocket configuration is invalid")]
    InvalidConfig,
    #[error("SIP WebSocket connection timed out")]
    ConnectTimeout,
    #[error("SIP WebSocket read timed out")]
    ReadTimeout,
    #[error("SIP WebSocket write timed out")]
    WriteTimeout,
    #[error("SIP WebSocket handshake failed")]
    HandshakeFailed,
    #[error("SIP WebSocket subprotocol was not accepted")]
    SubprotocolRejected,
    #[error("SIP WebSocket connection failed")]
    ConnectionFailed,
    #[error("SIP WebSocket connection closed")]
    Closed,
    #[error("SIP WebSocket frame exceeds the configured limit")]
    FrameTooLarge,
    #[error("SIP WebSocket received a non-text frame")]
    NonTextFrame,
    #[error("outbound SIP message is not UTF-8 text")]
    NonTextSipMessage,
    #[error(transparent)]
    Sip(#[from] SipError),
}
