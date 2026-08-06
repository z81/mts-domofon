use chrono::{Duration, TimeZone, Utc};
use domru_mts::domain::call::{
    CallCommand, CallEffect, CallEndReason, CallId, CallMedia, CallPhase, CallRegistry,
    CallRevision, CallSession, CallTransitionError, DomainEvent, IncomingInvite, reduce,
};
use secrecy::SecretString;

fn at(second: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(1_800_000_000 + second, 0).unwrap()
}

fn invite(id: &str) -> IncomingInvite {
    IncomingInvite::new(
        CallId::new(id),
        CallMedia::AudioVideo,
        SecretString::from("route-secret-701"),
        SecretString::from("session-secret-702"),
    )
    .with_caller_phone(SecretString::from("+79990001122"))
    .with_address(SecretString::from("Secret street, 42"))
}

fn ringing(id: &str) -> CallSession {
    CallSession::ringing(invite(id), at(0))
}

#[test]
fn legal_transitions_cover_the_complete_call_lifecycle() {
    struct Case {
        name: &'static str,
        state: CallSession,
        command: CallCommand,
        want_phase: CallPhase,
        want_effect: CallEffect,
    }

    let connecting = reduce(
        &ringing("connecting"),
        CallCommand::Accept {
            revision: CallRevision(1),
        },
        at(1),
    )
    .unwrap()
    .state;
    let active = reduce(
        &connecting,
        CallCommand::Connected {
            revision: CallRevision(2),
        },
        at(2),
    )
    .unwrap()
    .state;
    let ending = reduce(
        &active,
        CallCommand::Hangup {
            revision: CallRevision(3),
        },
        at(3),
    )
    .unwrap()
    .state;

    let cases = vec![
        Case {
            name: "accept reserves media while connecting",
            state: ringing("accept"),
            command: CallCommand::Accept {
                revision: CallRevision(1),
            },
            want_phase: CallPhase::Connecting,
            want_effect: CallEffect::Accept,
        },
        Case {
            name: "reject starts ending",
            state: ringing("reject"),
            command: CallCommand::Reject {
                revision: CallRevision(1),
            },
            want_phase: CallPhase::Ending,
            want_effect: CallEffect::Reject,
        },
        Case {
            name: "ignore leaves the SIP call ringing",
            state: ringing("ignore"),
            command: CallCommand::Ignore {
                revision: CallRevision(1),
            },
            want_phase: CallPhase::Ringing,
            want_effect: CallEffect::Ignore,
        },
        Case {
            name: "remote cancel ends a ringer",
            state: ringing("cancel"),
            command: CallCommand::RemoteEnded(CallEndReason::Cancelled),
            want_phase: CallPhase::Ended(CallEndReason::Cancelled),
            want_effect: CallEffect::None,
        },
        Case {
            name: "ring timeout records a missed call",
            state: ringing("missed"),
            command: CallCommand::Timeout,
            want_phase: CallPhase::Ended(CallEndReason::Missed),
            want_effect: CallEffect::None,
        },
        Case {
            name: "connection confirmation activates media",
            state: connecting,
            command: CallCommand::Connected {
                revision: CallRevision(2),
            },
            want_phase: CallPhase::Active,
            want_effect: CallEffect::None,
        },
        Case {
            name: "hangup starts ending",
            state: active,
            command: CallCommand::Hangup {
                revision: CallRevision(3),
            },
            want_phase: CallPhase::Ending,
            want_effect: CallEffect::Hangup,
        },
        Case {
            name: "remote confirmation completes ending",
            state: ending,
            command: CallCommand::RemoteEnded(CallEndReason::LocalHangup),
            want_phase: CallPhase::Ended(CallEndReason::LocalHangup),
            want_effect: CallEffect::None,
        },
    ];

    for case in cases {
        let transition = reduce(
            &case.state,
            case.command,
            case.state.updated_at() + Duration::seconds(1),
        )
        .unwrap_or_else(|error| panic!("{}: {error}", case.name));
        assert_eq!(transition.state.phase(), &case.want_phase, "{}", case.name);
        assert_eq!(transition.effect, case.want_effect, "{}", case.name);
        assert_eq!(
            transition.state.revision(),
            CallRevision(case.state.revision().0 + 1)
        );
        assert!(matches!(transition.event, DomainEvent::CallUpdated(_)));
    }
}

#[test]
fn duplicate_invite_is_idempotent_and_a_different_id_is_rejected() {
    let state = ringing("same-call");

    let duplicate = reduce(
        &state,
        CallCommand::ObserveInvite(invite("same-call")),
        at(1),
    )
    .unwrap();
    assert!(!duplicate.changed);
    assert_eq!(duplicate.state.revision(), CallRevision(1));
    assert_eq!(duplicate.effect, CallEffect::None);

    assert_eq!(
        reduce(
            &state,
            CallCommand::ObserveInvite(invite("different-call")),
            at(1),
        )
        .unwrap_err(),
        CallTransitionError::CallIdMismatch
    );
}

#[test]
fn stale_browser_revision_cannot_mutate_a_nonterminal_call() {
    let error = reduce(
        &ringing("stale"),
        CallCommand::Accept {
            revision: CallRevision(0),
        },
        at(1),
    )
    .unwrap_err();

    assert_eq!(
        error,
        CallTransitionError::StaleRevision {
            expected: CallRevision(1),
            actual: CallRevision(0),
        }
    );
}

#[test]
fn accept_timeout_race_always_preserves_the_first_terminal_outcome() {
    let state = ringing("accept-timeout");
    let connecting = reduce(
        &state,
        CallCommand::Accept {
            revision: CallRevision(1),
        },
        at(1),
    )
    .unwrap()
    .state;
    let timed_out = reduce(&connecting, CallCommand::Timeout, at(2))
        .unwrap()
        .state;
    assert_eq!(timed_out.phase(), &CallPhase::Ended(CallEndReason::Timeout));

    let timeout_first = reduce(&state, CallCommand::Timeout, at(1)).unwrap().state;
    let late_accept = reduce(
        &timeout_first,
        CallCommand::Accept {
            revision: CallRevision(1),
        },
        at(2),
    )
    .unwrap();
    assert!(!late_accept.changed);
    assert_eq!(
        late_accept.state.phase(),
        &CallPhase::Ended(CallEndReason::Missed)
    );
}

#[test]
fn reject_remote_end_race_is_terminal_and_idempotent() {
    let state = ringing("reject-bye");
    let ending = reduce(
        &state,
        CallCommand::Reject {
            revision: CallRevision(1),
        },
        at(1),
    )
    .unwrap()
    .state;
    let remote_won = reduce(
        &ending,
        CallCommand::RemoteEnded(CallEndReason::RemoteHangup),
        at(2),
    )
    .unwrap()
    .state;
    assert_eq!(
        remote_won.phase(),
        &CallPhase::Ended(CallEndReason::RemoteHangup)
    );

    let repeat = reduce(&remote_won, CallCommand::Timeout, at(3)).unwrap();
    assert!(!repeat.changed);
    assert_eq!(repeat.state, remote_won);
}

#[test]
fn multiple_ringers_are_allowed_but_only_one_can_reserve_media() {
    let mut calls = CallRegistry::new(4);
    calls.observe_invite(invite("first"), at(0)).unwrap();
    calls.observe_invite(invite("second"), at(1)).unwrap();
    assert_eq!(calls.ringing().count(), 2);

    calls
        .apply(
            &CallId::new("first"),
            CallCommand::Accept {
                revision: CallRevision(1),
            },
            at(2),
        )
        .unwrap();
    let error = calls
        .apply(
            &CallId::new("second"),
            CallCommand::Accept {
                revision: CallRevision(1),
            },
            at(3),
        )
        .unwrap_err();
    assert_eq!(
        error,
        CallTransitionError::ActiveCallExists(CallId::new("first"))
    );
    assert_eq!(
        calls.session(&CallId::new("second")).unwrap().phase(),
        &CallPhase::Ringing
    );
}

#[test]
fn timestamps_never_move_backwards() {
    let state = ringing("time");
    let error = reduce(
        &state,
        CallCommand::Reject {
            revision: CallRevision(1),
        },
        at(-1),
    )
    .unwrap_err();
    assert_eq!(
        error,
        CallTransitionError::NonMonotonicTimestamp {
            previous: at(0),
            supplied: at(-1),
        }
    );
}

#[test]
fn terminal_history_is_bounded_and_terminal_commands_do_not_resurrect_calls() {
    let mut calls = CallRegistry::new(2);
    for (index, id) in ["oldest", "middle", "newest"].into_iter().enumerate() {
        calls
            .observe_invite(invite(id), at(index as i64 * 2))
            .unwrap();
        calls
            .apply(
                &CallId::new(id),
                CallCommand::Timeout,
                at(index as i64 * 2 + 1),
            )
            .unwrap();
    }

    let ids: Vec<_> = calls
        .history()
        .map(|session| session.id().as_str())
        .collect();
    assert_eq!(ids, vec!["middle", "newest"]);

    let repeat = calls
        .apply(
            &CallId::new("newest"),
            CallCommand::RemoteEnded(CallEndReason::RemoteHangup),
            at(10),
        )
        .unwrap();
    assert!(!repeat.changed);
    assert_eq!(
        repeat.state.phase(),
        &CallPhase::Ended(CallEndReason::Missed)
    );
}

#[test]
fn public_events_are_redacted_and_observation_has_no_door_effect() {
    let session = CallSession::ringing(invite("public-id"), at(0));
    let observed = reduce(
        &session,
        CallCommand::ObserveInvite(invite("public-id")),
        at(1),
    )
    .unwrap();
    assert_eq!(observed.effect, CallEffect::None);

    let json = serde_json::to_string(&observed.event).unwrap();
    for forbidden in [
        "route-secret-701",
        "session-secret-702",
        "+79990001122",
        "Secret street, 42",
        "sip",
        "sdp",
    ] {
        assert!(
            !json
                .to_ascii_lowercase()
                .contains(&forbidden.to_ascii_lowercase())
        );
    }
    assert_eq!(
        json,
        r#"{"type":"call_updated","call":{"id":"public-id","revision":1,"phase":"ringing","media":"audio_video","observed_at":"2027-01-15T08:00:00Z","updated_at":"2027-01-15T08:00:00Z"}}"#
    );
}
