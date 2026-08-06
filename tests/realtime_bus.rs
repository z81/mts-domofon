use chrono::{TimeZone, Utc};
use domru_mts::domain::call::{CallId, CallMedia, CallSession, DomainEvent, IncomingInvite};
use domru_mts::realtime::{EventId, RealtimeBus, SubscriptionError};
use secrecy::SecretString;

fn event(id: &str) -> DomainEvent {
    let invite = IncomingInvite::new(
        CallId::new(id),
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

#[test]
fn publishing_without_receivers_never_panics_and_still_allocates_ids() {
    let bus = RealtimeBus::new(1);
    assert_eq!(bus.publish(event("one")), EventId(1));
    assert_eq!(bus.publish(event("two")), EventId(2));
    assert_eq!(bus.shutdown(), EventId(3));
}
