use std::fmt;
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;

use crate::domain::call::DomainEvent;

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct EventId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EventEnvelope {
    pub id: EventId,
    pub event: DomainEvent,
}

struct BusInner {
    state: Mutex<BusState>,
}

struct BusState {
    sender: broadcast::Sender<EventEnvelope>,
    next_id: u64,
    shutdown: Option<EventEnvelope>,
}

#[derive(Clone)]
pub struct RealtimeBus {
    inner: Arc<BusInner>,
}

impl RealtimeBus {
    pub fn new(capacity: usize) -> Self {
        Self::build(capacity, 1)
    }

    pub fn with_next_event_id(capacity: usize, next_id: EventId) -> Result<Self, BusConfigError> {
        if next_id.0 == 0 {
            return Err(BusConfigError::ZeroEventId);
        }
        Ok(Self::build(capacity, next_id.0))
    }

    fn build(capacity: usize, next_id: u64) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(1));
        Self {
            inner: Arc::new(BusInner {
                state: Mutex::new(BusState {
                    sender,
                    next_id,
                    shutdown: None,
                }),
            }),
        }
    }

    /// Publishes one event in ID order.
    ///
    /// Once shutdown wins the ordering lock, later calls do not publish and return the existing
    /// shutdown ID. If the ID space is exhausted, ID `u64::MAX` is used for an ordered shutdown
    /// notification instead of allowing the sequence to wrap.
    pub fn publish(&self, event: DomainEvent) -> EventId {
        let mut state = self.lock_state();
        if let Some(shutdown) = &state.shutdown {
            return shutdown.id;
        }
        if matches!(&event, DomainEvent::Shutdown) {
            return Self::shutdown_locked(&mut state);
        }
        if state.next_id == u64::MAX {
            return Self::shutdown_locked(&mut state);
        }
        let id = EventId(state.next_id);
        state.next_id += 1;
        let _ = state.sender.send(EventEnvelope { id, event });
        id
    }

    pub fn subscribe(&self) -> EventSubscription {
        let state = self.lock_state();
        EventSubscription {
            receiver: state.sender.subscribe(),
            pending_terminal: state.shutdown.clone(),
            terminal_received: false,
        }
    }

    pub fn shutdown(&self) -> EventId {
        let mut state = self.lock_state();
        if let Some(shutdown) = &state.shutdown {
            return shutdown.id;
        }
        Self::shutdown_locked(&mut state)
    }

    fn shutdown_locked(state: &mut BusState) -> EventId {
        let id = EventId(state.next_id);
        let shutdown = EventEnvelope {
            id,
            event: DomainEvent::Shutdown,
        };
        let _ = state.sender.send(shutdown.clone());
        state.shutdown = Some(shutdown);
        id
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, BusState> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl fmt::Debug for RealtimeBus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RealtimeBus")
            .finish_non_exhaustive()
    }
}

pub struct EventSubscription {
    receiver: broadcast::Receiver<EventEnvelope>,
    pending_terminal: Option<EventEnvelope>,
    terminal_received: bool,
}

impl EventSubscription {
    pub async fn recv(&mut self) -> Result<EventEnvelope, SubscriptionError> {
        if self.terminal_received {
            return Err(SubscriptionError::Closed);
        }
        if let Some(shutdown) = self.pending_terminal.take() {
            self.terminal_received = true;
            return Ok(shutdown);
        }

        match self.receiver.recv().await {
            Ok(envelope) => {
                if envelope.event == DomainEvent::Shutdown {
                    self.terminal_received = true;
                }
                Ok(envelope)
            }
            Err(broadcast::error::RecvError::Closed) => Err(SubscriptionError::Closed),
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                Err(SubscriptionError::Lagged { skipped })
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SubscriptionError {
    #[error("realtime subscription lagged by {skipped} events")]
    Lagged { skipped: u64 },
    #[error("realtime subscription closed")]
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BusConfigError {
    #[error("the first realtime event ID must be non-zero")]
    ZeroEventId,
}
