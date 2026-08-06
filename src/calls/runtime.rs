use std::{
    collections::VecDeque,
    fmt,
    sync::{Arc, Mutex as StdMutex, OnceLock, Weak},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use secrecy::{ExposeSecret, SecretString};
use subtle::ConstantTimeEq;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{sleep, timeout};
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

use crate::{
    calls::sip::{
        DialogState, ReconnectPolicy, RegisterConfig, RegistrarAction, SipDialog, SipLimits,
        SipMessage, SipMethod, SipRegistrar, SipRequest, SipResponse, SipTransport, TransportError,
        WssSipTransport, WssTransportConfig,
    },
    domain::call::{CallEndReason, CallId, CallMedia},
    error::{AppError, ErrorCode},
    provider::mts::{
        call::{CallActionProvider, RemoteCallAction},
        fcm::IncomingCallBootstrap,
    },
    service::calls::{CallService, CallTransport, ObservedIncomingCall},
};

const MAX_RUNTIME_DURATION: Duration = Duration::from_secs(5 * 60);
const DEFAULT_MAX_BOOTSTRAP_AGE: Duration = Duration::from_secs(3 * 60);
const DEFAULT_MAX_FUTURE_SKEW: Duration = Duration::from_secs(60);
const MAX_RECENT_BOOTSTRAPS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeConfig {
    command_capacity: usize,
    registration_timeout: Duration,
    invite_timeout: Duration,
    ringing_timeout: Duration,
    command_timeout: Duration,
    max_bootstrap_age: Duration,
    max_future_skew: Duration,
}

impl RuntimeConfig {
    pub fn new(
        command_capacity: usize,
        registration_timeout: Duration,
        invite_timeout: Duration,
        ringing_timeout: Duration,
        command_timeout: Duration,
    ) -> Result<Self, RuntimeError> {
        let config = Self {
            command_capacity,
            registration_timeout,
            invite_timeout,
            ringing_timeout,
            command_timeout,
            max_bootstrap_age: DEFAULT_MAX_BOOTSTRAP_AGE,
            max_future_skew: DEFAULT_MAX_FUTURE_SKEW,
        };
        if command_capacity == 0
            || command_capacity > 64
            || [
                registration_timeout,
                invite_timeout,
                ringing_timeout,
                command_timeout,
            ]
            .into_iter()
            .any(|duration| duration.is_zero() || duration > MAX_RUNTIME_DURATION)
        {
            return Err(RuntimeError::InvalidConfig);
        }
        Ok(config)
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            command_capacity: 8,
            registration_timeout: Duration::from_secs(15),
            invite_timeout: Duration::from_secs(30),
            ringing_timeout: Duration::from_secs(45),
            command_timeout: Duration::from_secs(10),
            max_bootstrap_age: DEFAULT_MAX_BOOTSTRAP_AGE,
            max_future_skew: DEFAULT_MAX_FUTURE_SKEW,
        }
    }
}

pub trait CallRuntimeClock: Send + Sync {
    fn now_millis(&self) -> u64;
}

#[derive(Clone, Copy, Debug, Default)]
struct SystemRuntimeClock;

impl CallRuntimeClock for SystemRuntimeClock {
    fn now_millis(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }
}

#[async_trait]
pub trait SipConnectionFactory: Send + Sync {
    async fn connect(
        &self,
        config: WssTransportConfig,
    ) -> Result<Box<dyn SipTransport>, RuntimeError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct WssConnectionFactory;

#[async_trait]
impl SipConnectionFactory for WssConnectionFactory {
    async fn connect(
        &self,
        config: WssTransportConfig,
    ) -> Result<Box<dyn SipTransport>, RuntimeError> {
        WssSipTransport::connect(config)
            .await
            .map(|transport| Box::new(transport) as Box<dyn SipTransport>)
            .map_err(RuntimeError::from)
    }
}

pub struct SipCallRuntime {
    factory: Arc<dyn SipConnectionFactory>,
    actions: Arc<dyn CallActionProvider>,
    clock: Arc<dyn CallRuntimeClock>,
    config: RuntimeConfig,
    service: OnceLock<Weak<CallService>>,
    registry: Arc<StdMutex<RuntimeRegistry>>,
}

impl SipCallRuntime {
    pub fn new(
        factory: Arc<dyn SipConnectionFactory>,
        actions: Arc<dyn CallActionProvider>,
        config: RuntimeConfig,
    ) -> Result<Self, RuntimeError> {
        Self::new_with_clock(factory, actions, Arc::new(SystemRuntimeClock), config)
    }

    pub fn new_with_clock(
        factory: Arc<dyn SipConnectionFactory>,
        actions: Arc<dyn CallActionProvider>,
        clock: Arc<dyn CallRuntimeClock>,
        config: RuntimeConfig,
    ) -> Result<Self, RuntimeError> {
        RuntimeConfig::new(
            config.command_capacity,
            config.registration_timeout,
            config.invite_timeout,
            config.ringing_timeout,
            config.command_timeout,
        )?;
        Ok(Self {
            factory,
            actions,
            clock,
            config,
            service: OnceLock::new(),
            registry: Arc::new(StdMutex::new(RuntimeRegistry::default())),
        })
    }

    pub fn production(actions: Arc<dyn CallActionProvider>) -> Self {
        Self {
            factory: Arc::new(WssConnectionFactory),
            actions,
            clock: Arc::new(SystemRuntimeClock),
            config: RuntimeConfig::default(),
            service: OnceLock::new(),
            registry: Arc::new(StdMutex::new(RuntimeRegistry::default())),
        }
    }

    pub fn attach_service(&self, service: Weak<CallService>) -> Result<(), RuntimeError> {
        if service.strong_count() == 0 {
            return Err(RuntimeError::ServiceUnavailable);
        }
        self.service
            .set(service)
            .map_err(|_| RuntimeError::ServiceAlreadyAttached)
    }

    pub async fn handle_bootstrap(
        &self,
        bootstrap: IncomingCallBootstrap,
        cancellation: CancellationToken,
    ) -> Result<(), RuntimeError> {
        let service = self
            .service
            .get()
            .and_then(Weak::upgrade)
            .ok_or(RuntimeError::ServiceUnavailable)?;
        self.validate_freshness(&bootstrap)?;
        let (mut commands, lease) = self.reserve(bootstrap.call_session_id().clone())?;
        let reconnect = ReconnectPolicy::default();
        let mut retry: u8 = 0;
        let result = loop {
            let connection = tokio::select! {
                biased;
                () = cancellation.cancelled() => break Err(RuntimeError::Cancelled),
                result = self.connect(&bootstrap) => result,
            };
            let mut transport = match connection {
                Ok(transport) => transport,
                Err(error) if should_reconnect(error, &lease) => {
                    let Some(delay) = reconnect.delay_for(retry + 1) else {
                        break Err(error);
                    };
                    retry += 1;
                    tokio::select! {
                        biased;
                        () = cancellation.cancelled() => break Err(RuntimeError::Cancelled),
                        () = sleep(delay) => continue,
                    }
                }
                Err(error) => break Err(error),
            };
            let attempt = self
                .run_connected(
                    &bootstrap,
                    &service,
                    &mut *transport,
                    &mut commands,
                    &lease,
                    &cancellation,
                )
                .await;
            let _ = transport.close().await;
            match attempt {
                Err(error) if should_reconnect(error, &lease) => {
                    let Some(delay) = reconnect.delay_for(retry + 1) else {
                        break Err(error);
                    };
                    retry += 1;
                    tokio::select! {
                        biased;
                        () = cancellation.cancelled() => break Err(RuntimeError::Cancelled),
                        () = sleep(delay) => continue,
                    }
                }
                result => break result,
            }
        };
        if result.is_err()
            && let Some(call_id) = lease.call_id()
        {
            let _ = service
                .remote_ended(&call_id, CallEndReason::RemoteHangup)
                .await;
        }
        match result {
            Err(RuntimeError::Cancelled) => Ok(()),
            other => other,
        }
    }

    fn validate_freshness(&self, bootstrap: &IncomingCallBootstrap) -> Result<(), RuntimeError> {
        let now = self.clock.now_millis();
        let timestamp = bootstrap.timestamp_millis();
        let max_age = duration_millis(self.config.max_bootstrap_age);
        let max_future = duration_millis(self.config.max_future_skew);
        if timestamp <= now && now - timestamp > max_age {
            return Err(RuntimeError::StaleBootstrap);
        }
        if timestamp > now && timestamp - now > max_future {
            return Err(RuntimeError::FutureBootstrap);
        }
        Ok(())
    }

    fn reserve(
        &self,
        call_session_id: SecretString,
    ) -> Result<(mpsc::Receiver<RuntimeCommand>, ActiveLease), RuntimeError> {
        let mut registry = lock_registry(&self.registry);
        if registry.active.is_some() {
            return Err(RuntimeError::CallAlreadyActive);
        }
        if registry
            .recent
            .iter()
            .any(|recent| secret_eq(recent, &call_session_id))
        {
            return Err(RuntimeError::DuplicateBootstrap);
        }
        let (sender, receiver) = mpsc::channel(self.config.command_capacity);
        let token = Uuid::new_v4();
        registry.active = Some(ActiveCall {
            token,
            call_session_id,
            call_id: None,
            invite_received: false,
            sender,
        });
        Ok((
            receiver,
            ActiveLease {
                token,
                registry: self.registry.clone(),
            },
        ))
    }

    async fn connect(
        &self,
        bootstrap: &IncomingCallBootstrap,
    ) -> Result<Box<dyn SipTransport>, RuntimeError> {
        let mut url = Url::parse(bootstrap.websocket_url().expose_secret())
            .map_err(|_| RuntimeError::InvalidBootstrap)?;
        if url.scheme() == "https" {
            url.set_scheme("wss")
                .map_err(|_| RuntimeError::InvalidBootstrap)?;
        }
        let limits = SipLimits {
            max_message_bytes: 256 * 1024,
            max_header_bytes: 16 * 1024,
            max_header_count: 96,
            max_line_bytes: 4 * 1024,
            max_body_bytes: 240 * 1024,
        };
        let config = WssTransportConfig::new(url, limits)?;
        self.factory.connect(config).await
    }

    async fn run_connected(
        &self,
        bootstrap: &IncomingCallBootstrap,
        service: &Arc<CallService>,
        transport: &mut dyn SipTransport,
        commands: &mut mpsc::Receiver<RuntimeCommand>,
        lease: &ActiveLease,
        cancellation: &CancellationToken,
    ) -> Result<(), RuntimeError> {
        let local_sent_by = Uuid::new_v4().simple().to_string();
        let username = bootstrap.sip().username();
        let register_config = RegisterConfig::new(
            bootstrap.sip().registrar(),
            SecretString::from(username.to_owned()),
            bootstrap.sip().secret().clone(),
            local_sent_by.clone(),
        )?;
        self.register(transport, register_config, cancellation)
            .await?;
        let local_contact = format!("<sip:{username}@{local_sent_by};transport=wss>");
        let mut dialog = self
            .wait_for_invite(transport, &local_contact, &local_sent_by, cancellation)
            .await?;
        lease.mark_invite_received()?;
        transport
            .send(&SipMessage::Response(dialog.ringing()?))
            .await?;

        let call_id = CallId::new();
        lease.bind_call(call_id.clone())?;
        let offer =
            std::str::from_utf8(dialog.offer_sdp()).map_err(|_| RuntimeError::InvalidInvite)?;
        let timestamp: i64 = bootstrap
            .timestamp_millis()
            .try_into()
            .map_err(|_| RuntimeError::InvalidBootstrap)?;
        let observed_at = DateTime::<Utc>::from_timestamp_millis(timestamp)
            .ok_or(RuntimeError::InvalidBootstrap)?;
        let media = if offer.lines().any(|line| line.starts_with("m=video ")) {
            CallMedia::AudioVideo
        } else {
            CallMedia::Audio
        };
        service
            .observe_incoming(ObservedIncomingCall::new(
                call_id.clone(),
                media,
                bootstrap.caller_name().to_owned(),
                SecretString::from(offer.to_owned()),
                SecretString::from(bootstrap.intercom_id().to_owned()),
                bootstrap.call_session_id().clone(),
                observed_at,
            ))
            .await
            .map_err(|_| RuntimeError::Service)?;

        let ringing_timeout = sleep(self.config.ringing_timeout);
        tokio::pin!(ringing_timeout);
        let mut pending_hangup = None;
        loop {
            tokio::select! {
                biased;
                () = cancellation.cancelled() => {
                    self.teardown_dialog(
                        transport,
                        &mut dialog,
                        &local_contact,
                        &mut pending_hangup,
                    ).await;
                    let _ = service.remote_ended(&call_id, CallEndReason::Cancelled).await;
                    return Err(RuntimeError::Cancelled);
                }
                command = commands.recv() => {
                    let Some(command) = command else {
                        return Err(RuntimeError::CommandChannelClosed);
                    };
                    let command_result = self.handle_command(
                        command,
                        bootstrap,
                        transport,
                        &mut dialog,
                        &call_id,
                        cancellation,
                        &mut pending_hangup,
                    ).await;
                    match command_result {
                        Ok(ActorControl::Continue) => {}
                        Ok(ActorControl::Terminate) => return Ok(()),
                        Err(RuntimeError::Cancelled) => {
                            self.teardown_dialog(
                                transport,
                                &mut dialog,
                                &local_contact,
                                &mut pending_hangup,
                            ).await;
                            let _ = service.remote_ended(&call_id, CallEndReason::Cancelled).await;
                            return Err(RuntimeError::Cancelled);
                        }
                        Err(error) => return Err(error),
                    }
                }
                inbound = transport.receive() => {
                    let inbound = match inbound {
                        Ok(inbound) => inbound,
                        Err(TransportError::ReadTimeout) => continue,
                        Err(error) => return Err(error.into()),
                    };
                    if self.handle_inbound(
                        inbound,
                        transport,
                        service,
                        &call_id,
                        &mut dialog,
                        &local_contact,
                        &mut pending_hangup,
                    ).await? == ActorControl::Terminate {
                        return Ok(());
                    }
                }
                () = &mut ringing_timeout, if dialog.state() == DialogState::Ringing => {
                    transport.send(&SipMessage::Response(dialog.timeout()?)).await?;
                    let _ = service.remote_ended(&call_id, CallEndReason::Timeout).await;
                    return Ok(());
                }
            }
        }
    }

    async fn register(
        &self,
        transport: &mut dyn SipTransport,
        config: RegisterConfig,
        cancellation: &CancellationToken,
    ) -> Result<(), RuntimeError> {
        let registration = async {
            let mut registrar = SipRegistrar::new(config);
            transport
                .send(&SipMessage::Request(registrar.begin()?))
                .await?;
            loop {
                let inbound = match transport.receive().await {
                    Ok(inbound) => inbound,
                    Err(TransportError::ReadTimeout) => continue,
                    Err(error) => return Err(error.into()),
                };
                match inbound {
                    SipMessage::Response(response) => match registrar.on_response(&response)? {
                        RegistrarAction::Send(request) => {
                            transport.send(&SipMessage::Request(request)).await?;
                        }
                        RegistrarAction::Registered { .. } => return Ok(()),
                        RegistrarAction::Failed { .. } => {
                            return Err(RuntimeError::RegistrationRejected);
                        }
                        RegistrarAction::Ignored => {}
                    },
                    SipMessage::Request(request) if request.method() == &SipMethod::Options => {
                        transport
                            .send(&SipMessage::Response(ok_to_request(&request, None)?))
                            .await?;
                    }
                    SipMessage::Request(_) => {}
                }
            }
        };
        tokio::select! {
            biased;
            () = cancellation.cancelled() => Err(RuntimeError::Cancelled),
            result = timeout(self.config.registration_timeout, registration) => {
                result.map_err(|_| RuntimeError::RegistrationTimeout)?
            }
        }
    }

    async fn wait_for_invite(
        &self,
        transport: &mut dyn SipTransport,
        local_contact: &str,
        local_sent_by: &str,
        cancellation: &CancellationToken,
    ) -> Result<SipDialog, RuntimeError> {
        let waiting = async {
            loop {
                let inbound = match transport.receive().await {
                    Ok(inbound) => inbound,
                    Err(TransportError::ReadTimeout) => continue,
                    Err(error) => return Err(error.into()),
                };
                match inbound {
                    SipMessage::Request(request) if request.method() == &SipMethod::Invite => {
                        return SipDialog::from_invite(&request, local_contact, local_sent_by)
                            .map_err(RuntimeError::from);
                    }
                    SipMessage::Request(request) if request.method() == &SipMethod::Options => {
                        transport
                            .send(&SipMessage::Response(ok_to_request(
                                &request,
                                Some(local_contact),
                            )?))
                            .await?;
                    }
                    SipMessage::Request(_) | SipMessage::Response(_) => {}
                }
            }
        };
        tokio::select! {
            biased;
            () = cancellation.cancelled() => Err(RuntimeError::Cancelled),
            result = timeout(self.config.invite_timeout, waiting) => {
                result.map_err(|_| RuntimeError::InviteTimeout)?
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn handle_inbound(
        &self,
        inbound: SipMessage,
        transport: &mut dyn SipTransport,
        service: &Arc<CallService>,
        call_id: &CallId,
        dialog: &mut SipDialog,
        local_contact: &str,
        pending_hangup: &mut Option<PendingHangup>,
    ) -> Result<ActorControl, RuntimeError> {
        let SipMessage::Request(request) = inbound else {
            return Ok(ActorControl::Continue);
        };
        match request.method() {
            SipMethod::Cancel if dialog.state() == DialogState::Ringing => {
                let cancelled = dialog.on_cancel(&request)?;
                transport
                    .send(&SipMessage::Response(cancelled.cancel_ok))
                    .await?;
                transport
                    .send(&SipMessage::Response(cancelled.invite_terminated))
                    .await?;
                let _ = service
                    .remote_ended(call_id, CallEndReason::Cancelled)
                    .await;
                return Ok(ActorControl::Terminate);
            }
            SipMethod::Ack if dialog.state() == DialogState::Answered => {
                if dialog.on_ack(&request).is_err() {
                    return Ok(ActorControl::Continue);
                }
                if let Some(pending) = pending_hangup.take() {
                    let bye = dialog.outbound_bye()?;
                    transport.send(&SipMessage::Request(bye)).await?;
                    let _ = pending.response.send(Ok(()));
                    return Ok(ActorControl::Terminate);
                }
                let _ = service.mark_connected(call_id).await;
            }
            SipMethod::Bye if dialog.state() == DialogState::Established => {
                let response = match dialog.on_bye(&request) {
                    Ok(response) => response,
                    Err(_) => return Ok(ActorControl::Continue),
                };
                transport.send(&SipMessage::Response(response)).await?;
                let _ = service
                    .remote_ended(call_id, CallEndReason::RemoteHangup)
                    .await;
                return Ok(ActorControl::Terminate);
            }
            SipMethod::Options => {
                transport
                    .send(&SipMessage::Response(ok_to_request(
                        &request,
                        Some(local_contact),
                    )?))
                    .await?;
            }
            SipMethod::Invite => {
                if dialog.state() == DialogState::Ringing
                    && request.headers().get("Call-ID") == Some(dialog.id().call_id())
                {
                    transport
                        .send(&SipMessage::Response(dialog.ringing()?))
                        .await?;
                } else if let Ok(mut extra) = SipDialog::from_invite(
                    &request,
                    local_contact,
                    local_contact_sent_by(local_contact)?,
                ) {
                    transport
                        .send(&SipMessage::Response(extra.ignore()?))
                        .await?;
                }
            }
            _ => {}
        }
        Ok(ActorControl::Continue)
    }

    #[allow(clippy::too_many_arguments)]
    async fn handle_command(
        &self,
        command: RuntimeCommand,
        bootstrap: &IncomingCallBootstrap,
        transport: &mut dyn SipTransport,
        dialog: &mut SipDialog,
        active_call_id: &CallId,
        cancellation: &CancellationToken,
        pending_hangup: &mut Option<PendingHangup>,
    ) -> Result<ActorControl, RuntimeError> {
        let RuntimeCommand {
            call_id,
            kind,
            response,
        } = command;
        if &call_id != active_call_id {
            let _ = response.send(Err(call_not_found()));
            return Ok(ActorControl::Continue);
        }
        match kind {
            CommandKind::Answer(answer_sdp) => {
                let answer = match dialog.answer(answer_sdp.expose_secret().as_bytes()) {
                    Ok(answer) => answer,
                    Err(_) => {
                        let _ = response.send(Err(invalid_call_action()));
                        return Ok(ActorControl::Continue);
                    }
                };
                transport.send(&SipMessage::Response(answer)).await?;
                let _ = response.send(Ok(()));
                Ok(ActorControl::Continue)
            }
            CommandKind::Decline => {
                let rejected = match dialog.reject() {
                    Ok(rejected) => rejected,
                    Err(_) => {
                        let _ = response.send(Err(invalid_call_action()));
                        return Ok(ActorControl::Continue);
                    }
                };
                transport.send(&SipMessage::Response(rejected)).await?;
                let _ = response.send(Ok(()));
                Ok(ActorControl::Terminate)
            }
            CommandKind::Ignore => {
                let action = tokio::select! {
                    biased;
                    () = cancellation.cancelled() => {
                        let _ = response.send(Err(command_unavailable()));
                        return Err(RuntimeError::Cancelled);
                    }
                    result = timeout(
                        self.config.command_timeout,
                        self.actions.action(
                            bootstrap.call_session_id(),
                            RemoteCallAction::Ignore,
                        ),
                    ) => result,
                };
                let action = match action {
                    Ok(action) => action,
                    Err(_) => {
                        let _ = response.send(Err(mutation_outcome_unknown()));
                        if let Ok(final_response) = dialog.timeout() {
                            transport
                                .send(&SipMessage::Response(final_response))
                                .await?;
                        }
                        return Ok(ActorControl::Terminate);
                    }
                };
                if let Err(error) = action {
                    let _ = response.send(Err(error));
                    if let Ok(final_response) = dialog.timeout() {
                        transport
                            .send(&SipMessage::Response(final_response))
                            .await?;
                    }
                    return Ok(ActorControl::Terminate);
                }
                let ignored = match dialog.ignore() {
                    Ok(ignored) => ignored,
                    Err(_) => {
                        let _ = response.send(Err(invalid_call_action()));
                        return Ok(ActorControl::Continue);
                    }
                };
                transport.send(&SipMessage::Response(ignored)).await?;
                let _ = response.send(Ok(()));
                Ok(ActorControl::Terminate)
            }
            CommandKind::Hangup => match dialog.state() {
                DialogState::Established => {
                    let bye = dialog.outbound_bye()?;
                    transport.send(&SipMessage::Request(bye)).await?;
                    let _ = response.send(Ok(()));
                    Ok(ActorControl::Terminate)
                }
                DialogState::Answered if pending_hangup.is_none() => {
                    *pending_hangup = Some(PendingHangup { response });
                    Ok(ActorControl::Continue)
                }
                _ => {
                    let _ = response.send(Err(invalid_call_action()));
                    Ok(ActorControl::Continue)
                }
            },
        }
    }

    async fn teardown_dialog(
        &self,
        transport: &mut dyn SipTransport,
        dialog: &mut SipDialog,
        local_contact: &str,
        pending_hangup: &mut Option<PendingHangup>,
    ) {
        match dialog.state() {
            DialogState::Ringing => {
                if let Ok(response) = dialog.timeout() {
                    let _ = transport.send(&SipMessage::Response(response)).await;
                }
            }
            DialogState::Established => {
                if let Ok(bye) = dialog.outbound_bye() {
                    let _ = transport.send(&SipMessage::Request(bye)).await;
                }
            }
            DialogState::Answered => {
                let awaiting_ack = async {
                    loop {
                        match transport.receive().await {
                            Ok(SipMessage::Request(request))
                                if request.method() == &SipMethod::Ack =>
                            {
                                if dialog.on_ack(&request).is_ok() {
                                    if let Ok(bye) = dialog.outbound_bye() {
                                        let _ = transport.send(&SipMessage::Request(bye)).await;
                                    }
                                    break;
                                }
                            }
                            Ok(SipMessage::Request(request))
                                if request.method() == &SipMethod::Options =>
                            {
                                if let Ok(response) = ok_to_request(&request, Some(local_contact)) {
                                    let _ = transport.send(&SipMessage::Response(response)).await;
                                }
                            }
                            Err(TransportError::ReadTimeout) => continue,
                            Ok(_) | Err(_) => break,
                        }
                    }
                };
                let _ = timeout(self.config.command_timeout, awaiting_ack).await;
            }
            DialogState::Ending | DialogState::Terminated => {}
        }
        if let Some(pending) = pending_hangup.take() {
            let _ = pending.response.send(Err(command_unavailable()));
        }
    }

    async fn dispatch(&self, call_id: &CallId, kind: CommandKind) -> Result<(), AppError> {
        let sender = {
            let registry = lock_registry(&self.registry);
            let active = registry.active.as_ref().ok_or_else(call_not_found)?;
            if active.call_id.as_ref() != Some(call_id) {
                return Err(call_not_found());
            }
            active.sender.clone()
        };
        let (response, receiver) = oneshot::channel();
        let command = RuntimeCommand {
            call_id: call_id.clone(),
            kind,
            response,
        };
        timeout(self.config.command_timeout, sender.send(command))
            .await
            .map_err(|_| command_timeout())?
            .map_err(|_| command_unavailable())?;
        timeout(self.config.command_timeout, receiver)
            .await
            .map_err(|_| command_timeout())?
            .map_err(|_| command_unavailable())?
    }
}

impl fmt::Debug for SipCallRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let registry = lock_registry(&self.registry);
        formatter
            .debug_struct("SipCallRuntime")
            .field("config", &self.config)
            .field("service_attached", &self.service.get().is_some())
            .field("call_active", &registry.active.is_some())
            .field("recent_bootstrap_count", &registry.recent.len())
            .finish()
    }
}

#[async_trait]
impl CallTransport for SipCallRuntime {
    async fn answer(&self, call_id: &CallId, answer_sdp: &str) -> Result<(), AppError> {
        self.dispatch(
            call_id,
            CommandKind::Answer(SecretString::from(answer_sdp.to_owned())),
        )
        .await
    }

    async fn decline(&self, call_id: &CallId) -> Result<(), AppError> {
        self.dispatch(call_id, CommandKind::Decline).await
    }

    async fn ignore(&self, call_id: &CallId) -> Result<(), AppError> {
        self.dispatch(call_id, CommandKind::Ignore).await
    }

    async fn hangup(&self, call_id: &CallId) -> Result<(), AppError> {
        self.dispatch(call_id, CommandKind::Hangup).await
    }
}

#[derive(Default)]
struct RuntimeRegistry {
    active: Option<ActiveCall>,
    recent: VecDeque<SecretString>,
}

struct ActiveCall {
    token: Uuid,
    call_session_id: SecretString,
    call_id: Option<CallId>,
    invite_received: bool,
    sender: mpsc::Sender<RuntimeCommand>,
}

struct ActiveLease {
    token: Uuid,
    registry: Arc<StdMutex<RuntimeRegistry>>,
}

impl ActiveLease {
    fn mark_invite_received(&self) -> Result<(), RuntimeError> {
        let mut registry = lock_registry(&self.registry);
        let active = registry
            .active
            .as_mut()
            .filter(|active| active.token == self.token)
            .ok_or(RuntimeError::CallUnavailable)?;
        active.invite_received = true;
        Ok(())
    }

    fn invite_received(&self) -> bool {
        lock_registry(&self.registry)
            .active
            .as_ref()
            .is_some_and(|active| active.token == self.token && active.invite_received)
    }

    fn bind_call(&self, call_id: CallId) -> Result<(), RuntimeError> {
        let mut registry = lock_registry(&self.registry);
        let active = registry
            .active
            .as_mut()
            .filter(|active| active.token == self.token)
            .ok_or(RuntimeError::CallUnavailable)?;
        active.call_id = Some(call_id);
        Ok(())
    }

    fn call_id(&self) -> Option<CallId> {
        lock_registry(&self.registry)
            .active
            .as_ref()
            .filter(|active| active.token == self.token)
            .and_then(|active| active.call_id.clone())
    }
}

impl Drop for ActiveLease {
    fn drop(&mut self) {
        let mut registry = lock_registry(&self.registry);
        if registry
            .active
            .as_ref()
            .is_none_or(|active| active.token != self.token)
        {
            return;
        }
        let Some(active) = registry.active.take() else {
            return;
        };
        if active.invite_received {
            registry.recent.push_back(active.call_session_id);
            while registry.recent.len() > MAX_RECENT_BOOTSTRAPS {
                registry.recent.pop_front();
            }
        }
    }
}

struct RuntimeCommand {
    call_id: CallId,
    kind: CommandKind,
    response: oneshot::Sender<Result<(), AppError>>,
}

enum CommandKind {
    Answer(SecretString),
    Decline,
    Ignore,
    Hangup,
}

struct PendingHangup {
    response: oneshot::Sender<Result<(), AppError>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActorControl {
    Continue,
    Terminate,
}

fn ok_to_request(
    request: &SipRequest,
    local_contact: Option<&str>,
) -> Result<SipResponse, RuntimeError> {
    let mut response = SipResponse::new(200, "OK")?;
    for via in request.headers().get_all("Via") {
        response.headers_mut().push("Via", via)?;
    }
    for name in ["From", "To", "Call-ID", "CSeq"] {
        let value = request
            .headers()
            .get(name)
            .ok_or(RuntimeError::InvalidRequest)?;
        response.headers_mut().push(name, value)?;
    }
    if let Some(contact) = local_contact {
        response.headers_mut().push("Contact", contact)?;
    }
    response
        .headers_mut()
        .push("Allow", "ACK,INVITE,BYE,OPTIONS,CANCEL")?;
    Ok(response)
}

fn local_contact_sent_by(contact: &str) -> Result<&str, RuntimeError> {
    contact
        .strip_prefix("<sip:")
        .and_then(|value| value.split_once('@'))
        .map(|(_, host)| host.trim_end_matches('>').split(';').next().unwrap_or(host))
        .filter(|host| !host.is_empty())
        .ok_or(RuntimeError::InvalidRequest)
}

fn lock_registry(
    registry: &StdMutex<RuntimeRegistry>,
) -> std::sync::MutexGuard<'_, RuntimeRegistry> {
    registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn secret_eq(left: &SecretString, right: &SecretString) -> bool {
    let left = left.expose_secret().as_bytes();
    let right = right.expose_secret().as_bytes();
    left.len() == right.len() && bool::from(left.ct_eq(right))
}

fn duration_millis(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

fn call_not_found() -> AppError {
    AppError::new(ErrorCode::CallNotFound, "Call was not found.", false)
}

fn command_timeout() -> AppError {
    AppError::new(
        ErrorCode::UpstreamUnavailable,
        "Call signaling command timed out.",
        false,
    )
}

fn command_unavailable() -> AppError {
    AppError::new(
        ErrorCode::UpstreamUnavailable,
        "Call signaling command is unavailable.",
        false,
    )
}

fn invalid_call_action() -> AppError {
    AppError::invalid_request("call action is not valid in the current signaling phase")
}

fn mutation_outcome_unknown() -> AppError {
    AppError::new(
        ErrorCode::MutationOutcomeUnknown,
        "Call action outcome is unknown.",
        false,
    )
}

fn should_reconnect(error: RuntimeError, lease: &ActiveLease) -> bool {
    if lease.invite_received() {
        return false;
    }
    matches!(
        error,
        RuntimeError::ConnectionUnavailable
            | RuntimeError::Transport
            | RuntimeError::Registration
            | RuntimeError::RegistrationTimeout
            | RuntimeError::InviteTimeout
    )
}

impl From<TransportError> for RuntimeError {
    fn from(_: TransportError) -> Self {
        Self::Transport
    }
}

impl From<crate::calls::sip::RegistrarError> for RuntimeError {
    fn from(_: crate::calls::sip::RegistrarError) -> Self {
        Self::Registration
    }
}

impl From<crate::calls::sip::DialogError> for RuntimeError {
    fn from(_: crate::calls::sip::DialogError) -> Self {
        Self::InvalidDialog
    }
}

impl From<crate::calls::sip::SipError> for RuntimeError {
    fn from(_: crate::calls::sip::SipError) -> Self {
        Self::Protocol
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeError {
    #[error("call runtime configuration is invalid")]
    InvalidConfig,
    #[error("call service is unavailable")]
    ServiceUnavailable,
    #[error("call service is already attached")]
    ServiceAlreadyAttached,
    #[error("incoming call bootstrap is stale")]
    StaleBootstrap,
    #[error("incoming call bootstrap timestamp is in the future")]
    FutureBootstrap,
    #[error("incoming call bootstrap is invalid")]
    InvalidBootstrap,
    #[error("this incoming call was already handled")]
    DuplicateBootstrap,
    #[error("another call is already active")]
    CallAlreadyActive,
    #[error("call is unavailable")]
    CallUnavailable,
    #[error("SIP WebSocket connection is unavailable")]
    ConnectionUnavailable,
    #[error("SIP WebSocket transport failed")]
    Transport,
    #[error("SIP registration failed")]
    Registration,
    #[error("SIP registration was rejected")]
    RegistrationRejected,
    #[error("SIP registration timed out")]
    RegistrationTimeout,
    #[error("SIP INVITE timed out")]
    InviteTimeout,
    #[error("SIP INVITE is invalid")]
    InvalidInvite,
    #[error("SIP dialog is invalid")]
    InvalidDialog,
    #[error("SIP request is invalid")]
    InvalidRequest,
    #[error("SIP protocol failed")]
    Protocol,
    #[error("call service update failed")]
    Service,
    #[error("call runtime command channel closed")]
    CommandChannelClosed,
    #[error("call runtime was cancelled")]
    Cancelled,
}
