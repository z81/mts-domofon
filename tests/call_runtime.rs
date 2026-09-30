use std::sync::{
    Arc, Mutex as StdMutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use async_trait::async_trait;
use domru_mts::calls::runtime::{
    CallRuntimeClock, RuntimeConfig, RuntimeError, SipCallRuntime, SipConnectionFactory,
};
use domru_mts::calls::sip::{
    SipMessage, SipMethod, SipRequest, SipResponse, SipTransport, TransportError,
    WssTransportConfig,
};
use domru_mts::provider::mts::call::{CallActionProvider, RemoteCallAction};
use domru_mts::provider::mts::fcm::{IncomingCallBootstrap, parse_incoming_call_bootstrap};
use domru_mts::service::calls::{CallOpenProvider, CallService, CallView, CallViewPhase};
use secrecy::SecretString;
use tokio::{
    sync::{Mutex, mpsc, oneshot},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

const OFFER_SDP: &str = "v=0\r\nm=audio 40000 RTP/AVP 111\r\na=rtpmap:111 opus/48000/2\r\n";
const ANSWER_SDP: &str = "v=0\r\nm=audio 41000 RTP/AVP 111\r\na=rtpmap:111 opus/48000/2\r\n";
const NOW_MILLIS: u64 = 1_770_000_123_456;

fn bootstrap() -> IncomingCallBootstrap {
    bootstrap_at(NOW_MILLIS)
}

fn bootstrap_at(timestamp_millis: u64) -> IncomingCallBootstrap {
    let body = serde_json::to_vec(&serde_json::json!({
        "data": {
            "action": "wake_up",
            "caller": serde_json::json!({
                "id": "intercom-42",
                "number": "1001",
                "name": "Front door",
                "type": "intercom"
            }).to_string(),
            "call": serde_json::json!({"sessionId": "call-session-secret"}).to_string(),
            "camera": serde_json::json!({"streams": []}).to_string(),
            "ts": timestamp_millis.to_string(),
            "sipServer": serde_json::json!({
                "ip": "sip.example.invalid",
                "sip": "resident-1001",
                "secret": "sip-password-secret"
            }).to_string(),
            "webRTC": serde_json::json!({
                "ws": "https://webrtc.example.invalid/per-call?token=wss-secret"
            }).to_string()
        }
    }))
    .expect("wake_up JSON");
    parse_incoming_call_bootstrap(&body)
        .expect("valid payload")
        .expect("wake_up bootstrap")
}

struct ScriptedTransport {
    inbound: mpsc::Receiver<SipMessage>,
    faults: mpsc::Receiver<TransportError>,
    outbound: mpsc::Sender<SipMessage>,
    close_count: Arc<AtomicUsize>,
}

#[async_trait]
impl SipTransport for ScriptedTransport {
    async fn send(&mut self, message: &SipMessage) -> Result<(), TransportError> {
        self.outbound
            .send(message.clone())
            .await
            .map_err(|_| TransportError::Closed)
    }

    async fn receive(&mut self) -> Result<SipMessage, TransportError> {
        tokio::select! {
            biased;
            fault = self.faults.recv() => fault.ok_or(TransportError::Closed).and_then(Err),
            inbound = self.inbound.recv() => inbound.ok_or(TransportError::Closed),
        }
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        self.close_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct ScriptedFactory {
    transport: StdMutex<Option<ScriptedTransport>>,
    connected_url: StdMutex<Option<oneshot::Sender<String>>>,
    connect_count: Arc<AtomicUsize>,
    fail_connects: AtomicUsize,
}

#[async_trait]
impl SipConnectionFactory for ScriptedFactory {
    async fn connect(
        &self,
        config: WssTransportConfig,
    ) -> Result<Box<dyn SipTransport>, RuntimeError> {
        self.connect_count.fetch_add(1, Ordering::SeqCst);
        let request = config.handshake_request().map_err(RuntimeError::from)?;
        if self
            .fail_connects
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return Err(RuntimeError::ConnectionUnavailable);
        }
        if let Some(sender) = self.connected_url.lock().expect("factory lock").take() {
            let _ = sender.send(request.uri().to_string());
        }
        self.transport
            .lock()
            .expect("factory lock")
            .take()
            .map(|transport| Box::new(transport) as Box<dyn SipTransport>)
            .ok_or(RuntimeError::ConnectionUnavailable)
    }
}

#[derive(Default)]
struct NoopActions;

#[async_trait]
impl CallActionProvider for NoopActions {
    async fn action(
        &self,
        _call_session_id: &SecretString,
        _action: RemoteCallAction,
    ) -> Result<(), domru_mts::error::AppError> {
        Ok(())
    }
}

struct BlockingActions {
    started: mpsc::Sender<RemoteCallAction>,
    release: Mutex<mpsc::Receiver<()>>,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl CallActionProvider for BlockingActions {
    async fn action(
        &self,
        _call_session_id: &SecretString,
        action: RemoteCallAction,
    ) -> Result<(), domru_mts::error::AppError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.send(action).await.expect("action observer");
        self.release
            .lock()
            .await
            .recv()
            .await
            .expect("action release");
        Ok(())
    }
}

#[derive(Default)]
struct NoopOpen;

#[async_trait]
impl CallOpenProvider for NoopOpen {
    async fn open(
        &self,
        _intercom_id: &SecretString,
        _call_session_id: &SecretString,
    ) -> Result<(), domru_mts::error::AppError> {
        Ok(())
    }
}

struct Harness {
    runtime: Arc<SipCallRuntime>,
    service: Arc<CallService>,
    inbound: mpsc::Sender<SipMessage>,
    faults: mpsc::Sender<TransportError>,
    outbound: mpsc::Receiver<SipMessage>,
    connected_url: oneshot::Receiver<String>,
    connect_count: Arc<AtomicUsize>,
    close_count: Arc<AtomicUsize>,
}

struct FixedClock;

impl CallRuntimeClock for FixedClock {
    fn now_millis(&self) -> u64 {
        NOW_MILLIS
    }
}

fn harness(config: RuntimeConfig) -> Harness {
    harness_with_actions(config, Arc::new(NoopActions))
}

fn harness_with_actions(config: RuntimeConfig, actions: Arc<dyn CallActionProvider>) -> Harness {
    harness_with_options(config, actions, 0)
}

fn harness_with_options(
    config: RuntimeConfig,
    actions: Arc<dyn CallActionProvider>,
    fail_connects: usize,
) -> Harness {
    let (inbound_tx, inbound_rx) = mpsc::channel(16);
    let (fault_tx, fault_rx) = mpsc::channel(16);
    let (outbound_tx, outbound_rx) = mpsc::channel(16);
    let (url_tx, url_rx) = oneshot::channel();
    let connect_count = Arc::new(AtomicUsize::new(0));
    let close_count = Arc::new(AtomicUsize::new(0));
    let factory = Arc::new(ScriptedFactory {
        transport: StdMutex::new(Some(ScriptedTransport {
            inbound: inbound_rx,
            faults: fault_rx,
            outbound: outbound_tx,
            close_count: close_count.clone(),
        })),
        connected_url: StdMutex::new(Some(url_tx)),
        connect_count: connect_count.clone(),
        fail_connects: AtomicUsize::new(fail_connects),
    });
    let runtime = Arc::new(
        SipCallRuntime::new_with_clock(factory, actions, Arc::new(FixedClock), config)
            .expect("runtime config"),
    );
    let service = Arc::new(CallService::new(runtime.clone(), Arc::new(NoopOpen)));
    runtime
        .attach_service(Arc::downgrade(&service))
        .expect("one-time service attachment");
    Harness {
        runtime,
        service,
        inbound: inbound_tx,
        faults: fault_tx,
        outbound: outbound_rx,
        connected_url: url_rx,
        connect_count,
        close_count,
    }
}

#[tokio::test]
async fn stale_or_implausibly_future_bootstrap_is_rejected_before_wss() {
    let stale_harness = harness(test_config());
    let stale = stale_harness
        .runtime
        .handle_bootstrap(
            bootstrap_at(NOW_MILLIS - Duration::from_secs(181).as_millis() as u64),
            CancellationToken::new(),
        )
        .await;
    assert!(matches!(stale, Err(RuntimeError::StaleBootstrap)));
    assert_eq!(stale_harness.connect_count.load(Ordering::SeqCst), 0);

    let future_harness = harness(test_config());
    let future = future_harness
        .runtime
        .handle_bootstrap(
            bootstrap_at(NOW_MILLIS + Duration::from_secs(61).as_millis() as u64),
            CancellationToken::new(),
        )
        .await;
    assert!(matches!(future, Err(RuntimeError::FutureBootstrap)));
    assert_eq!(future_harness.connect_count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn bootstrap_before_service_attachment_fails_safely_and_attachment_is_one_time() {
    let connect_count = Arc::new(AtomicUsize::new(0));
    let runtime = Arc::new(
        SipCallRuntime::new_with_clock(
            Arc::new(ScriptedFactory {
                transport: StdMutex::new(None),
                connected_url: StdMutex::new(None),
                connect_count: connect_count.clone(),
                fail_connects: AtomicUsize::new(0),
            }),
            Arc::new(NoopActions),
            Arc::new(FixedClock),
            test_config(),
        )
        .expect("runtime"),
    );
    assert!(matches!(
        runtime
            .handle_bootstrap(bootstrap(), CancellationToken::new())
            .await,
        Err(RuntimeError::ServiceUnavailable)
    ));
    assert_eq!(connect_count.load(Ordering::SeqCst), 0);

    let service = Arc::new(CallService::new(runtime.clone(), Arc::new(NoopOpen)));
    runtime
        .attach_service(Arc::downgrade(&service))
        .expect("first attachment");
    assert!(matches!(
        runtime.attach_service(Arc::downgrade(&service)),
        Err(RuntimeError::ServiceAlreadyAttached)
    ));
}

#[tokio::test]
async fn concurrent_duplicate_bootstrap_is_rejected_before_a_second_connect() {
    let harness = harness(test_config());
    let runtime = harness.runtime.clone();
    let cancellation = CancellationToken::new();
    let task_cancellation = cancellation.clone();
    let task = tokio::spawn(async move {
        runtime
            .handle_bootstrap(bootstrap(), task_cancellation)
            .await
    });
    let _ = harness.connected_url.await.expect("first connection");

    let duplicate = harness
        .runtime
        .handle_bootstrap(bootstrap(), CancellationToken::new())
        .await;
    assert!(matches!(duplicate, Err(RuntimeError::CallAlreadyActive)));
    assert_eq!(harness.connect_count.load(Ordering::SeqCst), 1);

    cancellation.cancel();
    assert!(task.await.expect("runtime task").is_ok());
}

fn response_to(request: &SipRequest, status: u16, reason: &str) -> SipResponse {
    let mut response = SipResponse::new(status, reason).expect("response");
    for name in ["Via", "From", "To", "Call-ID", "CSeq"] {
        response
            .headers_mut()
            .push(name, request.headers().get(name).expect("request header"))
            .expect("response header");
    }
    response
}

fn incoming_invite() -> SipRequest {
    let mut request =
        SipRequest::new(SipMethod::Invite, "sip:resident@runtime.invalid").expect("INVITE");
    for (name, value) in [
        (
            "Via",
            "SIP/2.0/WSS server.example;branch=z9hG4bK-runtime;rport",
        ),
        ("From", "<sip:door@sip.example>;tag=remote-tag"),
        ("To", "<sip:resident@sip.example>"),
        ("Call-ID", "runtime-call-123"),
        ("CSeq", "42 INVITE"),
        ("Contact", "<sip:door@server.example;transport=wss>"),
    ] {
        request
            .headers_mut()
            .push(name, value)
            .expect("INVITE header");
    }
    request
        .set_body("application/sdp", OFFER_SDP.as_bytes())
        .expect("offer SDP");
    request
}

fn test_config() -> RuntimeConfig {
    RuntimeConfig::new(
        8,
        Duration::from_secs(5),
        Duration::from_secs(5),
        Duration::from_secs(30),
        Duration::from_secs(5),
    )
    .expect("bounded runtime config")
}

async fn start_ringing(
    harness: &mut Harness,
    cancellation: &CancellationToken,
) -> (JoinHandle<Result<(), RuntimeError>>, CallView, SipRequest) {
    let runtime = harness.runtime.clone();
    let task_cancellation = cancellation.clone();
    let task = tokio::spawn(async move {
        runtime
            .handle_bootstrap(bootstrap(), task_cancellation)
            .await
    });
    assert_eq!(
        (&mut harness.connected_url).await.expect("connection URL"),
        "wss://webrtc.example.invalid/per-call?token=wss-secret"
    );
    let SipMessage::Request(initial) = harness.outbound.recv().await.expect("REGISTER") else {
        panic!("expected REGISTER");
    };
    let mut challenge = response_to(&initial, 401, "Unauthorized");
    challenge
        .headers_mut()
        .push(
            "WWW-Authenticate",
            "Digest realm=\"runtime\",nonce=\"abcdef\",algorithm=MD5",
        )
        .expect("challenge");
    harness
        .inbound
        .send(SipMessage::Response(challenge))
        .await
        .expect("401");
    let SipMessage::Request(authenticated) = harness.outbound.recv().await.expect("REGISTER")
    else {
        panic!("expected authenticated REGISTER");
    };
    harness
        .inbound
        .send(SipMessage::Response(response_to(&authenticated, 200, "OK")))
        .await
        .expect("200");
    let invite = incoming_invite();
    harness
        .inbound
        .send(SipMessage::Request(invite.clone()))
        .await
        .expect("INVITE");
    let SipMessage::Response(ringing) = harness.outbound.recv().await.expect("180") else {
        panic!("expected 180");
    };
    assert_eq!(ringing.status(), 180);
    let current = harness
        .service
        .current()
        .await
        .expect("call service")
        .expect("observed call");
    (task, current, invite)
}

fn ack_for(response: &SipResponse, invite: &SipRequest) -> SipRequest {
    in_dialog_request(SipMethod::Ack, 42, response, invite)
}

fn bye_for(response: &SipResponse, invite: &SipRequest) -> SipRequest {
    in_dialog_request(SipMethod::Bye, 43, response, invite)
}

fn in_dialog_request(
    method: SipMethod,
    cseq: u32,
    response: &SipResponse,
    invite: &SipRequest,
) -> SipRequest {
    let mut request =
        SipRequest::new(method.clone(), "sip:resident@runtime.invalid").expect("in-dialog request");
    for (name, value) in [
        (
            "Via",
            "SIP/2.0/WSS server.example;branch=z9hG4bK-next;rport",
        ),
        ("From", invite.headers().get("From").expect("From")),
        ("To", response.headers().get("To").expect("tagged To")),
        ("Call-ID", invite.headers().get("Call-ID").expect("Call-ID")),
    ] {
        request
            .headers_mut()
            .push(name, value)
            .expect("in-dialog header");
    }
    request
        .headers_mut()
        .push("CSeq", &format!("{cseq} {}", method.as_str()))
        .expect("CSeq");
    request
}

fn cancel_for(invite: &SipRequest) -> SipRequest {
    let mut request =
        SipRequest::new(SipMethod::Cancel, "sip:resident@runtime.invalid").expect("CANCEL");
    for name in ["Via", "From", "To", "Call-ID"] {
        request
            .headers_mut()
            .push(name, invite.headers().get(name).expect("INVITE header"))
            .expect("CANCEL header");
    }
    request
        .headers_mut()
        .push("CSeq", "42 CANCEL")
        .expect("CSeq");
    request
}

fn options_request() -> SipRequest {
    let mut request =
        SipRequest::new(SipMethod::Options, "sip:resident@runtime.invalid").expect("OPTIONS");
    for (name, value) in [
        (
            "Via",
            "SIP/2.0/WSS server.example;branch=z9hG4bK-options;rport",
        ),
        ("From", "<sip:server@example>;tag=options-remote"),
        ("To", "<sip:resident@example>"),
        ("Call-ID", "options-call"),
        ("CSeq", "1 OPTIONS"),
    ] {
        request
            .headers_mut()
            .push(name, value)
            .expect("OPTIONS header");
    }
    request
}

async fn wait_for_phase(harness: &Harness, phase: CallViewPhase) -> CallView {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Some(view) = harness.service.current().await.expect("call service")
                && view.phase == phase
            {
                return view;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("call phase transition")
}

#[tokio::test]
async fn connects_registers_rings_and_publishes_the_invite() {
    let mut harness = harness(test_config());
    let cancellation = CancellationToken::new();
    let runtime = harness.runtime.clone();
    let task_cancellation = cancellation.clone();
    let task = tokio::spawn(async move {
        runtime
            .handle_bootstrap(bootstrap(), task_cancellation)
            .await
    });

    assert_eq!(
        harness.connected_url.await.expect("connection URL"),
        "wss://webrtc.example.invalid/per-call?token=wss-secret"
    );
    let SipMessage::Request(initial) = harness.outbound.recv().await.expect("REGISTER") else {
        panic!("expected REGISTER");
    };
    assert_eq!(initial.method(), &SipMethod::Register);
    let mut challenge = response_to(&initial, 401, "Unauthorized");
    challenge
        .headers_mut()
        .push(
            "WWW-Authenticate",
            "Digest realm=\"runtime\",nonce=\"abcdef\",algorithm=MD5",
        )
        .expect("challenge");
    harness
        .inbound
        .send(SipMessage::Response(challenge))
        .await
        .expect("401");

    let SipMessage::Request(authenticated) = harness.outbound.recv().await.expect("REGISTER")
    else {
        panic!("expected authenticated REGISTER");
    };
    assert!(authenticated.headers().get("Authorization").is_some());
    let ok = response_to(&authenticated, 200, "OK");
    harness
        .inbound
        .send(SipMessage::Response(ok))
        .await
        .expect("200");
    harness
        .inbound
        .send(SipMessage::Request(incoming_invite()))
        .await
        .expect("INVITE");

    let SipMessage::Response(ringing) = harness.outbound.recv().await.expect("180") else {
        panic!("expected 180");
    };
    assert_eq!(ringing.status(), 180);
    let current = harness
        .service
        .current()
        .await
        .expect("call service")
        .expect("observed call");
    assert_eq!(current.phase, CallViewPhase::Ringing);
    assert_eq!(current.caller_name, "Front door");
    assert_eq!(current.remote_sdp, OFFER_SDP);

    cancellation.cancel();
    assert!(task.await.expect("runtime task").is_ok());
    assert!(harness.service.current().await.expect("service").is_none());
}

#[tokio::test(start_paused = true)]
async fn answer_sends_sdp_and_ack_marks_the_call_connected() {
    let mut harness = harness(test_config());
    let cancellation = CancellationToken::new();
    let (task, ringing, invite) = start_ringing(&mut harness, &cancellation).await;

    let connecting = harness
        .service
        .answer(&ringing.id, ringing.revision, ANSWER_SDP)
        .await
        .expect("answer call");
    assert_eq!(connecting.phase, CallViewPhase::Connecting);
    let SipMessage::Response(answer) = harness.outbound.recv().await.expect("200 answer") else {
        panic!("expected answer response");
    };
    assert_eq!(answer.status(), 200);
    assert_eq!(answer.body(), ANSWER_SDP.as_bytes());
    assert_eq!(
        answer.headers().get("Content-Type"),
        Some("application/sdp")
    );

    tokio::time::advance(Duration::from_millis(500)).await;
    let retry = harness.outbound.recv().await.expect("answer retry");
    assert_eq!(retry, SipMessage::Response(answer.clone()));
    harness
        .inbound
        .send(SipMessage::Request(invite.clone()))
        .await
        .expect("repeated INVITE");
    assert_eq!(
        harness.outbound.recv().await.expect("cached INVITE answer"),
        SipMessage::Response(answer.clone())
    );

    harness
        .inbound
        .send(SipMessage::Request(ack_for(&answer, &invite)))
        .await
        .expect("ACK");
    let active = wait_for_phase(&harness, CallViewPhase::Active).await;
    assert!(active.can_open);
    tokio::time::advance(Duration::from_secs(5)).await;
    assert!(matches!(
        harness.outbound.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));

    cancellation.cancel();
    let SipMessage::Request(bye) = harness.outbound.recv().await.expect("cancellation BYE") else {
        panic!("expected BYE");
    };
    assert_eq!(bye.method(), &SipMethod::Bye);
    assert!(task.await.expect("runtime task").is_ok());
    assert!(harness.service.current().await.expect("service").is_none());
}

#[tokio::test]
async fn decline_sends_600_and_ends_the_runtime() {
    let (started_tx, mut started_rx) = mpsc::channel(1);
    let (release_tx, release_rx) = mpsc::channel(1);
    let calls = Arc::new(AtomicUsize::new(0));
    let actions = Arc::new(BlockingActions {
        started: started_tx,
        release: Mutex::new(release_rx),
        calls: calls.clone(),
    });
    let mut harness = harness_with_actions(test_config(), actions);
    let cancellation = CancellationToken::new();
    let (task, ringing, _) = start_ringing(&mut harness, &cancellation).await;

    harness
        .service
        .decline(&ringing.id, ringing.revision)
        .await
        .expect("decline call");
    let SipMessage::Response(response) = harness.outbound.recv().await.expect("600") else {
        panic!("expected decline response");
    };
    assert_eq!(response.status(), 600);
    assert!(task.await.expect("runtime task").is_ok());
    assert!(harness.service.current().await.expect("service").is_none());
    assert_eq!(started_rx.recv().await, Some(RemoteCallAction::Unwanted));
    release_tx
        .send(())
        .await
        .expect("release peer notification");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn ignore_calls_rest_once_before_sending_486() {
    let (started_tx, mut started_rx) = mpsc::channel(1);
    let (release_tx, release_rx) = mpsc::channel(1);
    let calls = Arc::new(AtomicUsize::new(0));
    let actions = Arc::new(BlockingActions {
        started: started_tx,
        release: Mutex::new(release_rx),
        calls: calls.clone(),
    });
    let mut harness = harness_with_actions(test_config(), actions);
    let cancellation = CancellationToken::new();
    let (task, ringing, _) = start_ringing(&mut harness, &cancellation).await;
    let service = harness.service.clone();
    let ignore = tokio::spawn(async move { service.ignore(&ringing.id, ringing.revision).await });

    assert_eq!(started_rx.recv().await, Some(RemoteCallAction::Ignore));
    assert!(matches!(
        harness.outbound.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    release_tx.send(()).await.expect("release REST action");
    ignore.await.expect("ignore task").expect("ignore command");
    let SipMessage::Response(response) = harness.outbound.recv().await.expect("486") else {
        panic!("expected ignore response");
    };
    assert_eq!(response.status(), 486);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(task.await.expect("runtime task").is_ok());
}

#[tokio::test]
async fn local_hangup_before_ack_waits_for_ack_then_sends_one_bye() {
    let mut harness = harness(test_config());
    let cancellation = CancellationToken::new();
    let (task, ringing, invite) = start_ringing(&mut harness, &cancellation).await;
    let connecting = harness
        .service
        .answer(&ringing.id, ringing.revision, ANSWER_SDP)
        .await
        .expect("answer call");
    let SipMessage::Response(answer) = harness.outbound.recv().await.expect("200 answer") else {
        panic!("expected answer response");
    };
    let service = harness.service.clone();
    let hangup_id = connecting.id.clone();
    let hangup = tokio::spawn(async move { service.hangup(&hangup_id, connecting.revision).await });
    tokio::task::yield_now().await;
    assert!(matches!(
        harness.outbound.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));

    harness
        .inbound
        .send(SipMessage::Request(ack_for(&answer, &invite)))
        .await
        .expect("ACK");
    let SipMessage::Request(bye) = harness.outbound.recv().await.expect("BYE") else {
        panic!("expected BYE");
    };
    assert_eq!(bye.method(), &SipMethod::Bye);
    hangup.await.expect("hangup task").expect("hangup command");
    assert!(task.await.expect("runtime task").is_ok());
    assert!(harness.service.current().await.expect("service").is_none());
}

#[tokio::test]
async fn cancel_gets_200_and_terminates_invite_with_487() {
    let mut harness = harness(test_config());
    let cancellation = CancellationToken::new();
    let (task, _, invite) = start_ringing(&mut harness, &cancellation).await;
    harness
        .inbound
        .send(SipMessage::Request(cancel_for(&invite)))
        .await
        .expect("CANCEL");

    let SipMessage::Response(ok) = harness.outbound.recv().await.expect("CANCEL 200") else {
        panic!("expected CANCEL 200");
    };
    let SipMessage::Response(terminated) = harness.outbound.recv().await.expect("INVITE 487")
    else {
        panic!("expected INVITE 487");
    };
    assert_eq!(ok.status(), 200);
    assert_eq!(terminated.status(), 487);
    assert!(task.await.expect("runtime task").is_ok());
    assert!(harness.service.current().await.expect("service").is_none());
}

#[tokio::test]
async fn remote_bye_gets_200_and_ends_the_call() {
    let mut harness = harness(test_config());
    let cancellation = CancellationToken::new();
    let (task, ringing, invite) = start_ringing(&mut harness, &cancellation).await;
    harness
        .service
        .answer(&ringing.id, ringing.revision, ANSWER_SDP)
        .await
        .expect("answer call");
    let SipMessage::Response(answer) = harness.outbound.recv().await.expect("200 answer") else {
        panic!("expected answer response");
    };
    harness
        .inbound
        .send(SipMessage::Request(ack_for(&answer, &invite)))
        .await
        .expect("ACK");
    wait_for_phase(&harness, CallViewPhase::Active).await;
    harness
        .inbound
        .send(SipMessage::Request(bye_for(&answer, &invite)))
        .await
        .expect("BYE");

    let SipMessage::Response(ok) = harness.outbound.recv().await.expect("BYE 200") else {
        panic!("expected BYE 200");
    };
    assert_eq!(ok.status(), 200);
    assert!(task.await.expect("runtime task").is_ok());
    assert!(harness.service.current().await.expect("service").is_none());
}

#[tokio::test]
async fn options_is_answered_without_changing_the_dialog() {
    let mut harness = harness(test_config());
    let cancellation = CancellationToken::new();
    let (task, _, _) = start_ringing(&mut harness, &cancellation).await;
    harness
        .inbound
        .send(SipMessage::Request(options_request()))
        .await
        .expect("OPTIONS");
    let SipMessage::Response(ok) = harness.outbound.recv().await.expect("OPTIONS 200") else {
        panic!("expected OPTIONS 200");
    };
    assert_eq!(ok.status(), 200);
    assert_eq!(
        harness
            .service
            .current()
            .await
            .expect("service")
            .unwrap()
            .phase,
        CallViewPhase::Ringing
    );

    cancellation.cancel();
    let SipMessage::Response(timeout) = harness.outbound.recv().await.expect("teardown 480") else {
        panic!("expected teardown 480");
    };
    assert_eq!(timeout.status(), 480);
    assert!(task.await.expect("runtime task").is_ok());
}

#[tokio::test(start_paused = true)]
async fn ringing_timeout_sends_480_and_ends_the_call() {
    let config = RuntimeConfig::new(
        8,
        Duration::from_secs(5),
        Duration::from_secs(5),
        Duration::from_secs(30),
        Duration::from_secs(5),
    )
    .expect("runtime config");
    let mut harness = harness(config);
    let cancellation = CancellationToken::new();
    let (task, _, _) = start_ringing(&mut harness, &cancellation).await;
    tokio::time::advance(Duration::from_secs(31)).await;
    let SipMessage::Response(timeout) = harness.outbound.recv().await.expect("480") else {
        panic!("expected timeout response");
    };
    assert_eq!(timeout.status(), 480);
    assert!(task.await.expect("runtime task").is_ok());
    assert!(harness.service.current().await.expect("service").is_none());
}

#[tokio::test]
async fn cancellation_while_ringing_sends_480_before_teardown() {
    let mut harness = harness(test_config());
    let cancellation = CancellationToken::new();
    let (task, _, _) = start_ringing(&mut harness, &cancellation).await;
    cancellation.cancel();
    let SipMessage::Response(response) = harness.outbound.recv().await.expect("480") else {
        panic!("expected ringing teardown response");
    };
    assert_eq!(response.status(), 480);
    assert!(task.await.expect("runtime task").is_ok());
    assert_eq!(harness.close_count.load(Ordering::SeqCst), 1);
    assert!(harness.service.current().await.expect("service").is_none());
}

#[tokio::test]
async fn cancellation_after_ack_sends_exactly_one_bye_before_close() {
    let mut harness = harness(test_config());
    let cancellation = CancellationToken::new();
    let (task, ringing, invite) = start_ringing(&mut harness, &cancellation).await;
    harness
        .service
        .answer(&ringing.id, ringing.revision, ANSWER_SDP)
        .await
        .expect("answer");
    let SipMessage::Response(answer) = harness.outbound.recv().await.expect("200 answer") else {
        panic!("expected 200 answer");
    };
    harness
        .inbound
        .send(SipMessage::Request(ack_for(&answer, &invite)))
        .await
        .expect("ACK");
    wait_for_phase(&harness, CallViewPhase::Active).await;

    cancellation.cancel();
    let SipMessage::Request(bye) = harness.outbound.recv().await.expect("teardown BYE") else {
        panic!("expected teardown BYE");
    };
    assert_eq!(bye.method(), &SipMethod::Bye);
    assert!(task.await.expect("runtime task").is_ok());
    assert!(matches!(
        harness.outbound.try_recv(),
        Err(mpsc::error::TryRecvError::Empty) | Err(mpsc::error::TryRecvError::Disconnected)
    ));
    assert_eq!(harness.close_count.load(Ordering::SeqCst), 1);
    assert!(harness.service.current().await.expect("service").is_none());
}

#[tokio::test(start_paused = true)]
async fn retries_a_failed_initial_connect_before_any_invite() {
    let mut harness = harness_with_options(test_config(), Arc::new(NoopActions), 1);
    let cancellation = CancellationToken::new();
    let (task, _, _) = start_ringing(&mut harness, &cancellation).await;
    assert_eq!(harness.connect_count.load(Ordering::SeqCst), 2);

    cancellation.cancel();
    let SipMessage::Response(response) = harness.outbound.recv().await.expect("teardown 480")
    else {
        panic!("expected ringing teardown response");
    };
    assert_eq!(response.status(), 480);
    assert!(task.await.expect("runtime task").is_ok());
}

#[tokio::test]
async fn read_timeout_is_a_tick_during_registration_invite_wait_and_active_dialog() {
    let mut harness = harness(test_config());
    let runtime = harness.runtime.clone();
    let cancellation = CancellationToken::new();
    let task_cancellation = cancellation.clone();
    let task = tokio::spawn(async move {
        runtime
            .handle_bootstrap(bootstrap(), task_cancellation)
            .await
    });
    assert_eq!(
        (&mut harness.connected_url).await.expect("connection URL"),
        "wss://webrtc.example.invalid/per-call?token=wss-secret"
    );
    let SipMessage::Request(initial) = harness.outbound.recv().await.expect("REGISTER") else {
        panic!("expected REGISTER");
    };
    harness
        .faults
        .send(TransportError::ReadTimeout)
        .await
        .expect("registration tick");
    tokio::task::yield_now().await;
    let mut challenge = response_to(&initial, 401, "Unauthorized");
    challenge
        .headers_mut()
        .push(
            "WWW-Authenticate",
            "Digest realm=\"runtime\",nonce=\"abcdef\",algorithm=MD5",
        )
        .expect("challenge");
    harness
        .inbound
        .send(SipMessage::Response(challenge))
        .await
        .expect("401");
    let SipMessage::Request(authenticated) = harness.outbound.recv().await.expect("REGISTER")
    else {
        panic!("expected authenticated REGISTER");
    };
    harness
        .inbound
        .send(SipMessage::Response(response_to(&authenticated, 200, "OK")))
        .await
        .expect("200");
    harness
        .faults
        .send(TransportError::ReadTimeout)
        .await
        .expect("invite-wait tick");
    tokio::task::yield_now().await;
    let invite = incoming_invite();
    harness
        .inbound
        .send(SipMessage::Request(invite.clone()))
        .await
        .expect("INVITE");
    let SipMessage::Response(_) = harness.outbound.recv().await.expect("180") else {
        panic!("expected 180");
    };
    let ringing = harness.service.current().await.unwrap().unwrap();
    harness
        .service
        .answer(&ringing.id, ringing.revision, ANSWER_SDP)
        .await
        .expect("answer");
    let SipMessage::Response(answer) = harness.outbound.recv().await.expect("200 answer") else {
        panic!("expected 200 answer");
    };
    harness
        .inbound
        .send(SipMessage::Request(ack_for(&answer, &invite)))
        .await
        .expect("ACK");
    wait_for_phase(&harness, CallViewPhase::Active).await;
    harness
        .faults
        .send(TransportError::ReadTimeout)
        .await
        .expect("active-dialog tick");
    tokio::task::yield_now().await;
    assert!(!task.is_finished());
    assert_eq!(
        harness.service.current().await.unwrap().unwrap().phase,
        CallViewPhase::Active
    );

    harness
        .inbound
        .send(SipMessage::Request(bye_for(&answer, &invite)))
        .await
        .expect("BYE");
    let SipMessage::Response(ok) = harness.outbound.recv().await.expect("BYE 200") else {
        panic!("expected BYE 200");
    };
    assert_eq!(ok.status(), 200);
    assert!(task.await.expect("runtime task").is_ok());
}

#[tokio::test(start_paused = true)]
async fn a_pre_invite_failure_does_not_poison_bootstrap_deduplication() {
    let connect_count = Arc::new(AtomicUsize::new(0));
    let factory = Arc::new(ScriptedFactory {
        transport: StdMutex::new(None),
        connected_url: StdMutex::new(None),
        connect_count: connect_count.clone(),
        fail_connects: AtomicUsize::new(10),
    });
    let runtime = Arc::new(
        SipCallRuntime::new_with_clock(
            factory,
            Arc::new(NoopActions),
            Arc::new(FixedClock),
            test_config(),
        )
        .expect("runtime"),
    );
    let service = Arc::new(CallService::new(runtime.clone(), Arc::new(NoopOpen)));
    runtime.attach_service(Arc::downgrade(&service)).unwrap();

    assert!(matches!(
        runtime
            .handle_bootstrap(bootstrap(), CancellationToken::new())
            .await,
        Err(RuntimeError::ConnectionUnavailable)
    ));
    assert_eq!(connect_count.load(Ordering::SeqCst), 3);
    assert!(matches!(
        runtime
            .handle_bootstrap(bootstrap(), CancellationToken::new())
            .await,
        Err(RuntimeError::ConnectionUnavailable)
    ));
    assert_eq!(connect_count.load(Ordering::SeqCst), 6);
}

#[tokio::test]
async fn never_reconnects_or_replays_a_bootstrap_after_an_invite_was_observed() {
    let mut harness = harness(test_config());
    let cancellation = CancellationToken::new();
    let (task, _, _) = start_ringing(&mut harness, &cancellation).await;
    harness
        .faults
        .send(TransportError::Closed)
        .await
        .expect("transport failure");

    assert!(matches!(
        task.await.expect("runtime task"),
        Err(RuntimeError::Transport)
    ));
    assert_eq!(harness.connect_count.load(Ordering::SeqCst), 1);
    assert_eq!(harness.close_count.load(Ordering::SeqCst), 1);
    assert!(harness.service.current().await.expect("service").is_none());
    assert!(matches!(
        harness
            .runtime
            .handle_bootstrap(bootstrap(), CancellationToken::new())
            .await,
        Err(RuntimeError::DuplicateBootstrap)
    ));
    assert_eq!(harness.connect_count.load(Ordering::SeqCst), 1);
}
