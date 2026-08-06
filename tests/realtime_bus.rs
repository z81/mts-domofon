use chrono::{TimeZone, Utc};
use domru_mts::domain::call::{
    CallId, CallMedia, CallSession, DomainEvent, IncomingInvite, ProviderCallId,
};
use domru_mts::realtime::{BusConfigError, EventId, RealtimeBus, SubscriptionError};
use secrecy::SecretString;
use std::sync::{Arc, Barrier};
use std::time::Duration;
use uuid::Uuid;

fn call_id(label: &str) -> CallId {
    let mut bytes = [0_u8; 16];
    for (index, byte) in label.bytes().enumerate() {
        bytes[index % bytes.len()] ^= byte;
    }
    CallId::from_uuid(Uuid::from_bytes(bytes))
}

fn event(id: &str) -> DomainEvent {
    let invite = IncomingInvite::new(
        call_id(id),
        ProviderCallId::new(SecretString::from(format!("provider-secret-{id}"))),
        CallMedia::Audio,
        SecretString::from("route-secret"),
        SecretString::from("session-secret"),
    );
    DomainEvent::CallUpdated(
        CallSession::ringing(invite, Utc.timestamp_opt(1_800_000_000, 0).unwrap()).snapshot(),
    )
}

#[tokio::test]
async fn event_ids_are_monotonic_and_receivers_get_the_envelope() {
    let bus = RealtimeBus::new(4);
    let mut subscription = bus.subscribe();

    assert_eq!(bus.publish(event("one")), EventId(1));
    assert_eq!(bus.publish(event("two")), EventId(2));

    let first = subscription.recv().await.unwrap();
    let second = subscription.recv().await.unwrap();
    assert_eq!(first.id, EventId(1));
    assert_eq!(second.id, EventId(2));
    assert_eq!(first.event, event("one"));
    assert_eq!(second.event, event("two"));
}

#[tokio::test]
async fn bounded_lag_is_visible_and_recovery_continues_at_the_oldest_retained_event() {
    let bus = RealtimeBus::new(2);
    let mut slow = bus.subscribe();
    bus.publish(event("one"));
    bus.publish(event("two"));
    bus.publish(event("three"));

    assert_eq!(
        slow.recv().await.unwrap_err(),
        SubscriptionError::Lagged { skipped: 1 }
    );
    assert_eq!(slow.recv().await.unwrap().id, EventId(2));
    assert_eq!(slow.recv().await.unwrap().id, EventId(3));
}

#[tokio::test]
async fn a_slow_subscriber_does_not_delay_or_corrupt_an_independent_subscriber() {
    let bus = RealtimeBus::new(2);
    let mut slow = bus.subscribe();
    let mut current = bus.subscribe();

    for index in 1..=3 {
        bus.publish(event(&format!("call-{index}")));
        assert_eq!(current.recv().await.unwrap().id, EventId(index));
    }

    assert_eq!(
        slow.recv().await.unwrap_err(),
        SubscriptionError::Lagged { skipped: 1 }
    );
    assert_eq!(slow.recv().await.unwrap().id, EventId(2));
}

#[tokio::test]
async fn shutdown_is_an_explicit_ordered_notification() {
    let bus = RealtimeBus::new(2);
    let mut subscription = bus.subscribe();
    bus.publish(event("last-call"));
    let shutdown_id = bus.shutdown();

    assert_eq!(subscription.recv().await.unwrap().id, EventId(1));
    let shutdown = subscription.recv().await.unwrap();
    assert_eq!(shutdown.id, shutdown_id);
    assert_eq!(shutdown.id, EventId(2));
    assert_eq!(shutdown.event, DomainEvent::Shutdown);
}

#[tokio::test]
async fn shutdown_is_terminal_for_late_subscribers_and_future_publish_calls() {
    let bus = RealtimeBus::new(4);
    let shutdown_id = bus.shutdown();
    assert_eq!(bus.publish(event("too-late")), shutdown_id);

    let mut late = bus.subscribe();
    let shutdown = tokio::time::timeout(Duration::from_millis(100), late.recv())
        .await
        .expect("late subscriber must be notified")
        .unwrap();
    assert_eq!(shutdown.id, shutdown_id);
    assert_eq!(shutdown.event, DomainEvent::Shutdown);
    assert_eq!(
        tokio::time::timeout(Duration::from_millis(100), late.recv())
            .await
            .expect("terminal subscription must close")
            .unwrap_err(),
        SubscriptionError::Closed
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_publish_and_shutdown_have_one_terminal_ordering_boundary() {
    let bus = RealtimeBus::new(4);
    let mut subscription = bus.subscribe();
    let gate = Arc::new(tokio::sync::Barrier::new(3));

    let publisher = {
        let bus = bus.clone();
        let gate = gate.clone();
        tokio::spawn(async move {
            gate.wait().await;
            bus.publish(event("racing"))
        })
    };
    let shutdown = {
        let bus = bus.clone();
        let gate = gate.clone();
        tokio::spawn(async move {
            gate.wait().await;
            bus.shutdown()
        })
    };
    gate.wait().await;

    let published_id = publisher.await.unwrap();
    let shutdown_id = shutdown.await.unwrap();
    let mut received = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_millis(100), subscription.recv())
            .await
            .expect("shutdown must terminate the subscription")
        {
            Ok(envelope) => received.push(envelope),
            Err(SubscriptionError::Closed) => break,
            Err(error) => panic!("unexpected receive error: {error}"),
        }
    }

    assert_eq!(received.last().unwrap().event, DomainEvent::Shutdown);
    assert_eq!(received.last().unwrap().id, shutdown_id);
    assert!(received.windows(2).all(|pair| pair[0].id < pair[1].id));
    if published_id == shutdown_id {
        assert_eq!(received.len(), 1, "shutdown won the race");
    } else {
        assert_eq!(received.len(), 2, "publish won the race");
        assert!(published_id < shutdown_id);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_publishers_through_clones_are_observed_in_id_order() {
    const THREADS: usize = 8;
    const EVENTS_PER_THREAD: usize = 2_000;
    let event_count = THREADS * EVENTS_PER_THREAD;
    let bus = RealtimeBus::new(event_count);
    let mut subscription = bus.subscribe();
    let gate = Arc::new(Barrier::new(THREADS));
    let shared_event = event("parallel");

    let publishers: Vec<_> = (0..THREADS)
        .map(|_| {
            let bus = bus.clone();
            let gate = gate.clone();
            let event = shared_event.clone();
            std::thread::spawn(move || {
                gate.wait();
                for _ in 0..EVENTS_PER_THREAD {
                    bus.publish(event.clone());
                }
            })
        })
        .collect();
    for publisher in publishers {
        publisher.join().unwrap();
    }

    let mut previous = EventId(0);
    for _ in 0..event_count {
        let current = subscription.recv().await.unwrap().id;
        assert!(
            current > previous,
            "received event {current:?} after {previous:?}"
        );
        previous = current;
    }
    assert_eq!(previous, EventId(event_count as u64));
}

#[test]
fn publishing_without_receivers_never_panics_and_still_allocates_ids() {
    let bus = RealtimeBus::new(1);
    assert_eq!(bus.publish(event("one")), EventId(1));
    assert_eq!(bus.publish(event("two")), EventId(2));
    assert_eq!(bus.shutdown(), EventId(3));
}

#[tokio::test]
async fn event_id_exhaustion_emits_shutdown_at_max_without_wrapping() {
    let bus = RealtimeBus::with_next_event_id(4, EventId(u64::MAX - 1)).unwrap();
    let mut subscription = bus.subscribe();

    assert_eq!(bus.publish(event("last-normal")), EventId(u64::MAX - 1));
    assert_eq!(bus.publish(event("would-wrap")), EventId(u64::MAX));
    assert_eq!(bus.publish(event("after-exhaustion")), EventId(u64::MAX));

    let last_normal = subscription.recv().await.unwrap();
    assert_eq!(last_normal.id, EventId(u64::MAX - 1));
    assert_eq!(last_normal.event, event("last-normal"));
    let shutdown = subscription.recv().await.unwrap();
    assert_eq!(shutdown.id, EventId(u64::MAX));
    assert_eq!(shutdown.event, DomainEvent::Shutdown);
    assert_eq!(
        subscription.recv().await.unwrap_err(),
        SubscriptionError::Closed
    );
}

#[test]
fn an_event_sequence_cannot_start_at_zero() {
    assert_eq!(
        RealtimeBus::with_next_event_id(1, EventId(0)).unwrap_err(),
        BusConfigError::ZeroEventId
    );
}
