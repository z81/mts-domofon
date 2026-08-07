use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::Duration,
};

use chrono::{DateTime, TimeDelta, Utc};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    domain::{
        auth::PersistedSession,
        auto_open::{
            AutoOpenIntentId, AutoOpenIntentView, AutoOpenMode, AutoOpenPolicyView,
            AutoOpenResultStatus, AutoOpenResultView,
        },
        call::CallId,
    },
    error::{AppError, ErrorCode},
    service::calls::{CallService, CallViewPhase},
    storage::SecretStore,
};

const DEFAULT_INTENT_TTL: Duration = Duration::from_secs(30);
const DEFAULT_UNTIL_WINDOW: Duration = Duration::from_secs(30 * 60);
const DEFAULT_MAX_PENDING_INTENTS: usize = 64;
const DEFAULT_MAX_PROCESSED_CALLS: usize = 256;

pub trait AutoOpenClock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

#[derive(Clone, Copy, Debug, Default)]
struct SystemAutoOpenClock;

impl AutoOpenClock for SystemAutoOpenClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PersistedAutoOpenState {
    policy_generation: Uuid,
    session_authenticated_at: DateTime<Utc>,
    mode: AutoOpenMode,
    until: Option<DateTime<Utc>>,
    dismiss_after_open: bool,
    processed_calls: VecDeque<CallId>,
    last_result: Option<AutoOpenResultView>,
}

impl PersistedAutoOpenState {
    fn view(&self) -> AutoOpenPolicyView {
        AutoOpenPolicyView {
            mode: self.mode,
            until: self.until,
            dismiss_after_open: self.dismiss_after_open,
            last_result: self.last_result.clone(),
        }
    }
}

struct PendingIntent {
    mode: AutoOpenMode,
    dismiss_after_open: bool,
    session_authenticated_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

#[derive(Default)]
struct AutoOpenState {
    policy: Option<PersistedAutoOpenState>,
    pending_intents: HashMap<AutoOpenIntentId, PendingIntent>,
}

pub struct AutoOpenReservation {
    call_id: CallId,
    dismiss_after_open: bool,
    policy_generation: Uuid,
    session_authenticated_at: DateTime<Utc>,
}

impl AutoOpenReservation {
    pub fn call_id(&self) -> &CallId {
        &self.call_id
    }

    pub fn dismiss_after_open(&self) -> bool {
        self.dismiss_after_open
    }
}

pub struct AutoOpenService {
    store: SecretStore,
    clock: Arc<dyn AutoOpenClock>,
    intent_ttl: TimeDelta,
    until_window: TimeDelta,
    max_pending_intents: usize,
    max_processed_calls: usize,
    state: Mutex<AutoOpenState>,
}

impl AutoOpenService {
    pub async fn restore(store: SecretStore) -> Result<Self, AppError> {
        Self::try_restore(
            store,
            Arc::new(SystemAutoOpenClock),
            DEFAULT_INTENT_TTL,
            DEFAULT_UNTIL_WINDOW,
            DEFAULT_MAX_PENDING_INTENTS,
            DEFAULT_MAX_PROCESSED_CALLS,
        )
        .await
    }

    #[cfg(feature = "test-support")]
    pub async fn restore_for_test(
        store: SecretStore,
        clock: Arc<dyn AutoOpenClock>,
        intent_ttl: Duration,
        until_window: Duration,
        max_pending_intents: usize,
        max_processed_calls: usize,
    ) -> Result<Self, AppError> {
        Self::try_restore(
            store,
            clock,
            intent_ttl,
            until_window,
            max_pending_intents,
            max_processed_calls,
        )
        .await
    }

    async fn try_restore(
        store: SecretStore,
        clock: Arc<dyn AutoOpenClock>,
        intent_ttl: Duration,
        until_window: Duration,
        max_pending_intents: usize,
        max_processed_calls: usize,
    ) -> Result<Self, AppError> {
        if intent_ttl.is_zero()
            || until_window.is_zero()
            || until_window > DEFAULT_UNTIL_WINDOW
            || max_pending_intents == 0
            || max_processed_calls == 0
        {
            return Err(AppError::invalid_request("invalid auto-open limits"));
        }
        let intent_ttl = TimeDelta::from_std(intent_ttl)
            .map_err(|_| AppError::invalid_request("invalid auto-open intent lifetime"))?;
        let until_window = TimeDelta::from_std(until_window)
            .map_err(|_| AppError::invalid_request("invalid auto-open policy lifetime"))?;
        let now = clock.now();
        let session = store.load_session().await?;
        let policy = match store.load_auto_open_state().await {
            Ok(Some(raw)) => match serde_json::from_slice::<PersistedAutoOpenState>(&raw) {
                Ok(policy)
                    if valid_persisted_policy(
                        &policy,
                        session.as_ref(),
                        now,
                        until_window,
                        max_processed_calls,
                    ) =>
                {
                    Some(policy)
                }
                _ => {
                    store.clear_auto_open_state().await?;
                    None
                }
            },
            Ok(None) => None,
            Err(_) => {
                store.clear_auto_open_state().await?;
                None
            }
        };
        Ok(Self {
            store,
            clock,
            intent_ttl,
            until_window,
            max_pending_intents,
            max_processed_calls,
            state: Mutex::new(AutoOpenState {
                policy,
                pending_intents: HashMap::new(),
            }),
        })
    }

    pub async fn policy(&self) -> Result<AutoOpenPolicyView, AppError> {
        let mut state = self.state.lock().await;
        let binding = self.reconcile(&mut state).await?;
        if binding.is_none() {
            return Ok(AutoOpenPolicyView::disabled());
        }
        Ok(state
            .policy
            .as_ref()
            .map(PersistedAutoOpenState::view)
            .unwrap_or_else(AutoOpenPolicyView::disabled))
    }

    pub async fn create_intent(
        &self,
        mode: AutoOpenMode,
        dismiss_after_open: bool,
    ) -> Result<AutoOpenIntentView, AppError> {
        if mode == AutoOpenMode::Disabled {
            return Err(AppError::invalid_request(
                "Use the disable action to turn off auto-open.",
            ));
        }
        let now = self.clock.now();
        let expires_at = now.checked_add_signed(self.intent_ttl).ok_or_else(|| {
            AppError::new(
                ErrorCode::Internal,
                "Auto-open confirmation deadline could not be created.",
                false,
            )
        })?;
        let mut state = self.state.lock().await;
        let binding = self
            .reconcile(&mut state)
            .await?
            .ok_or_else(session_expired)?;
        prune_intents(&mut state, now, binding);
        if state.pending_intents.len() >= self.max_pending_intents {
            return Err(AppError::new(
                ErrorCode::RateLimited,
                "Too many pending auto-open confirmations.",
                true,
            ));
        }
        let intent_id = loop {
            let candidate = AutoOpenIntentId::generate();
            if !state.pending_intents.contains_key(&candidate) {
                break candidate;
            }
        };
        state.pending_intents.insert(
            intent_id.clone(),
            PendingIntent {
                mode,
                dismiss_after_open,
                session_authenticated_at: binding,
                expires_at,
            },
        );
        Ok(AutoOpenIntentView {
            intent_id,
            expires_at,
        })
    }

    pub async fn execute_intent(
        &self,
        intent_id: &AutoOpenIntentId,
    ) -> Result<AutoOpenPolicyView, AppError> {
        let now = self.clock.now();
        let mut state = self.state.lock().await;
        let binding = self
            .reconcile(&mut state)
            .await?
            .ok_or_else(session_expired)?;
        prune_intents(&mut state, now, binding);
        let intent = state
            .pending_intents
            .remove(intent_id)
            .ok_or_else(intent_not_found)?;
        if intent.session_authenticated_at != binding || intent.expires_at <= now {
            return Err(intent_not_found());
        }
        let until = match intent.mode {
            AutoOpenMode::Until => {
                Some(now.checked_add_signed(self.until_window).ok_or_else(|| {
                    AppError::new(
                        ErrorCode::Internal,
                        "Auto-open deadline could not be created.",
                        false,
                    )
                })?)
            }
            AutoOpenMode::NextCall => None,
            AutoOpenMode::Disabled => return Err(intent_not_found()),
        };
        let policy = PersistedAutoOpenState {
            policy_generation: Uuid::new_v4(),
            session_authenticated_at: binding,
            mode: intent.mode,
            until,
            dismiss_after_open: intent.dismiss_after_open,
            processed_calls: VecDeque::new(),
            last_result: None,
        };
        self.persist(&policy).await?;
        let view = policy.view();
        state.policy = Some(policy);
        Ok(view)
    }

    pub async fn disable(&self) -> Result<(), AppError> {
        let mut state = self.state.lock().await;
        state.policy = None;
        state.pending_intents.clear();
        self.store.clear_auto_open_state().await
    }

    pub async fn reserve_call(
        &self,
        call_id: CallId,
    ) -> Result<Option<AutoOpenReservation>, AppError> {
        let mut state = self.state.lock().await;
        let Some(binding) = self.reconcile(&mut state).await? else {
            return Ok(None);
        };
        let Some(current) = state.policy.as_ref() else {
            return Ok(None);
        };
        let dismiss_after_open = current.dismiss_after_open;
        let policy_generation = current.policy_generation;
        match current.mode {
            AutoOpenMode::Disabled => Ok(None),
            AutoOpenMode::NextCall => {
                let consumed = PersistedAutoOpenState {
                    policy_generation,
                    session_authenticated_at: binding,
                    mode: AutoOpenMode::Disabled,
                    until: None,
                    dismiss_after_open: false,
                    processed_calls: VecDeque::new(),
                    last_result: current.last_result.clone(),
                };
                self.persist(&consumed).await?;
                state.policy = Some(consumed);
                Ok(Some(AutoOpenReservation {
                    call_id,
                    dismiss_after_open,
                    policy_generation,
                    session_authenticated_at: binding,
                }))
            }
            AutoOpenMode::Until => {
                if current.processed_calls.contains(&call_id) {
                    return Ok(None);
                }
                let mut updated = current.clone();
                if updated.processed_calls.len() == self.max_processed_calls {
                    updated.processed_calls.pop_front();
                }
                updated.processed_calls.push_back(call_id.clone());
                self.persist(&updated).await?;
                state.policy = Some(updated);
                Ok(Some(AutoOpenReservation {
                    call_id,
                    dismiss_after_open,
                    policy_generation,
                    session_authenticated_at: binding,
                }))
            }
        }
    }

    pub async fn complete(
        &self,
        reservation: AutoOpenReservation,
        result: Result<(), AppError>,
    ) -> Result<(), AppError> {
        let now = self.clock.now();
        let status = match result {
            Ok(()) => AutoOpenResultStatus::Opened,
            Err(error) if error.code == ErrorCode::MutationOutcomeUnknown => {
                AutoOpenResultStatus::OutcomeUnknown
            }
            Err(_) => AutoOpenResultStatus::Failed,
        };
        let mut state = self.state.lock().await;
        let Some(binding) = self.reconcile(&mut state).await? else {
            return Ok(());
        };
        if binding != reservation.session_authenticated_at {
            return Ok(());
        }
        let Some(current) = state.policy.as_ref() else {
            return Ok(());
        };
        if current.policy_generation != reservation.policy_generation {
            return Ok(());
        }
        let mut updated = current.clone();
        updated.last_result = Some(AutoOpenResultView {
            status,
            completed_at: now,
        });
        self.persist(&updated).await?;
        state.policy = Some(updated);
        Ok(())
    }

    async fn reconcile(
        &self,
        state: &mut AutoOpenState,
    ) -> Result<Option<DateTime<Utc>>, AppError> {
        let now = self.clock.now();
        let session = self.store.load_session().await?;
        let binding = session.as_ref().map(|session| session.authenticated_at);
        prune_intents_for_binding(state, now, binding);
        let valid = state.policy.as_ref().is_none_or(|policy| {
            valid_persisted_policy(
                policy,
                session.as_ref(),
                now,
                self.until_window,
                self.max_processed_calls,
            )
        });
        if !valid {
            state.policy = None;
            state.pending_intents.clear();
            self.store.clear_auto_open_state().await?;
        }
        Ok(binding)
    }

    async fn persist(&self, policy: &PersistedAutoOpenState) -> Result<(), AppError> {
        let bytes = serde_json::to_vec(policy).map_err(|_| storage_failure())?;
        self.store.save_auto_open_state(&bytes).await
    }
}

pub async fn run_auto_open_watcher(
    auto_open: Arc<AutoOpenService>,
    calls: Arc<CallService>,
    cancellation: CancellationToken,
    poll_interval: Duration,
) {
    let mut interval = tokio::time::interval(poll_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = cancellation.cancelled() => return,
            _ = interval.tick() => {}
        }
        let current = match calls.current().await {
            Ok(Some(call)) if call.phase == CallViewPhase::Ringing => call,
            Ok(_) => continue,
            Err(error) => {
                tracing::warn!(code = ?error.code, "auto-open could not inspect the current call");
                continue;
            }
        };
        let reservation = match auto_open.reserve_call(current.id.clone()).await {
            Ok(Some(reservation)) => reservation,
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!(code = ?error.code, "auto-open could not reserve the current call");
                continue;
            }
        };
        let dismiss_after_open = reservation.dismiss_after_open();
        let result = calls.auto_open(&current.id).await;
        let successful_revision = result.as_ref().ok().copied();
        let completion = result.map(|_| ());
        if let Err(error) = auto_open.complete(reservation, completion).await {
            tracing::warn!(code = ?error.code, "auto-open result could not be stored");
        }
        if dismiss_after_open
            && let Some(revision) = successful_revision
            && let Err(error) = calls.ignore(&current.id, revision).await
        {
            tracing::warn!(code = ?error.code, "opened call could not be dismissed");
        }
    }
}

fn valid_persisted_policy(
    policy: &PersistedAutoOpenState,
    session: Option<&PersistedSession>,
    now: DateTime<Utc>,
    until_window: TimeDelta,
    max_processed_calls: usize,
) -> bool {
    if session.map(|session| session.authenticated_at) != Some(policy.session_authenticated_at)
        || policy.processed_calls.len() > max_processed_calls
    {
        return false;
    }
    let unique = policy.processed_calls.iter().collect::<HashSet<_>>();
    if unique.len() != policy.processed_calls.len() {
        return false;
    }
    match policy.mode {
        AutoOpenMode::Disabled | AutoOpenMode::NextCall => {
            policy.until.is_none() && policy.processed_calls.is_empty()
        }
        AutoOpenMode::Until => policy.until.is_some_and(|deadline| {
            deadline > now
                && deadline
                    <= now
                        .checked_add_signed(until_window)
                        .unwrap_or(DateTime::<Utc>::MAX_UTC)
        }),
    }
}

fn prune_intents(state: &mut AutoOpenState, now: DateTime<Utc>, binding: DateTime<Utc>) {
    state
        .pending_intents
        .retain(|_, intent| intent.expires_at > now && intent.session_authenticated_at == binding);
}

fn prune_intents_for_binding(
    state: &mut AutoOpenState,
    now: DateTime<Utc>,
    binding: Option<DateTime<Utc>>,
) {
    state.pending_intents.retain(|_, intent| {
        Some(intent.session_authenticated_at) == binding && intent.expires_at > now
    });
}

fn intent_not_found() -> AppError {
    AppError::new(
        ErrorCode::AutoOpenIntentNotFound,
        "Auto-open confirmation intent was not found.",
        false,
    )
}

fn session_expired() -> AppError {
    AppError::new(ErrorCode::SessionExpired, "Session expired.", false)
}

fn storage_failure() -> AppError {
    AppError::new(
        ErrorCode::StorageFailure,
        "Auto-open state could not be stored.",
        true,
    )
}
