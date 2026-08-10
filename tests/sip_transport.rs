use std::collections::VecDeque;
use std::time::Duration;

use async_trait::async_trait;
use domru_mts::calls::sip::{
    ReconnectPolicy, SipLimits, SipMessage, SipMethod, SipRequest, SipResponse, SipTransport,
    TransportError, WssTransportConfig,
};
use url::Url;

#[test]
fn wss_config_requires_secure_websocket_and_builds_a_safe_handshake() {
    for url in ["ws://sip.example/ws", "https://sip.example/ws"] {
        assert!(
            WssTransportConfig::new(Url::parse(url).expect("URL"), SipLimits::default()).is_err()
        );
    }

    let config = WssTransportConfig::new(
        Url::parse("wss://secret-host.example/per-call?token=secret-token").expect("URL"),
        SipLimits::default(),
    )
    .expect("valid WSS config");
    let request = config.handshake_request().expect("handshake request");
    assert_eq!(
        request
            .headers()
            .get("Sec-WebSocket-Protocol")
            .and_then(|value| value.to_str().ok()),
        Some("sip")
    );
    assert_eq!(
        request
            .headers()
            .get("Origin")
            .and_then(|value| value.to_str().ok()),
        Some("https://secret-host.example")
    );
    assert!(request.headers().get("Authorization").is_none());
    assert!(request.headers().get("Cookie").is_none());

    let diagnostics = format!("{config:?}");
    assert!(!diagnostics.contains("secret-host"));
    assert!(!diagnostics.contains("secret-token"));
}

#[test]
fn transport_config_bounds_messages_and_timeouts() {
    let url = Url::parse("wss://sip.example/ws").expect("URL");
    let invalid_limits = SipLimits {
        max_message_bytes: 0,
        ..SipLimits::default()
    };
    assert!(WssTransportConfig::new(url.clone(), invalid_limits).is_err());

    let config = WssTransportConfig::new(url, SipLimits::default())
        .expect("config")
        .with_timeouts(
            Duration::from_secs(5),
            Duration::from_secs(30),
            Duration::from_secs(5),
        )
        .expect("bounded timeouts");
    assert_eq!(config.connect_timeout(), Duration::from_secs(5));
    assert_eq!(config.read_timeout(), Duration::from_secs(30));
    assert_eq!(config.write_timeout(), Duration::from_secs(5));

    assert!(
        config
            .clone()
            .with_timeouts(
                Duration::ZERO,
                Duration::from_secs(1),
                Duration::from_secs(1)
            )
            .is_err()
    );
    assert!(
        config
            .with_timeouts(
                Duration::from_secs(121),
                Duration::from_secs(1),
                Duration::from_secs(1)
            )
            .is_err()
    );
}

#[test]
fn reconnect_policy_is_explicit_bounded_and_capped() {
    let policy = ReconnectPolicy::new(3, Duration::from_millis(250), Duration::from_secs(1))
        .expect("policy");
    assert_eq!(policy.delay_for(1), Some(Duration::from_millis(250)));
    assert_eq!(policy.delay_for(2), Some(Duration::from_millis(500)));
    assert_eq!(policy.delay_for(3), Some(Duration::from_secs(1)));
    assert_eq!(policy.delay_for(4), None);
    assert!(ReconnectPolicy::new(0, Duration::from_secs(1), Duration::from_secs(1)).is_err());
    assert!(ReconnectPolicy::new(11, Duration::from_secs(1), Duration::from_secs(1)).is_err());
}

struct ScriptedTransport {
    sent: Vec<SipMessage>,
    inbound: VecDeque<SipMessage>,
}

#[async_trait]
impl SipTransport for ScriptedTransport {
    async fn send(&mut self, message: &SipMessage) -> Result<(), TransportError> {
        self.sent.push(message.clone());
        Ok(())
    }

    async fn receive(&mut self) -> Result<SipMessage, TransportError> {
        self.inbound.pop_front().ok_or(TransportError::Closed)
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        Ok(())
    }
}

async fn one_exchange(transport: &mut dyn SipTransport) -> Result<SipMessage, TransportError> {
    let request = SipRequest::new(SipMethod::Options, "sip:sip.example").expect("valid OPTIONS");
    transport.send(&SipMessage::Request(request)).await?;
    transport.receive().await
}

#[tokio::test]
async fn state_machines_can_use_an_injected_transport_deterministically() {
    let response = SipMessage::Response(SipResponse::new(200, "OK").expect("response"));
    let mut transport = ScriptedTransport {
        sent: Vec::new(),
        inbound: VecDeque::from([response.clone()]),
    };

    assert_eq!(
        one_exchange(&mut transport).await.expect("exchange"),
        response
    );
    assert_eq!(transport.sent.len(), 1);
    let SipMessage::Request(request) = &transport.sent[0] else {
        panic!("expected request");
    };
    assert_eq!(request.method(), &SipMethod::Options);
}
