use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use domru_mts::{
    error::AppError,
    provider::mts::fcm::{
        FcmSecretState, FcmSecretStore, FcmServiceError, FcmTransport, FcmTransportEvent,
        FirebaseConfig, IncomingCallBootstrap,
    },
    service::push::{
        IncomingCallHandler, PushAuthState, PushRegistrationClient, PushSupervisor,
        PushSupervisorConfig,
    },
    storage::SecretStore,
};
use fcm_push_listener::{Registration, Session, WebPushKeys};
use secrecy::{ExposeSecret, SecretString};
use tokio::sync::{Mutex, Notify, mpsc};
use tokio_util::sync::CancellationToken;

const OLD_TOKEN: &str = "old-fcm-token-private-marker";
const FRESH_TOKEN: &str = "fresh-fcm-token-private-marker";

fn registration(token: &str) -> Registration {
    Registration {
        fcm_token: token.into(),
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

fn state(token: &str) -> FcmSecretState {
    FcmSecretState::new(registration(token), vec![]).unwrap()
}

fn wake_up_body() -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "data": {
            "action": "wake_up",
            "caller": serde_json::json!({
                "id": "private-intercom-id",
                "number": "1001",
                "name": "Private caller label",
                "type": "intercom"
            }).to_string(),
            "call": serde_json::json!({"sessionId": "private-call-session"}).to_string(),
            "camera": serde_json::json!({"streams": []}).to_string(),
            "ts": "1770000000000",
            "sipServer": serde_json::json!({
                "ip": "sip.invalid",
                "sip": "resident",
                "secret": "private-sip-secret"
            }).to_string(),
            "webRTC": serde_json::json!({"ws": "https://webrtc.invalid/ws"}).to_string()
        }
    }))
    .unwrap()
}

#[derive(Default)]
struct MemoryStore {
    state: Mutex<Option<FcmSecretState>>,
    saves: AtomicUsize,
    clears: AtomicUsize,
    persistent_count: AtomicUsize,
}

#[async_trait]
impl FcmSecretStore for MemoryStore {
    async fn load(&self) -> Result<Option<FcmSecretState>, FcmServiceError> {
        Ok(self.state.lock().await.clone())
    }

    async fn save(&self, state: &FcmSecretState) -> Result<(), FcmServiceError> {
        self.persistent_count
            .store(state.persistent_ids().len(), Ordering::SeqCst);
        *self.state.lock().await = Some(state.clone());
        self.saves.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn clear(&self) -> Result<(), FcmServiceError> {
        *self.state.lock().await = None;
        self.clears.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct ToggleAuth(AtomicBool);

#[async_trait]
impl PushAuthState for ToggleAuth {
    async fn is_authenticated(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
struct RecordingProvider {
    lookups: AtomicUsize,
    old_tokens: AtomicUsize,
    fresh_tokens: AtomicUsize,
}

#[async_trait]
impl PushRegistrationClient for RecordingProvider {
    async fn current_user_id(&self) -> Result<String, AppError> {
        self.lookups.fetch_add(1, Ordering::SeqCst);
        Ok("private-user-id".into())
    }

    async fn register_fcm_device_token(
        &self,
        _user_id: &str,
        token: &SecretString,
    ) -> Result<(), AppError> {
        match token.expose_secret() {
            OLD_TOKEN => self.old_tokens.fetch_add(1, Ordering::SeqCst),
            FRESH_TOKEN => self.fresh_tokens.fetch_add(1, Ordering::SeqCst),
            _ => 0,
        };
        Ok(())
    }
}

#[derive(Default)]
struct RecordingHandler {
    calls: AtomicUsize,
    valid_bootstrap: AtomicBool,
    notified: Notify,
}

#[async_trait]
impl IncomingCallHandler for RecordingHandler {
    async fn handle(&self, bootstrap: IncomingCallBootstrap, _cancellation: CancellationToken) {
        self.valid_bootstrap.store(
            bootstrap.caller_name() == "Private caller label"
                && bootstrap.websocket_url().expose_secret() == "wss://webrtc.invalid/ws",
            Ordering::SeqCst,
        );
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.notified.notify_waiters();
    }
}

struct LogoutOnFirstHandler {
    auth: Arc<ToggleAuth>,
    calls: AtomicUsize,
}

#[async_trait]
impl IncomingCallHandler for LogoutOnFirstHandler {
    async fn handle(&self, _bootstrap: IncomingCallBootstrap, _cancellation: CancellationToken) {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            self.auth.0.store(false, Ordering::SeqCst);
        }
    }
}

#[derive(Default)]
struct BlockingHandler {
    calls: AtomicUsize,
    cancellation_seen: AtomicBool,
    completed: AtomicBool,
    started_gate: Option<Arc<Notify>>,
}

#[async_trait]
impl IncomingCallHandler for BlockingHandler {
    async fn handle(&self, _bootstrap: IncomingCallBootstrap, cancellation: CancellationToken) {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(gate) = &self.started_gate {
            gate.notify_waiters();
        }
        cancellation.cancelled().await;
        self.cancellation_seen.store(true, Ordering::SeqCst);
        tokio::task::yield_now().await;
        self.completed.store(true, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct UncooperativeHandler {
    calls: AtomicUsize,
}

#[async_trait]
impl IncomingCallHandler for UncooperativeHandler {
    async fn handle(&self, _bootstrap: IncomingCallBootstrap, _cancellation: CancellationToken) {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::future::pending().await
    }
}

struct ControlledTransport {
    registrations: AtomicUsize,
    listens: AtomicUsize,
    cancellations: AtomicUsize,
    emit_count: usize,
    between_events: Option<Arc<Notify>>,
    reject_old: bool,
}

struct CountOnDrop<'a>(&'a AtomicUsize);

impl Drop for CountOnDrop<'_> {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl FcmTransport for ControlledTransport {
    async fn register(&self, _config: &FirebaseConfig) -> Result<FcmSecretState, FcmServiceError> {
        self.registrations.fetch_add(1, Ordering::SeqCst);
        Ok(state(FRESH_TOKEN))
    }

    async fn listen(
        &self,
        state: &FcmSecretState,
        events: mpsc::Sender<FcmTransportEvent>,
        cancellation: CancellationToken,
    ) -> Result<(), FcmServiceError> {
        self.listens.fetch_add(1, Ordering::SeqCst);
        let _count_cancellation_or_drop = CountOnDrop(&self.cancellations);
        if self.reject_old && state.fcm_token() == OLD_TOKEN {
            return Err(FcmServiceError::Transport);
        }
        for index in 0..self.emit_count {
            if index > 0
                && let Some(gate) = &self.between_events
            {
                gate.notified().await;
            }
            events
                .send(FcmTransportEvent::data(
                    wake_up_body(),
                    Some(format!("persistent-call-{index}")),
                ))
                .await
                .map_err(|_| FcmServiceError::ChannelClosed)?;
        }
        cancellation.cancelled().await;
        Ok(())
    }
}

fn config() -> PushSupervisorConfig {
    PushSupervisorConfig::new(
        Duration::from_millis(5),
        Duration::from_millis(5),
        Duration::from_millis(20),
        Duration::from_millis(1),
    )
    .unwrap()
}

fn supervisor(
    auth: Arc<ToggleAuth>,
    provider: Arc<RecordingProvider>,
    handler: Arc<RecordingHandler>,
    transport: Arc<ControlledTransport>,
    store: Arc<MemoryStore>,
) -> PushSupervisor {
    PushSupervisor::new(
        auth,
        provider,
        handler,
        transport,
        store,
        FirebaseConfig::new(
            "test-app-id",
            "test-project-id",
            SecretString::from("private-api-key"),
        )
        .unwrap(),
        config(),
    )
}

async fn wait_until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while !condition() {
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("condition was not reached");
}

#[tokio::test]
async fn secret_store_adapter_encrypts_round_trips_and_clears_fcm_state() {
    let directory = tempfile::tempdir().unwrap();
    let store = SecretStore::open(directory.path(), Some([31_u8; 32]))
        .await
        .unwrap();
    let original = state(FRESH_TOKEN);

    FcmSecretStore::save(&store, &original).await.unwrap();

    let ciphertext = tokio::fs::read(directory.path().join("call-push.enc"))
        .await
        .unwrap();
    assert!(ciphertext.starts_with(b"VDM1"));
    assert!(!String::from_utf8_lossy(&ciphertext).contains(FRESH_TOKEN));
    let restored = FcmSecretStore::load(&store).await.unwrap().unwrap();
    assert!(restored.fcm_token() == FRESH_TOKEN);

    FcmSecretStore::clear(&store).await.unwrap();
    assert!(FcmSecretStore::load(&store).await.unwrap().is_none());
}

#[tokio::test]
async fn anonymous_supervisor_clears_stale_state_once_without_network_work_or_busy_loop() {
    let auth = Arc::new(ToggleAuth(AtomicBool::new(false)));
    let provider = Arc::new(RecordingProvider::default());
    let handler = Arc::new(RecordingHandler::default());
    let transport = Arc::new(ControlledTransport {
        registrations: AtomicUsize::new(0),
        listens: AtomicUsize::new(0),
        cancellations: AtomicUsize::new(0),
        emit_count: 0,
        between_events: None,
        reject_old: false,
    });
    let store = Arc::new(MemoryStore::default());
    *store.state.lock().await = Some(state(OLD_TOKEN));
    let supervisor = supervisor(
        auth,
        provider.clone(),
        handler,
        transport.clone(),
        store.clone(),
    );
    let cancellation = CancellationToken::new();
    let task = tokio::spawn({
        let cancellation = cancellation.clone();
        async move { supervisor.run(cancellation).await }
    });

    wait_until(|| store.clears.load(Ordering::SeqCst) == 1).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(store.clears.load(Ordering::SeqCst), 1);
    assert_eq!(provider.lookups.load(Ordering::SeqCst), 0);
    assert_eq!(transport.registrations.load(Ordering::SeqCst), 0);
    assert_eq!(transport.listens.load(Ordering::SeqCst), 0);

    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn active_supervisor_registers_token_persists_state_and_forwards_bootstrap() {
    let auth = Arc::new(ToggleAuth(AtomicBool::new(true)));
    let provider = Arc::new(RecordingProvider::default());
    let handler = Arc::new(RecordingHandler::default());
    let transport = Arc::new(ControlledTransport {
        registrations: AtomicUsize::new(0),
        listens: AtomicUsize::new(0),
        cancellations: AtomicUsize::new(0),
        emit_count: 1,
        between_events: None,
        reject_old: false,
    });
    let store = Arc::new(MemoryStore::default());
    let supervisor = supervisor(
        auth,
        provider.clone(),
        handler.clone(),
        transport.clone(),
        store.clone(),
    );
    let cancellation = CancellationToken::new();
    let task = tokio::spawn({
        let cancellation = cancellation.clone();
        async move { supervisor.run(cancellation).await }
    });

    wait_until(|| handler.calls.load(Ordering::SeqCst) == 1).await;
    assert_eq!(transport.registrations.load(Ordering::SeqCst), 1);
    assert_eq!(provider.lookups.load(Ordering::SeqCst), 1);
    assert_eq!(provider.fresh_tokens.load(Ordering::SeqCst), 1);
    assert!(store.saves.load(Ordering::SeqCst) >= 2);
    assert!(handler.valid_bootstrap.load(Ordering::SeqCst));

    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn logout_cancels_listener_clears_state_and_waits_without_restarting_network() {
    let auth = Arc::new(ToggleAuth(AtomicBool::new(true)));
    let provider = Arc::new(RecordingProvider::default());
    let handler = Arc::new(RecordingHandler::default());
    let transport = Arc::new(ControlledTransport {
        registrations: AtomicUsize::new(0),
        listens: AtomicUsize::new(0),
        cancellations: AtomicUsize::new(0),
        emit_count: 0,
        between_events: None,
        reject_old: false,
    });
    let store = Arc::new(MemoryStore::default());
    let supervisor = supervisor(
        auth.clone(),
        provider.clone(),
        handler,
        transport.clone(),
        store.clone(),
    );
    let cancellation = CancellationToken::new();
    let task = tokio::spawn({
        let cancellation = cancellation.clone();
        async move { supervisor.run(cancellation).await }
    });

    wait_until(|| transport.listens.load(Ordering::SeqCst) == 1).await;
    auth.0.store(false, Ordering::SeqCst);
    wait_until(|| {
        transport.cancellations.load(Ordering::SeqCst) == 1
            && store.clears.load(Ordering::SeqCst) == 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(transport.listens.load(Ordering::SeqCst), 1);
    assert_eq!(provider.lookups.load(Ordering::SeqCst), 1);
    assert_eq!(store.clears.load(Ordering::SeqCst), 1);

    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn blocking_handler_does_not_stop_fcm_event_processing() {
    let auth = Arc::new(ToggleAuth(AtomicBool::new(true)));
    let provider = Arc::new(RecordingProvider::default());
    let started_gate = Arc::new(Notify::new());
    let handler = Arc::new(BlockingHandler {
        started_gate: Some(started_gate.clone()),
        ..BlockingHandler::default()
    });
    let transport = Arc::new(ControlledTransport {
        registrations: AtomicUsize::new(0),
        listens: AtomicUsize::new(0),
        cancellations: AtomicUsize::new(0),
        emit_count: 2,
        between_events: Some(started_gate),
        reject_old: false,
    });
    let store = Arc::new(MemoryStore::default());
    let supervisor = PushSupervisor::new(
        auth,
        provider,
        handler.clone(),
        transport,
        store.clone(),
        FirebaseConfig::new(
            "test-app-id",
            "test-project-id",
            SecretString::from("private-api-key"),
        )
        .unwrap(),
        config(),
    );
    let cancellation = CancellationToken::new();
    let task = tokio::spawn({
        let cancellation = cancellation.clone();
        async move { supervisor.run(cancellation).await }
    });

    wait_until(|| handler.calls.load(Ordering::SeqCst) == 1).await;
    wait_until(|| store.persistent_count.load(Ordering::SeqCst) == 2).await;
    assert_eq!(handler.calls.load(Ordering::SeqCst), 1);

    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(handler.cancellation_seen.load(Ordering::SeqCst));
    assert!(handler.completed.load(Ordering::SeqCst));
}

#[tokio::test]
async fn logout_cancels_and_awaits_handler_before_clearing_push_state() {
    let auth = Arc::new(ToggleAuth(AtomicBool::new(true)));
    let provider = Arc::new(RecordingProvider::default());
    let handler = Arc::new(BlockingHandler::default());
    let transport = Arc::new(ControlledTransport {
        registrations: AtomicUsize::new(0),
        listens: AtomicUsize::new(0),
        cancellations: AtomicUsize::new(0),
        emit_count: 1,
        between_events: None,
        reject_old: false,
    });
    let store = Arc::new(MemoryStore::default());
    let supervisor = PushSupervisor::new(
        auth.clone(),
        provider,
        handler.clone(),
        transport,
        store.clone(),
        FirebaseConfig::new(
            "test-app-id",
            "test-project-id",
            SecretString::from("private-api-key"),
        )
        .unwrap(),
        config(),
    );
    let cancellation = CancellationToken::new();
    let task = tokio::spawn({
        let cancellation = cancellation.clone();
        async move { supervisor.run(cancellation).await }
    });

    wait_until(|| handler.calls.load(Ordering::SeqCst) == 1).await;
    auth.0.store(false, Ordering::SeqCst);
    wait_until(|| store.clears.load(Ordering::SeqCst) == 1).await;
    assert!(handler.cancellation_seen.load(Ordering::SeqCst));
    assert!(handler.completed.load(Ordering::SeqCst));

    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test(start_paused = true)]
async fn uncooperative_handler_gets_fifteen_seconds_before_bounded_abort() {
    let auth = Arc::new(ToggleAuth(AtomicBool::new(true)));
    let provider = Arc::new(RecordingProvider::default());
    let handler = Arc::new(UncooperativeHandler::default());
    let transport = Arc::new(ControlledTransport {
        registrations: AtomicUsize::new(0),
        listens: AtomicUsize::new(0),
        cancellations: AtomicUsize::new(0),
        emit_count: 1,
        between_events: None,
        reject_old: false,
    });
    let store = Arc::new(MemoryStore::default());
    let supervisor = PushSupervisor::new(
        auth,
        provider,
        handler.clone(),
        transport,
        store,
        FirebaseConfig::new(
            "test-app-id",
            "test-project-id",
            SecretString::from("private-api-key"),
        )
        .unwrap(),
        config(),
    );
    let cancellation = CancellationToken::new();
    let task = tokio::spawn({
        let cancellation = cancellation.clone();
        async move { supervisor.run(cancellation).await }
    });

    while handler.calls.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    cancellation.cancel();
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(14)).await;
    tokio::task::yield_now().await;
    assert!(!task.is_finished());

    tokio::time::advance(Duration::from_secs(1)).await;
    tokio::task::yield_now().await;
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn queued_bootstraps_are_not_dispatched_after_logout() {
    let auth = Arc::new(ToggleAuth(AtomicBool::new(true)));
    let provider = Arc::new(RecordingProvider::default());
    let handler = Arc::new(LogoutOnFirstHandler {
        auth: auth.clone(),
        calls: AtomicUsize::new(0),
    });
    let transport = Arc::new(ControlledTransport {
        registrations: AtomicUsize::new(0),
        listens: AtomicUsize::new(0),
        cancellations: AtomicUsize::new(0),
        emit_count: 2,
        between_events: None,
        reject_old: false,
    });
    let store = Arc::new(MemoryStore::default());
    let supervisor = PushSupervisor::new(
        auth,
        provider,
        handler.clone(),
        transport,
        store.clone(),
        FirebaseConfig::new(
            "test-app-id",
            "test-project-id",
            SecretString::from("private-api-key"),
        )
        .unwrap(),
        config(),
    );
    let cancellation = CancellationToken::new();
    let task = tokio::spawn({
        let cancellation = cancellation.clone();
        async move { supervisor.run(cancellation).await }
    });

    wait_until(|| store.clears.load(Ordering::SeqCst) == 1).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(handler.calls.load(Ordering::SeqCst), 1);

    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn stale_saved_registration_is_cleared_once_then_replaced_with_fresh_registration() {
    let auth = Arc::new(ToggleAuth(AtomicBool::new(true)));
    let provider = Arc::new(RecordingProvider::default());
    let handler = Arc::new(RecordingHandler::default());
    let transport = Arc::new(ControlledTransport {
        registrations: AtomicUsize::new(0),
        listens: AtomicUsize::new(0),
        cancellations: AtomicUsize::new(0),
        emit_count: 1,
        between_events: None,
        reject_old: true,
    });
    let store = Arc::new(MemoryStore::default());
    *store.state.lock().await = Some(state(OLD_TOKEN));
    let supervisor = supervisor(
        auth,
        provider.clone(),
        handler.clone(),
        transport.clone(),
        store.clone(),
    );
    let cancellation = CancellationToken::new();
    let task = tokio::spawn({
        let cancellation = cancellation.clone();
        async move { supervisor.run(cancellation).await }
    });

    wait_until(|| handler.calls.load(Ordering::SeqCst) == 1).await;
    assert_eq!(store.clears.load(Ordering::SeqCst), 1);
    assert_eq!(transport.registrations.load(Ordering::SeqCst), 1);
    assert_eq!(provider.old_tokens.load(Ordering::SeqCst), 1);
    assert_eq!(provider.fresh_tokens.load(Ordering::SeqCst), 1);
    assert!(transport.listens.load(Ordering::SeqCst) >= 4);

    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
