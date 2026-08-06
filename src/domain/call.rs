use std::collections::{BTreeMap, VecDeque};
use std::fmt;

use chrono::{DateTime, Utc};
use secrecy::SecretString;

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct CallId(uuid::Uuid);

impl CallId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    pub fn from_uuid(value: uuid::Uuid) -> Self {
        Self(value)
    }

    pub fn parse(value: &str) -> Result<Self, uuid::Error> {
        uuid::Uuid::parse_str(value).map(Self)
    }

    pub fn as_uuid(&self) -> uuid::Uuid {
        self.0
    }
}

impl Default for CallId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for CallId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct CallRevision(pub u64);

impl CallRevision {
    pub fn checked_next(self) -> Result<Self, CallTransitionError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(CallTransitionError::RevisionExhausted)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallMedia {
    Audio,
    AudioVideo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallEndReason {
    Cancelled,
    LocalHangup,
    Missed,
    Rejected,
    RemoteHangup,
    Timeout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallPhase {
    Ringing,
    Connecting,
    Active,
    Ending,
    Ended(CallEndReason),
}

#[derive(Clone)]
pub struct ProviderCallId(SecretString);

impl ProviderCallId {
    pub fn new(value: SecretString) -> Self {
        Self(value)
    }

    pub fn as_secret(&self) -> &SecretString {
        &self.0
    }
}

impl fmt::Debug for ProviderCallId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProviderCallId([REDACTED])")
    }
}

#[derive(Clone)]
pub struct IncomingInvite {
    id: CallId,
    provider_call_id: ProviderCallId,
    media: CallMedia,
    action_route_id: SecretString,
    call_session_id: SecretString,
    caller_phone: Option<SecretString>,
    address: Option<SecretString>,
}

impl IncomingInvite {
    pub fn new(
        id: CallId,
        provider_call_id: ProviderCallId,
        media: CallMedia,
        action_route_id: SecretString,
        call_session_id: SecretString,
    ) -> Self {
        Self {
            id,
            provider_call_id,
            media,
            action_route_id,
            call_session_id,
            caller_phone: None,
            address: None,
        }
    }

    pub fn with_caller_phone(mut self, caller_phone: SecretString) -> Self {
        self.caller_phone = Some(caller_phone);
        self
    }

    pub fn with_address(mut self, address: SecretString) -> Self {
        self.address = Some(address);
        self
    }

    pub fn id(&self) -> &CallId {
        &self.id
    }

    pub fn media(&self) -> CallMedia {
        self.media
    }

    pub fn provider_call_id(&self) -> &ProviderCallId {
        &self.provider_call_id
    }

    pub fn action_route_id(&self) -> &SecretString {
        &self.action_route_id
    }

    pub fn call_session_id(&self) -> &SecretString {
        &self.call_session_id
    }
}

impl fmt::Debug for IncomingInvite {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IncomingInvite")
            .field("id", &self.id)
            .field("media", &self.media)
            .field("has_provider_call_id", &true)
            .field("has_action_route_id", &true)
            .field("has_call_session_id", &true)
            .field("has_caller_phone", &self.caller_phone.is_some())
            .field("has_address", &self.address.is_some())
            .finish()
    }
}

#[derive(Clone, Debug)]
pub enum CallCommand {
    ObserveInvite(IncomingInvite),
    Accept {
        revision: CallRevision,
    },
    Reject {
        revision: CallRevision,
    },
    Ignore {
        revision: CallRevision,
    },
    Hangup {
        revision: CallRevision,
    },
    RemoteEnded(CallEndReason),
    Timeout,
    /// Internal, provider-neutral confirmation that the accepted media leg connected.
    Connected {
        revision: CallRevision,
    },
}

#[derive(Clone)]
pub struct CallSession {
    invite: IncomingInvite,
    revision: CallRevision,
    phase: CallPhase,
    observed_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    media_reserved: bool,
}

impl CallSession {
    pub fn ringing(invite: IncomingInvite, now: DateTime<Utc>) -> Self {
        Self {
            invite,
            revision: CallRevision(1),
            phase: CallPhase::Ringing,
            observed_at: now,
            updated_at: now,
            media_reserved: false,
        }
    }

    pub fn id(&self) -> &CallId {
        self.invite.id()
    }

    pub fn revision(&self) -> CallRevision {
        self.revision
    }

    pub fn phase(&self) -> &CallPhase {
        &self.phase
    }

    pub fn observed_at(&self) -> DateTime<Utc> {
        self.observed_at
    }

    pub fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    pub fn snapshot(&self) -> CallSnapshot {
        CallSnapshot {
            id: self.id().clone(),
            revision: self.revision,
            phase: self.phase,
            media: self.invite.media(),
            observed_at: self.observed_at,
            updated_at: self.updated_at,
        }
    }

    fn is_terminal(&self) -> bool {
        matches!(self.phase, CallPhase::Ended(_))
    }
}

impl fmt::Debug for CallSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CallSession")
            .field("snapshot", &self.snapshot())
            .field("media_reserved", &self.media_reserved)
            .finish()
    }
}

impl PartialEq for CallSession {
    fn eq(&self, other: &Self) -> bool {
        self.snapshot() == other.snapshot() && self.media_reserved == other.media_reserved
    }
}

impl Eq for CallSession {}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CallSnapshot {
    pub id: CallId,
    pub revision: CallRevision,
    pub phase: CallPhase,
    pub media: CallMedia,
    pub observed_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "call", rename_all = "snake_case")]
pub enum DomainEvent {
    CallUpdated(CallSnapshot),
    Shutdown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallEffect {
    None,
    Accept,
    Reject,
    Ignore,
    Hangup,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transition {
    pub state: CallSession,
    pub effect: CallEffect,
    pub event: DomainEvent,
    pub changed: bool,
}

impl Transition {
    fn unchanged(state: &CallSession) -> Self {
        Self {
            state: state.clone(),
            effect: CallEffect::None,
            event: DomainEvent::CallUpdated(state.snapshot()),
            changed: false,
        }
    }

    fn changed(state: CallSession, effect: CallEffect) -> Self {
        let event = DomainEvent::CallUpdated(state.snapshot());
        Self {
            state,
            effect,
            event,
            changed: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CallTransitionError {
    #[error("call ID does not match the observed session")]
    CallIdMismatch,
    #[error("call revision is stale: expected {expected:?}, got {actual:?}")]
    StaleRevision {
        expected: CallRevision,
        actual: CallRevision,
    },
    #[error("call revision space is exhausted")]
    RevisionExhausted,
    #[error("command is invalid while call is in phase {0:?}")]
    InvalidTransition(CallPhase),
    #[error("call timestamp moved backwards")]
    NonMonotonicTimestamp {
        previous: DateTime<Utc>,
        supplied: DateTime<Utc>,
    },
    #[error("another call already owns the media slot: {0:?}")]
    ActiveCallExists(CallId),
    #[error("call was not found: {0:?}")]
    CallNotFound(CallId),
}

pub fn reduce(
    state: &CallSession,
    command: CallCommand,
    now: DateTime<Utc>,
) -> Result<Transition, CallTransitionError> {
    if now < state.updated_at {
        return Err(CallTransitionError::NonMonotonicTimestamp {
            previous: state.updated_at,
            supplied: now,
        });
    }

    if let CallCommand::ObserveInvite(invite) = &command
        && invite.id() != state.id()
    {
        return Err(CallTransitionError::CallIdMismatch);
    }

    if state.is_terminal() {
        return Ok(Transition::unchanged(state));
    }

    if let Some(actual) = command.revision()
        && actual != state.revision
    {
        return Err(CallTransitionError::StaleRevision {
            expected: state.revision,
            actual,
        });
    }

    let (phase, effect, media_reserved) = match (&state.phase, command) {
        (_, CallCommand::ObserveInvite(_)) => return Ok(Transition::unchanged(state)),
        (CallPhase::Ringing, CallCommand::Accept { .. }) => {
            (CallPhase::Connecting, CallEffect::Accept, true)
        }
        (CallPhase::Ringing, CallCommand::Reject { .. }) => {
            (CallPhase::Ending, CallEffect::Reject, false)
        }
        (CallPhase::Ringing, CallCommand::Ignore { .. }) => {
            (CallPhase::Ringing, CallEffect::Ignore, false)
        }
        (CallPhase::Connecting, CallCommand::Connected { .. }) => {
            (CallPhase::Active, CallEffect::None, true)
        }
        (CallPhase::Connecting | CallPhase::Active, CallCommand::Hangup { .. }) => {
            (CallPhase::Ending, CallEffect::Hangup, true)
        }
        (CallPhase::Ringing, CallCommand::Timeout) => (
            CallPhase::Ended(CallEndReason::Missed),
            CallEffect::None,
            false,
        ),
        (CallPhase::Connecting | CallPhase::Active | CallPhase::Ending, CallCommand::Timeout) => (
            CallPhase::Ended(CallEndReason::Timeout),
            CallEffect::None,
            false,
        ),
        (_, CallCommand::RemoteEnded(reason)) => {
            (CallPhase::Ended(reason), CallEffect::None, false)
        }
        (phase, _) => return Err(CallTransitionError::InvalidTransition(*phase)),
    };

    let mut next = state.clone();
    next.revision = next.revision.checked_next()?;
    next.phase = phase;
    next.updated_at = now;
    next.media_reserved = media_reserved;
    Ok(Transition::changed(next, effect))
}

impl CallCommand {
    fn revision(&self) -> Option<CallRevision> {
        match self {
            Self::Accept { revision }
            | Self::Reject { revision }
            | Self::Ignore { revision }
            | Self::Hangup { revision }
            | Self::Connected { revision } => Some(*revision),
            Self::ObserveInvite(_) | Self::RemoteEnded(_) | Self::Timeout => None,
        }
    }
}

pub struct CallRegistry {
    current: BTreeMap<CallId, CallSession>,
    history: VecDeque<CallSession>,
    history_capacity: usize,
}

impl CallRegistry {
    pub fn new(history_capacity: usize) -> Self {
        let history_capacity = history_capacity.max(1);
        Self {
            current: BTreeMap::new(),
            history: VecDeque::with_capacity(history_capacity),
            history_capacity,
        }
    }

    pub fn observe_invite(
        &mut self,
        invite: IncomingInvite,
        now: DateTime<Utc>,
    ) -> Result<Transition, CallTransitionError> {
        let id = invite.id().clone();
        if self.current.contains_key(&id) || self.history.iter().any(|call| call.id() == &id) {
            return self.apply(&id, CallCommand::ObserveInvite(invite), now);
        }

        let state = CallSession::ringing(invite, now);
        let transition = Transition::changed(state.clone(), CallEffect::None);
        self.current.insert(id, state);
        Ok(transition)
    }

    pub fn apply(
        &mut self,
        id: &CallId,
        command: CallCommand,
        now: DateTime<Utc>,
    ) -> Result<Transition, CallTransitionError> {
        if let Some(state) = self.current.get(id) {
            let previously_reserved = state.media_reserved;
            let transition = reduce(state, command, now)?;
            if !previously_reserved
                && transition.state.media_reserved
                && let Some(owner) = self
                    .current
                    .values()
                    .find(|call| call.id() != id && call.media_reserved)
            {
                return Err(CallTransitionError::ActiveCallExists(owner.id().clone()));
            }
            if transition.state.is_terminal() {
                self.current.remove(id);
                self.push_history(transition.state.clone());
            } else {
                self.current.insert(id.clone(), transition.state.clone());
            }
            return Ok(transition);
        }

        if let Some(state) = self.history.iter().find(|call| call.id() == id) {
            return reduce(state, command, now);
        }

        Err(CallTransitionError::CallNotFound(id.clone()))
    }

    pub fn session(&self, id: &CallId) -> Option<&CallSession> {
        self.current
            .get(id)
            .or_else(|| self.history.iter().find(|call| call.id() == id))
    }

    pub fn ringing(&self) -> impl Iterator<Item = &CallSession> {
        self.current
            .values()
            .filter(|call| matches!(call.phase, CallPhase::Ringing))
    }

    pub fn history(&self) -> impl Iterator<Item = &CallSession> {
        self.history.iter()
    }

    fn push_history(&mut self, state: CallSession) {
        if self.history_capacity == 0 {
            return;
        }
        if self.history.len() == self.history_capacity {
            self.history.pop_front();
        }
        self.history.push_back(state);
    }
}
