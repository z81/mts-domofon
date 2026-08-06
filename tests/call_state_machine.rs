use chrono::{Duration, TimeZone, Utc};
use domru_mts::domain::call::{
    CallCommand, CallEffect, CallEndReason, CallId, CallMedia, CallPhase, CallRegistry,
    CallRevision, CallSession, CallTransitionError, DomainEvent, IncomingInvite, ProviderCallId,
    reduce,
};
use secrecy::SecretString;
use uuid::Uuid;

fn at(second: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(1_800_000_000 + second, 0).unwrap()
}

fn call_id(label: &str) -> CallId {
    let mut bytes = [0_u8; 16];
    for (index, byte) in label.bytes().enumerate() {
        bytes[index % bytes.len()] ^= byte;
    }
    CallId::from_uuid(Uuid::from_bytes(bytes))
}

fn invite_with_id(id: CallId, provider_id: &str) -> IncomingInvite {
    IncomingInvite::new(
        id,
        ProviderCallId::new(SecretString::from(provider_id)),
        CallMedia::AudioVideo,
        SecretString::from("route-secret-701"),
        SecretString::from("session-secret-702"),
    )
    .with_caller_phone(SecretString::from("+79990001122"))
    .with_address(SecretString::from("Secret street, 42"))
}

fn invite(id: &str) -> IncomingInvite {
    invite_with_id(call_id(id), &format!("provider-call-id-secret-{id}"))
}

fn ringing(id: &str) -> CallSession {
    CallSession::ringing(invite(id), at(0))
}

fn connecting(id: &str) -> CallSession {
    reduce(
        &ringing(id),
        CallCommand::Accept {
            revision: CallRevision(1),
        },
        at(1),
    )
    .unwrap()
    .state
}

fn active(id: &str) -> CallSession {
    reduce(
        &connecting(id),
        CallCommand::Connected {
            revision: CallRevision(2),
        },
        at(2),
    )
    .unwrap()
    .state
}

fn ending(id: &str) -> CallSession {
    reduce(
        &active(id),
        CallCommand::Hangup {
            revision: CallRevision(3),
        },
        at(3),
    )
    .unwrap()
    .state
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
fn remote_end_before_reject_preserves_the_remote_terminal_reason() {
    let remote_first = reduce(
        &ringing("remote-first"),
        CallCommand::RemoteEnded(CallEndReason::RemoteHangup),
        at(1),
    )
    .unwrap()
    .state;

    let late_reject = reduce(
        &remote_first,
        CallCommand::Reject {
            revision: CallRevision(1),
        },
        at(2),
    )
    .unwrap();
    assert!(!late_reject.changed);
    assert_eq!(late_reject.state, remote_first);
}

#[test]
fn every_nonterminal_phase_handles_remote_end_and_duplicate_invites() {
    for (name, id, state) in [
        ("ringing", "ringing-remote", ringing("ringing-remote")),
        (
            "connecting",
            "connecting-remote",
            connecting("connecting-remote"),
        ),
        ("active", "active-remote", active("active-remote")),
        ("ending", "ending-remote", ending("ending-remote")),
    ] {
        let duplicate = reduce(
            &state,
            CallCommand::ObserveInvite(invite(id)),
            state.updated_at() + Duration::seconds(1),
        )
        .unwrap_or_else(|error| panic!("duplicate in {name}: {error}"));
        assert!(!duplicate.changed, "duplicate in {name}");
        assert_eq!(duplicate.state, state, "duplicate in {name}");

        let remote = reduce(
            &state,
            CallCommand::RemoteEnded(CallEndReason::RemoteHangup),
            state.updated_at() + Duration::seconds(1),
        )
        .unwrap_or_else(|error| panic!("remote end in {name}: {error}"));
        assert_eq!(
            remote.state.phase(),
            &CallPhase::Ended(CallEndReason::RemoteHangup),
            "remote end in {name}"
        );
    }
}

#[test]
fn connecting_hangup_and_active_or_ending_timeout_are_legal() {
    let hanging_up = reduce(
        &connecting("connecting-hangup"),
        CallCommand::Hangup {
            revision: CallRevision(2),
        },
        at(2),
    )
    .unwrap();
    assert_eq!(hanging_up.state.phase(), &CallPhase::Ending);
    assert_eq!(hanging_up.effect, CallEffect::Hangup);

    for (name, state) in [
        ("active", active("active-timeout")),
        ("ending", ending("ending-timeout")),
    ] {
        let timed_out = reduce(
            &state,
            CallCommand::Timeout,
            state.updated_at() + Duration::seconds(1),
        )
        .unwrap_or_else(|error| panic!("timeout in {name}: {error}"));
        assert_eq!(
            timed_out.state.phase(),
            &CallPhase::Ended(CallEndReason::Timeout),
            "timeout in {name}"
        );
    }
}

#[test]
fn phase_invalid_commands_are_rejected_without_mutating_state() {
    struct Case {
        name: &'static str,
        state: CallSession,
        command: CallCommand,
    }

    let cases = vec![
        Case {
            name: "ringing cannot hang up",
            state: ringing("invalid-ring-hangup"),
            command: CallCommand::Hangup {
                revision: CallRevision(1),
            },
        },
        Case {
            name: "ringing cannot connect before accept",
            state: ringing("invalid-ring-connected"),
            command: CallCommand::Connected {
                revision: CallRevision(1),
            },
        },
        Case {
            name: "connecting cannot accept twice",
            state: connecting("invalid-connecting-accept"),
            command: CallCommand::Accept {
                revision: CallRevision(2),
            },
        },
        Case {
            name: "active cannot reject",
            state: active("invalid-active-reject"),
            command: CallCommand::Reject {
                revision: CallRevision(3),
            },
        },
        Case {
            name: "ending cannot accept",
            state: ending("invalid-ending-accept"),
            command: CallCommand::Accept {
                revision: CallRevision(4),
            },
        },
        Case {
            name: "ending cannot hang up twice",
            state: ending("invalid-ending-hangup"),
            command: CallCommand::Hangup {
                revision: CallRevision(4),
            },
        },
    ];

    for case in cases {
        let want_phase = *case.state.phase();
        let error = reduce(
            &case.state,
            case.command,
            case.state.updated_at() + Duration::seconds(1),
        )
        .unwrap_err();
        assert_eq!(
            error,
            CallTransitionError::InvalidTransition(want_phase),
            "{}",
            case.name
        );
    }
}

#[test]
fn multiple_ringers_are_allowed_but_only_one_can_reserve_media() {
    let mut calls = CallRegistry::new(4);
    calls.observe_invite(invite("first"), at(0)).unwrap();
    calls.observe_invite(invite("second"), at(1)).unwrap();
    assert_eq!(calls.ringing().count(), 2);

    calls
        .apply(
            &call_id("first"),
            CallCommand::Accept {
                revision: CallRevision(1),
            },
            at(2),
        )
        .unwrap();
    let error = calls
        .apply(
            &call_id("second"),
            CallCommand::Accept {
                revision: CallRevision(1),
            },
            at(3),
        )
        .unwrap_err();
    assert_eq!(
        error,
        CallTransitionError::ActiveCallExists(call_id("first"))
    );
    assert_eq!(
        calls.session(&call_id("second")).unwrap().phase(),
        &CallPhase::Ringing
    );
}

fn registry_with_media_owner() -> CallRegistry {
    let mut calls = CallRegistry::new(8);
    calls.observe_invite(invite("owner"), at(0)).unwrap();
    calls.observe_invite(invite("target"), at(1)).unwrap();
    calls
        .apply(
            &call_id("owner"),
            CallCommand::Accept {
                revision: CallRevision(1),
            },
            at(2),
        )
        .unwrap();
    calls
}

#[test]
fn media_owner_check_does_not_mask_missing_stale_or_nonmonotonic_targets() {
    let mut missing = registry_with_media_owner();
    assert_eq!(
        missing
            .apply(
                &call_id("missing"),
                CallCommand::Accept {
                    revision: CallRevision(1),
                },
                at(3),
            )
            .unwrap_err(),
        CallTransitionError::CallNotFound(call_id("missing"))
    );

    let mut stale = registry_with_media_owner();
    assert_eq!(
        stale
            .apply(
                &call_id("target"),
                CallCommand::Accept {
                    revision: CallRevision(0),
                },
                at(3),
            )
            .unwrap_err(),
        CallTransitionError::StaleRevision {
            expected: CallRevision(1),
            actual: CallRevision(0),
        }
    );

    let mut backwards = registry_with_media_owner();
    assert_eq!(
        backwards
            .apply(
                &call_id("target"),
                CallCommand::Accept {
                    revision: CallRevision(1),
                },
                at(0),
            )
            .unwrap_err(),
        CallTransitionError::NonMonotonicTimestamp {
            previous: at(1),
            supplied: at(0),
        }
    );
}

#[test]
fn media_owner_check_does_not_mask_invalid_or_terminal_target_behavior() {
    let mut invalid = CallRegistry::new(8);
    invalid.observe_invite(invite("target"), at(0)).unwrap();
    invalid
        .apply(
            &call_id("target"),
            CallCommand::Reject {
                revision: CallRevision(1),
            },
            at(1),
        )
        .unwrap();
    invalid.observe_invite(invite("owner"), at(2)).unwrap();
    invalid
        .apply(
            &call_id("owner"),
            CallCommand::Accept {
                revision: CallRevision(1),
            },
            at(3),
        )
        .unwrap();
    assert_eq!(
        invalid
            .apply(
                &call_id("target"),
                CallCommand::Accept {
                    revision: CallRevision(2),
                },
                at(4),
            )
            .unwrap_err(),
        CallTransitionError::InvalidTransition(CallPhase::Ending)
    );

    let mut terminal = CallRegistry::new(8);
    terminal.observe_invite(invite("ended"), at(0)).unwrap();
    terminal
        .apply(&call_id("ended"), CallCommand::Timeout, at(1))
        .unwrap();
    terminal.observe_invite(invite("owner"), at(2)).unwrap();
    terminal
        .apply(
            &call_id("owner"),
            CallCommand::Accept {
                revision: CallRevision(1),
            },
            at(3),
        )
        .unwrap();
    let repeat = terminal
        .apply(
            &call_id("ended"),
            CallCommand::Accept {
                revision: CallRevision(1),
            },
            at(4),
        )
        .unwrap();
    assert!(!repeat.changed);
    assert_eq!(
        repeat.state.phase(),
        &CallPhase::Ended(CallEndReason::Missed)
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
            .apply(&call_id(id), CallCommand::Timeout, at(index as i64 * 2 + 1))
            .unwrap();
    }

    let ids: Vec<_> = calls
        .history()
        .map(|session| session.id().clone())
        .collect();
    assert_eq!(ids, vec![call_id("middle"), call_id("newest")]);

    let repeat = calls
        .apply(
            &call_id("newest"),
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
fn zero_history_configuration_still_retains_terminal_idempotency() {
    let mut calls = CallRegistry::new(0);
    calls.observe_invite(invite("retained"), at(0)).unwrap();
    calls
        .apply(&call_id("retained"), CallCommand::Timeout, at(1))
        .unwrap();

    let repeat = calls
        .apply(
            &call_id("retained"),
            CallCommand::Reject {
                revision: CallRevision(1),
            },
            at(2),
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
    let public_id = CallId::from_uuid(
        Uuid::parse_str("01980000-0000-7000-8000-000000000001").expect("fixed public call UUID"),
    );
    let session = CallSession::ringing(
        invite_with_id(public_id.clone(), "provider-call-id-secret-700"),
        at(0),
    );
    let observed = reduce(
        &session,
        CallCommand::ObserveInvite(invite_with_id(public_id, "provider-call-id-secret-700")),
        at(1),
    )
    .unwrap();
    assert_eq!(observed.effect, CallEffect::None);

    let json = serde_json::to_string(&observed.event).unwrap();
    for forbidden in [
        "provider-call-id-secret-700",
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
        r#"{"type":"call_updated","call":{"id":"01980000-0000-7000-8000-000000000001","revision":1,"phase":"ringing","media":"audio_video","observed_at":"2027-01-15T08:00:00Z","updated_at":"2027-01-15T08:00:00Z"}}"#
    );
    assert!(CallId::parse("provider-call-id-secret-700").is_err());
    assert!(serde_json::from_str::<CallId>(r#""provider-call-id-secret-700""#).is_err());
}

#[test]
fn revision_exhaustion_is_a_typed_error_instead_of_wrapping() {
    assert_eq!(
        CallRevision(u64::MAX - 1).checked_next().unwrap(),
        CallRevision(u64::MAX)
    );
    assert_eq!(
        CallRevision(u64::MAX).checked_next().unwrap_err(),
        CallTransitionError::RevisionExhausted
    );
}
