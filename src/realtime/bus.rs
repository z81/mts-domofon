use std::sync::atomic::{AtomicU64, Ordering};
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
    sender: broadcast::Sender<EventEnvelope>,
    next_id: AtomicU64,
    shutdown_id: Mutex<Option<EventId>>,
}

#[derive(Clone)]
pub struct RealtimeBus {
    inner: Arc<BusInner>,
}

impl RealtimeBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(1));
        Self {
            inner: Arc::new(BusInner {
                sender,
                next_id: AtomicU64::new(1),
                shutdown_id: Mutex::new(None),
            }),
        }
    }

    pub fn publish(&self, event: DomainEvent) -> EventId {
        let id = EventId(self.inner.next_id.fetch_add(1, Ordering::Relaxed));
        let _ = self.inner.sender.send(EventEnvelope { id, event });
        id
    }

    pub fn subscribe(&self) -> EventSubscription {
        EventSubscription {
            receiver: self.inner.sender.subscribe(),
        }
    }

    pub fn shutdown(&self) -> EventId {
        let mut shutdown_id = self
            .inner
            .shutdown_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(id) = *shutdown_id {
            return id;
        }
        let id = self.publish(DomainEvent::Shutdown);
        *shutdown_id = Some(id);
        id
    }
}

pub struct EventSubscription {
    receiver: broadcast::Receiver<EventEnvelope>,
}

impl EventSubscription {
    pub async fn recv(&mut self) -> Result<EventEnvelope, SubscriptionError> {
        self.receiver.recv().await.map_err(|error| match error {
            broadcast::error::RecvError::Closed => SubscriptionError::Closed,
            broadcast::error::RecvError::Lagged(skipped) => SubscriptionError::Lagged { skipped },
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SubscriptionError {
    #[error("realtime subscription lagged by {skipped} events")]
    Lagged { skipped: u64 },
    #[error("realtime subscription closed")]
    Closed,
}
