use std::{collections::HashMap, fmt, str::FromStr, sync::Arc, time::Duration};

use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use secrecy::{ExposeSecret, SecretString};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    domain::call::{
        CallCommand, CallEndReason, CallId, CallMedia, CallPhase, CallRevision, CallSession,
        CallTransitionError, IncomingInvite, ProviderCallId, reduce,
    },
    error::{AppError, ErrorCode},
};

const DEFAULT_OPEN_INTENT_TTL: Duration = Duration::from_secs(30);
const DEFAULT_MAX_PENDING_OPEN_INTENTS: usize = 64;
const MAX_CALLER_NAME_BYTES: usize = 2 * 1024;
pub const MAX_SDP_BYTES: usize = 256 * 1024;

#[async_trait]
pub trait CallTransport: Send + Sync {
    async fn answer(&self, call_id: &CallId, answer_sdp: &str) -> Result<(), AppError>;
    async fn decline(&self, call_id: &CallId) -> Result<(), AppError>;
    async fn ignore(&self, call_id: &CallId) -> Result<(), AppError>;
    async fn hangup(&self, call_id: &CallId) -> Result<(), AppError>;
}

#[async_trait]
pub trait CallOpenProvider: Send + Sync {
    async fn open(
        &self,
        intercom_id: &SecretString,
        call_session_id: &SecretString,
    ) -> Result<(), AppError>;
}

pub trait CallClock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

#[derive(Clone, Copy, Debug, Default)]
struct SystemCallClock;

impl CallClock for SystemCallClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[derive(Clone)]
pub struct ObservedIncomingCall {
    id: CallId,
    media: CallMedia,
    caller_name: String,
    remote_sdp: SecretString,
    intercom_id: SecretString,
    call_session_id: SecretString,
    observed_at: DateTime<Utc>,
}

impl ObservedIncomingCall {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: CallId,
        media: CallMedia,
        caller_name: String,
        remote_sdp: SecretString,
        intercom_id: SecretString,
        call_session_id: SecretString,
        observed_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id,
            media,
            caller_name,
            remote_sdp,
            intercom_id,
            call_session_id,
            observed_at,
        }
    }

    pub fn id(&self) -> &CallId {
        &self.id
    }
}

impl fmt::Debug for ObservedIncomingCall {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ObservedIncomingCall")
            .field("id", &self.id)
            .field("media", &self.media)
            .field("observed_at", &self.observed_at)
            .field("caller_name", &"[REDACTED]")
            .field("remote_sdp", &"[REDACTED]")
            .field("intercom_id", &"[REDACTED]")
            .field("call_session_id", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CallViewPhase {
    Ringing,
    Connecting,
    Active,
    Ending,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallView {
    pub id: CallId,
    pub revision: CallRevision,
    pub phase: CallViewPhase,
    pub caller_name: String,
    pub started_at: DateTime<Utc>,
    pub remote_sdp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_url: Option<String>,
    pub can_open: bool,
}

impl fmt::Debug for CallView {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CallView")
            .field("id", &self.id)
            .field("revision", &self.revision)
            .field("phase", &self.phase)
            .field("started_at", &self.started_at)
            .field("caller_name", &"[REDACTED]")
            .field("remote_sdp", &"[REDACTED]")
            .field("has_preview_url", &self.preview_url.is_some())
            .field("can_open", &self.can_open)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct CallOpenIntentId(Uuid);

impl CallOpenIntentId {
    fn generate() -> Self {
        Self(Uuid::new_v4())
    }
}

impl fmt::Display for CallOpenIntentId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for CallOpenIntentId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallOpenIntentView {
    pub intent_id: CallOpenIntentId,
    pub expires_at: DateTime<Utc>,
}

struct ManagedCall {
    session: CallSession,
    caller_name: String,
    remote_sdp: SecretString,
    intercom_id: SecretString,
    call_session_id: SecretString,
}

impl ManagedCall {
    fn view(&self) -> Result<CallView, AppError> {
        let phase = match self.session.phase() {
            CallPhase::Ringing => CallViewPhase::Ringing,
            CallPhase::Connecting => CallViewPhase::Connecting,
            CallPhase::Active => CallViewPhase::Active,
            CallPhase::Ending => CallViewPhase::Ending,
            CallPhase::Ended(_) => return Err(internal_state_error()),
        };
        Ok(CallView {
            id: self.session.id().clone(),
            revision: self.session.revision(),
            phase,
            caller_name: self.caller_name.clone(),
            started_at: self.session.observed_at(),
            remote_sdp: self.remote_sdp.expose_secret().to_owned(),
            preview_url: None,
            can_open: phase == CallViewPhase::Active,
        })
    }
}

struct PendingOpenIntent {
    call_id: CallId,
    revision: CallRevision,
    expires_at: DateTime<Utc>,
}

#[derive(Default)]
struct CallState {
    current: Option<ManagedCall>,
    pending_open: HashMap<CallOpenIntentId, PendingOpenIntent>,
}

pub struct CallService {
    transport: Arc<dyn CallTransport>,
    opener: Arc<dyn CallOpenProvider>,
    clock: Arc<dyn CallClock>,
    open_intent_ttl: TimeDelta,
    max_pending_open: usize,
    state: Mutex<CallState>,
}

impl CallService {
    pub fn new(transport: Arc<dyn CallTransport>, opener: Arc<dyn CallOpenProvider>) -> Self {
        Self::try_new(
            transport,
            opener,
            Arc::new(SystemCallClock),
            DEFAULT_OPEN_INTENT_TTL,
            DEFAULT_MAX_PENDING_OPEN_INTENTS,
        )
        .expect("production call intent limits are valid")
    }

    #[cfg(feature = "test-support")]
    pub fn new_for_test(
        transport: Arc<dyn CallTransport>,
        opener: Arc<dyn CallOpenProvider>,
        clock: Arc<dyn CallClock>,
        open_intent_ttl: Duration,
        max_pending_open: usize,
    ) -> Result<Self, AppError> {
        Self::try_new(transport, opener, clock, open_intent_ttl, max_pending_open)
    }

    fn try_new(
        transport: Arc<dyn CallTransport>,
        opener: Arc<dyn CallOpenProvider>,
        clock: Arc<dyn CallClock>,
        open_intent_ttl: Duration,
        max_pending_open: usize,
    ) -> Result<Self, AppError> {
        if open_intent_ttl.is_zero() || max_pending_open == 0 {
            return Err(AppError::invalid_request("invalid call-open intent limits"));
        }
        let open_intent_ttl = TimeDelta::from_std(open_intent_ttl)
            .map_err(|_| AppError::invalid_request("invalid call-open intent lifetime"))?;
        Ok(Self {
            transport,
            opener,
            clock,
            open_intent_ttl,
            max_pending_open,
            state: Mutex::new(CallState::default()),
        })
    }

    pub async fn observe_incoming(
        &self,
        observed: ObservedIncomingCall,
    ) -> Result<CallView, AppError> {
        validate_observed(&observed)?;
        let mut state = self.state.lock().await;
        if let Some(current) = &state.current {
            if current.session.id() == observed.id() {
                return current.view();
            }
            return Err(AppError::new(
                ErrorCode::RateLimited,
                "Another incoming call is already being handled.",
                false,
            ));
        }

        let provider_call_id = ProviderCallId::new(observed.call_session_id.clone());
        let invite = IncomingInvite::new(
            observed.id.clone(),
            provider_call_id,
            observed.media,
            observed.intercom_id.clone(),
            observed.call_session_id.clone(),
        );
        let session = CallSession::ringing(invite, observed.observed_at);
        let managed = ManagedCall {
            session,
            caller_name: observed.caller_name,
            remote_sdp: observed.remote_sdp,
            intercom_id: observed.intercom_id,
            call_session_id: observed.call_session_id,
        };
        let view = managed.view()?;
        state.current = Some(managed);
        Ok(view)
    }

    pub async fn current(&self) -> Result<Option<CallView>, AppError> {
        self.state
            .lock()
            .await
            .current
            .as_ref()
            .map(ManagedCall::view)
            .transpose()
    }

    pub(crate) async fn ringing_call_for_intercom(
        &self,
        intercom_id: &str,
    ) -> Option<(CallId, CallRevision)> {
        let state = self.state.lock().await;
        let current = state.current.as_ref()?;
        if current.intercom_id.expose_secret() != intercom_id
            || !matches!(current.session.phase(), CallPhase::Ringing)
        {
            return None;
        }
        Some((current.session.id().clone(), current.session.revision()))
    }

    pub async fn answer(
        &self,
        id: &CallId,
        revision: CallRevision,
        answer_sdp: &str,
    ) -> Result<CallView, AppError> {
        validate_sdp(answer_sdp)?;
        let view = self
            .transition(id, CallCommand::Accept { revision })
            .await?;
        if let Err(error) = self.transport.answer(id, answer_sdp).await {
            self.finish(id, CallEndReason::Timeout).await;
            return Err(error);
        }
        Ok(view)
    }

    pub async fn decline(&self, id: &CallId, revision: CallRevision) -> Result<(), AppError> {
        self.transition(id, CallCommand::Reject { revision })
            .await?;
        let result = self.transport.decline(id).await;
        self.finish(id, CallEndReason::Rejected).await;
        result
    }

    pub async fn ignore(&self, id: &CallId, revision: CallRevision) -> Result<(), AppError> {
        self.transition(id, CallCommand::Ignore { revision })
            .await?;
        let result = self.transport.ignore(id).await;
        self.finish(id, CallEndReason::Rejected).await;
        result
    }

    pub async fn hangup(&self, id: &CallId, revision: CallRevision) -> Result<(), AppError> {
        self.transition(id, CallCommand::Hangup { revision })
            .await?;
        let result = self.transport.hangup(id).await;
        self.finish(id, CallEndReason::LocalHangup).await;
        result
    }

    pub async fn mark_connected(&self, id: &CallId) -> Result<CallView, AppError> {
        let revision = {
            let state = self.state.lock().await;
            current_for(&state, id)?.session.revision()
        };
        self.transition(id, CallCommand::Connected { revision })
            .await
    }

    pub async fn remote_ended(&self, id: &CallId, reason: CallEndReason) -> Result<(), AppError> {
        let now = self.clock.now();
        let mut state = self.state.lock().await;
        let current = current_for_mut(&mut state, id)?;
        reduce(&current.session, CallCommand::RemoteEnded(reason), now)
            .map_err(map_transition_error)?;
        state.current = None;
        state.pending_open.retain(|_, intent| &intent.call_id != id);
        Ok(())
    }

    pub async fn create_open_intent(
        &self,
        id: &CallId,
        revision: CallRevision,
    ) -> Result<CallOpenIntentView, AppError> {
        let now = self.clock.now();
        let expires_at = now
            .checked_add_signed(self.open_intent_ttl)
            .ok_or_else(|| {
                AppError::new(
                    ErrorCode::Internal,
                    "Call-open confirmation deadline could not be created.",
                    false,
                )
            })?;
        let mut state = self.state.lock().await;
        prune_open_intents(&mut state, now);
        let current = current_for(&state, id)?;
        if current.session.revision() != revision {
            return Err(stale_revision(current.session.revision(), revision));
        }
        if !matches!(current.session.phase(), CallPhase::Active) {
            return Err(AppError::invalid_request(
                "The door can only be opened during an active call.",
            ));
        }
        if state.pending_open.len() >= self.max_pending_open {
            return Err(AppError::new(
                ErrorCode::RateLimited,
                "Too many pending call-open confirmations.",
                true,
            ));
        }
        let intent_id = loop {
            let candidate = CallOpenIntentId::generate();
            if !state.pending_open.contains_key(&candidate) {
                break candidate;
            }
        };
        state.pending_open.insert(
            intent_id.clone(),
            PendingOpenIntent {
                call_id: id.clone(),
                revision,
                expires_at,
            },
        );
        Ok(CallOpenIntentView {
            intent_id,
            expires_at,
        })
    }

    pub async fn execute_open_intent(
        &self,
        id: &CallId,
        intent_id: &CallOpenIntentId,
        revision: CallRevision,
    ) -> Result<(), AppError> {
        let now = self.clock.now();
        let (intercom_id, call_session_id) = {
            let mut state = self.state.lock().await;
            prune_open_intents(&mut state, now);
            let intent = state
                .pending_open
                .remove(intent_id)
                .ok_or_else(open_intent_not_found)?;
            if &intent.call_id != id || intent.revision != revision {
                return Err(open_intent_not_found());
            }
            let current = current_for(&state, id)?;
            if current.session.revision() != revision {
                return Err(stale_revision(current.session.revision(), revision));
            }
            if !matches!(current.session.phase(), CallPhase::Active) {
                return Err(AppError::invalid_request(
                    "The door can only be opened during an active call.",
                ));
            }
            (current.intercom_id.clone(), current.call_session_id.clone())
        };
        self.opener.open(&intercom_id, &call_session_id).await
    }

    /// Opens the call-associated default lock without accepting provider identifiers
    /// from a browser. This is reserved for the session-bound auto-open watcher.
    pub async fn auto_open(&self, id: &CallId) -> Result<CallRevision, AppError> {
        let (revision, intercom_id, call_session_id) = {
            let state = self.state.lock().await;
            let current = current_for(&state, id)?;
            if !matches!(current.session.phase(), CallPhase::Ringing) {
                return Err(AppError::invalid_request(
                    "Automatic opening is only valid for a ringing call.",
                ));
            }
            (
                current.session.revision(),
                current.intercom_id.clone(),
                current.call_session_id.clone(),
            )
        };
        self.opener.open(&intercom_id, &call_session_id).await?;
        Ok(revision)
    }

    async fn transition(&self, id: &CallId, command: CallCommand) -> Result<CallView, AppError> {
        let now = self.clock.now();
        let mut state = self.state.lock().await;
        let current = current_for_mut(&mut state, id)?;
        let transition = reduce(&current.session, command, now).map_err(map_transition_error)?;
        current.session = transition.state;
        current.view()
    }

    async fn finish(&self, id: &CallId, reason: CallEndReason) {
        let now = self.clock.now();
        let mut state = self.state.lock().await;
        if let Some(current) = state.current.as_ref()
            && current.session.id() == id
        {
            let _ = reduce(&current.session, CallCommand::RemoteEnded(reason), now);
            state.current = None;
            state.pending_open.retain(|_, intent| &intent.call_id != id);
        }
    }
}

impl fmt::Debug for CallService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CallService")
            .field("open_intent_ttl", &self.open_intent_ttl)
            .field("max_pending_open", &self.max_pending_open)
            .finish_non_exhaustive()
    }
}

fn validate_observed(observed: &ObservedIncomingCall) -> Result<(), AppError> {
    if observed.caller_name.is_empty()
        || observed.caller_name.len() > MAX_CALLER_NAME_BYTES
        || observed.caller_name.chars().any(char::is_control)
    {
        return Err(AppError::invalid_request("invalid incoming caller name"));
    }
    validate_sdp(observed.remote_sdp.expose_secret())?;
    validate_opaque_secret(&observed.intercom_id, "intercom")?;
    validate_opaque_secret(&observed.call_session_id, "call session")
}

fn validate_sdp(sdp: &str) -> Result<(), AppError> {
    if sdp.is_empty() || sdp.len() > MAX_SDP_BYTES || !sdp.starts_with("v=") {
        return Err(AppError::invalid_request("invalid WebRTC SDP"));
    }
    Ok(())
}

fn validate_opaque_secret(value: &SecretString, name: &str) -> Result<(), AppError> {
    let value = value.expose_secret();
    if value.is_empty() || value.len() > 2 * 1024 || value.chars().any(char::is_control) {
        return Err(AppError::invalid_request(format!(
            "invalid {name} identifier"
        )));
    }
    Ok(())
}

fn current_for<'a>(state: &'a CallState, id: &CallId) -> Result<&'a ManagedCall, AppError> {
    state
        .current
        .as_ref()
        .filter(|call| call.session.id() == id)
        .ok_or_else(call_not_found)
}

fn current_for_mut<'a>(
    state: &'a mut CallState,
    id: &CallId,
) -> Result<&'a mut ManagedCall, AppError> {
    state
        .current
        .as_mut()
        .filter(|call| call.session.id() == id)
        .ok_or_else(call_not_found)
}

fn prune_open_intents(state: &mut CallState, now: DateTime<Utc>) {
    state
        .pending_open
        .retain(|_, intent| intent.expires_at > now);
}

fn map_transition_error(error: CallTransitionError) -> AppError {
    match error {
        CallTransitionError::CallNotFound(_) => call_not_found(),
        CallTransitionError::StaleRevision { expected, actual } => stale_revision(expected, actual),
        CallTransitionError::InvalidTransition(_) | CallTransitionError::ActiveCallExists(_) => {
            AppError::invalid_request("call action is not valid in the current phase")
        }
        other => AppError::new(
            ErrorCode::Internal,
            "Call state could not be updated.",
            false,
        )
        .with_cause(other),
    }
}

fn stale_revision(expected: CallRevision, actual: CallRevision) -> AppError {
    AppError::invalid_request(format!(
        "call revision is stale: expected {}, got {}",
        expected.0, actual.0
    ))
}

fn call_not_found() -> AppError {
    AppError::new(ErrorCode::CallNotFound, "Call was not found.", false)
}

fn open_intent_not_found() -> AppError {
    AppError::new(
        ErrorCode::CallOpenIntentNotFound,
        "Call-open confirmation intent was not found.",
        false,
    )
}

fn internal_state_error() -> AppError {
    AppError::new(ErrorCode::Internal, "Call state is unavailable.", false)
}
