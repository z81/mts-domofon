use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use domru_mts::provider::mts::fcm::{
    FcmBootstrapService, FcmSecretState, FcmSecretStore, FcmServiceError, FcmTransport,
    FcmTransportEvent, FirebaseConfig, acknowledge_heartbeat,
};
use fcm_push_listener::{Registration, Session, WebPushKeys};
use secrecy::SecretString;
use tokio::{
    io::AsyncReadExt,
    sync::{Mutex, mpsc},
};
use tokio_util::sync::CancellationToken;

fn registration() -> Registration {
    Registration {
        fcm_token: "fcm-token-secret-marker".into(),
        gcm: Session {
            android_id: 42,
            security_token: 84,
        },
        keys: WebPushKeys {
            public_key: vec![1, 2, 3],
            private_key: vec![4, 5, 6],
            auth_secret: vec![7, 8, 9],
        },
    }
}

fn wake_up_body() -> Vec<u8> {
    let json = serde_json::json!({
        "data": {
            "action": "wake_up",
            "caller": serde_json::json!({
                "id": "intercom-service",
                "number": "1",
                "name": "Door",
                "type": "intercom"
            }).to_string(),
            "call": serde_json::json!({"sessionId": "session-secret"}).to_string(),
            "camera": serde_json::json!({"streams": []}).to_string(),
            "ts": "1770000000000",
            "sipServer": serde_json::json!({
                "ip": "sip.invalid",
                "sip": "resident",
                "secret": "sip-secret"
            }).to_string(),
            "webRTC": serde_json::json!({"ws": "wss://webrtc.invalid/ws"}).to_string()
        }
    });
    serde_json::to_vec(&json).unwrap()
}

#[derive(Default)]
struct MemoryStore {
    state: Mutex<Option<FcmSecretState>>,
    saves: AtomicUsize,
}

#[async_trait]
impl FcmSecretStore for MemoryStore {
    async fn load(&self) -> Result<Option<FcmSecretState>, FcmServiceError> {
        Ok(self.state.lock().await.clone())
    }

    async fn save(&self, state: &FcmSecretState) -> Result<(), FcmServiceError> {
        *self.state.lock().await = Some(state.clone());
        self.saves.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct ReconnectingTransport {
    attempts: AtomicUsize,
    body: Vec<u8>,
}

struct BlockingRegistration;

#[async_trait]
impl FcmTransport for BlockingRegistration {
    async fn register(&self, _config: &FirebaseConfig) -> Result<FcmSecretState, FcmServiceError> {
        std::future::pending().await
    }

    async fn listen(
        &self,
        _state: &FcmSecretState,
        _events: mpsc::Sender<FcmTransportEvent>,
        _cancellation: CancellationToken,
    ) -> Result<(), FcmServiceError> {
        unreachable!("registration never completed")
    }
}

#[async_trait]
impl FcmTransport for ReconnectingTransport {
    async fn register(&self, _config: &FirebaseConfig) -> Result<FcmSecretState, FcmServiceError> {
        panic!("stored registration should be reused")
    }

    async fn listen(
        &self,
        _state: &FcmSecretState,
        events: mpsc::Sender<FcmTransportEvent>,
        cancellation: CancellationToken,
    ) -> Result<(), FcmServiceError> {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
        if attempt == 0 {
            return Err(FcmServiceError::Transport);
        }
        events
            .send(FcmTransportEvent::data(
                self.body.clone(),
                Some("persistent-1".into()),
            ))
            .await
            .map_err(|_| FcmServiceError::ChannelClosed)?;
        cancellation.cancelled().await;
        Ok(())
    }
}

#[test]
fn secret_state_round_trips_but_debug_redacts_registration_and_ids() {
    let state = FcmSecretState::new(registration(), vec!["persistent-secret".into()]).unwrap();
    let encoded = state.serialize_secret().unwrap();
    let decoded = FcmSecretState::deserialize_secret(&encoded).unwrap();

    assert_eq!(decoded.fcm_token(), "fcm-token-secret-marker");
    assert_eq!(decoded.persistent_ids(), ["persistent-secret"]);
    let debug = format!("{decoded:?}");
    for secret in ["fcm-token-secret-marker", "persistent-secret", "4, 5, 6"] {
        assert!(!debug.contains(secret));
    }
}

#[tokio::test]
async fn service_reconnects_persists_message_id_emits_bootstrap_and_cancels() {
    let store = Arc::new(MemoryStore::default());
    *store.state.lock().await = Some(FcmSecretState::new(registration(), vec![]).unwrap());
    let transport = Arc::new(ReconnectingTransport {
        attempts: AtomicUsize::new(0),
        body: wake_up_body(),
    });
    let service = FcmBootstrapService::new(
        transport.clone(),
        store.clone(),
        FirebaseConfig::new(
            "app-id",
            "project-id",
            SecretString::from("firebase-api-secret"),
        )
        .unwrap(),
        Duration::from_millis(1),
    );
    let cancellation = CancellationToken::new();
    let (tx, mut rx) = mpsc::channel(1);
    let task = tokio::spawn({
        let cancellation = cancellation.clone();
        async move { service.run(tx, cancellation).await }
    });

    let bootstrap = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bootstrap.intercom_id(), "intercom-service");
    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .expect("service must be cancellation-safe")
        .unwrap()
        .unwrap();

    assert!(transport.attempts.load(Ordering::SeqCst) >= 2);
    let state = store.state.lock().await.clone().unwrap();
    assert_eq!(state.persistent_ids(), ["persistent-1"]);
    assert!(store.saves.load(Ordering::SeqCst) >= 1);
}

#[tokio::test]
async fn heartbeat_ack_writes_the_mcs_ack_frame() {
    let (mut writer, mut reader) = tokio::io::duplex(16);

    acknowledge_heartbeat(&mut writer).await.unwrap();

    let mut bytes = [0_u8; 2];
    reader.read_exact(&mut bytes).await.unwrap();
    assert_eq!(bytes, [1, 0]);
}

#[tokio::test]
async fn cancellation_interrupts_an_in_flight_network_registration() {
    let service = FcmBootstrapService::new(
        Arc::new(BlockingRegistration),
        Arc::new(MemoryStore::default()),
        FirebaseConfig::new(
            "app-id",
            "project-id",
            SecretString::from("firebase-api-secret"),
        )
        .unwrap(),
        Duration::from_millis(1),
    );
    let cancellation = CancellationToken::new();
    let (tx, _rx) = mpsc::channel(1);
    let task = tokio::spawn({
        let cancellation = cancellation.clone();
        async move { service.run(tx, cancellation).await }
    });
    tokio::task::yield_now().await;

    cancellation.cancel();

    tokio::time::timeout(Duration::from_millis(100), task)
        .await
        .expect("registration future must be dropped on cancellation")
        .unwrap()
        .unwrap();
}

#[test]
fn firebase_config_debug_redacts_the_api_key() {
    let config = FirebaseConfig::new(
        "app-id",
        "project-id",
        SecretString::from("firebase-api-secret-marker"),
    )
    .unwrap();

    let debug = format!("{config:?}");
    assert!(!debug.contains("firebase-api-secret-marker"));
    assert!(debug.contains("[REDACTED]"));
}
