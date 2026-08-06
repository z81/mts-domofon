use domru_mts::calls::sip::{
    DialogState, DigestChallenge, DigestRequest, RegisterConfig, RegistrarAction, RegistrarState,
    SipDialog, SipLimits, SipMessage, SipMethod, SipRegistrar, SipRequest, SipResponse,
    digest_authorization, parse_frame,
};
use secrecy::SecretString;

const SDP: &str = "v=0\r\nm=audio 40000 RTP/AVP 111\r\na=rtpmap:111 opus/48000/2\r\n";

fn invite_frame() -> Vec<u8> {
    format!(
        concat!(
            "INVITE sip:door@example.test SIP/2.0\r\n",
            "v: SIP/2.0/WSS client.invalid;branch=z9hG4bK-invite;rport\r\n",
            "f: <sip:door@example.test>;tag=remote-tag\r\n",
            "T: <sip:resident@example.test>\r\n",
            "i: call-123\r\n",
            "CSeq: 42 INVITE\r\n",
            "Content-Type: application/sdp\r\n",
            "content-length: {}\r\n",
            "\r\n",
            "{}"
        ),
        SDP.len(),
        SDP
    )
    .into_bytes()
}

#[test]
fn parses_requests_with_case_insensitive_and_compact_headers() {
    let parsed = parse_frame(&invite_frame(), SipLimits::default()).expect("valid INVITE");
    let SipMessage::Request(request) = parsed else {
        panic!("expected request");
    };

    assert_eq!(request.method(), &SipMethod::Invite);
    assert_eq!(request.uri(), "sip:door@example.test");
    assert_eq!(
        request.headers().get("Via"),
        Some("SIP/2.0/WSS client.invalid;branch=z9hG4bK-invite;rport")
    );
    assert_eq!(
        request.headers().get("from"),
        Some("<sip:door@example.test>;tag=remote-tag")
    );
    assert_eq!(request.headers().get("CALL-ID"), Some("call-123"));
    assert_eq!(request.body(), SDP.as_bytes());
}

#[test]
fn parses_responses_and_preserves_the_reason_phrase() {
    let frame = b"SIP/2.0 401 Unauthorized\r\nCSeq: 7 REGISTER\r\nContent-Length: 0\r\n\r\n";
    let parsed = parse_frame(frame, SipLimits::default()).expect("valid response");
    let SipMessage::Response(response) = parsed else {
        panic!("expected response");
    };

    assert_eq!(response.status(), 401);
    assert_eq!(response.reason(), "Unauthorized");
    assert_eq!(response.headers().get("cseq"), Some("7 REGISTER"));
}

#[test]
fn serializer_owns_content_length_and_counts_encoded_bytes() {
    let mut response = SipResponse::new(200, "OK").expect("valid response");
    response
        .headers_mut()
        .push("X-Test", "kept")
        .expect("valid header");
    response
        .headers_mut()
        .push("Content-Length", "999")
        .expect("valid header");
    response
        .set_body("application/sdp", "ёж".as_bytes())
        .expect("valid body");

    let wire = response
        .to_bytes(SipLimits::default())
        .expect("bounded wire");
    let wire = String::from_utf8(wire).expect("SIP text");
    assert!(wire.contains("Content-Type: application/sdp\r\n"));
    assert!(wire.contains("Content-Length: 4\r\n\r\nёж"));
    assert_eq!(wire.matches("Content-Length:").count(), 1);
}

#[test]
fn rejects_body_length_mismatch_folded_headers_and_oversized_frames() {
    let mismatch = b"INVITE sip:x@y SIP/2.0\r\nContent-Length: 4\r\n\r\nabc";
    assert!(parse_frame(mismatch, SipLimits::default()).is_err());

    let folded = b"INVITE sip:x@y SIP/2.0\r\nX-Secret: safe\r\n leaked-password\r\nContent-Length: 0\r\n\r\n";
    assert!(parse_frame(folded, SipLimits::default()).is_err());

    let limits = SipLimits {
        max_message_bytes: 32,
        ..SipLimits::default()
    };
    let error = parse_frame(&invite_frame(), limits).expect_err("frame must be bounded");
    let diagnostic = format!("{error:?} {error}");
    assert!(!diagnostic.contains("leaked-password"));
    assert!(!diagnostic.contains("call-123"));
}

#[test]
fn rejects_header_injection_before_serializing() {
    let mut request =
        SipRequest::new(SipMethod::Options, "sip:door@example.test").expect("valid request");
    let error = request
        .headers_mut()
        .push("X-Test", "ok\r\nAuthorization: leaked-password")
        .expect_err("CRLF injection must fail");
    assert!(!format!("{error:?} {error}").contains("leaked-password"));
}

fn test_register_config() -> RegisterConfig {
    RegisterConfig::new(
        "sip.example",
        SecretString::from("alice"),
        SecretString::from("swordfish"),
        "client.invalid",
    )
    .expect("valid config")
}

fn response_to(request: &SipRequest, status: u16, reason: &str) -> SipResponse {
    let mut response = SipResponse::new(status, reason).expect("valid response");
    for name in ["Via", "From", "To", "Call-ID", "CSeq"] {
        response
            .headers_mut()
            .push(name, request.headers().get(name).expect("request header"))
            .expect("valid copied header");
    }
    response
}

#[test]
fn register_uses_the_recovered_wss_contract() {
    let mut registrar = SipRegistrar::new(test_register_config());
    let request = registrar.begin().expect("initial REGISTER");

    assert_eq!(request.method(), &SipMethod::Register);
    assert_eq!(request.uri(), "sip:sip.example");
    assert_eq!(request.headers().get("Max-Forwards"), Some("70"));
    assert_eq!(request.headers().get("Expires"), Some("600"));
    assert_eq!(
        request.headers().get("User-Agent"),
        Some("mts-ndv-ua/sip/android/1.0")
    );
    let via = request.headers().get("Via").expect("Via");
    assert!(via.starts_with("SIP/2.0/WSS client.invalid;branch=z9hG4bK"));
    assert!(via.ends_with(";rport"));
    assert_eq!(
        request.headers().get("Contact"),
        Some("<sip:alice@client.invalid;transport=wss>")
    );
    assert_eq!(registrar.state(), RegistrarState::Registering);
}

#[test]
fn apk_digest_is_md5_without_qop_and_diagnostics_are_redacted() {
    let challenge =
        DigestChallenge::parse("Digest realm=\"testrealm\", nonce=\"abcdef\", algorithm=MD5")
            .expect("supported challenge");
    let authorization = digest_authorization(
        &challenge,
        &DigestRequest::new("REGISTER", "sip:sip.example").expect("request"),
        &SecretString::from("alice"),
        &SecretString::from("swordfish"),
    )
    .expect("digest");

    assert_eq!(
        authorization.expose(),
        concat!(
            "Digest algorithm=MD5,username=\"alice\",realm=\"testrealm\",",
            "nonce=\"abcdef\",uri=\"sip:sip.example\",",
            "response=\"afd4f2c5d1d10d9e440376ba42841e2d\""
        )
    );
    let diagnostics = format!("{challenge:?} {authorization:?}");
    assert!(!diagnostics.contains("alice"));
    assert!(!diagnostics.contains("swordfish"));
    assert!(!diagnostics.contains("abcdef"));

    assert!(
        DigestChallenge::parse(
            "Digest realm=\"testrealm\",nonce=\"abcdef\",algorithm=MD5,qop=\"auth\""
        )
        .is_err()
    );
    assert!(
        DigestChallenge::parse("dIgEsT realm=\"testrealm\",nonce=\"abcdef\",algorithm=md5").is_ok()
    );
}

#[test]
fn correlated_401_produces_one_authenticated_register_retry() {
    let mut registrar = SipRegistrar::new(test_register_config());
    let first = registrar.begin().expect("initial REGISTER");
    let first_cseq = first.headers().get("CSeq").expect("CSeq").to_owned();
    let first_via = first.headers().get("Via").expect("Via").to_owned();
    let mut challenge = response_to(&first, 401, "Unauthorized");
    challenge
        .headers_mut()
        .set(
            "Via",
            &format!("{first_via};received=203.0.113.8;rport=52341"),
        )
        .expect("server-updated Via");
    challenge
        .headers_mut()
        .push(
            "wWw-AuThEnTiCaTe",
            "Digest realm=\"testrealm\", nonce=\"abcdef\", algorithm=MD5",
        )
        .expect("challenge");

    let RegistrarAction::Send(authenticated) = registrar
        .on_response(&challenge)
        .expect("challenge accepted")
    else {
        panic!("expected authenticated retry");
    };
    assert_ne!(
        authenticated.headers().get("CSeq"),
        Some(first_cseq.as_str())
    );
    assert_ne!(authenticated.headers().get("Via"), Some(first_via.as_str()));
    assert!(
        authenticated
            .headers()
            .get("Authorization")
            .expect("digest header")
            .contains("response=\"afd4f2c5d1d10d9e440376ba42841e2d\"")
    );
    assert_eq!(registrar.state(), RegistrarState::Authenticating);

    let mut ok = response_to(&authenticated, 200, "OK");
    let authenticated_via = authenticated.headers().get("Via").expect("Via");
    ok.headers_mut()
        .set(
            "Via",
            &format!("{authenticated_via};received=203.0.113.8;rport=52341"),
        )
        .expect("server-updated Via");
    assert_eq!(
        registrar.on_response(&ok).expect("registered"),
        RegistrarAction::Registered { expires: 600 }
    );
    assert_eq!(registrar.state(), RegistrarState::Registered);
}

#[test]
fn registrar_ignores_uncorrelated_responses_and_rejects_a_second_challenge() {
    let mut registrar = SipRegistrar::new(test_register_config());
    let first = registrar.begin().expect("initial REGISTER");
    let mut unrelated = response_to(&first, 401, "Unauthorized");
    unrelated
        .headers_mut()
        .set("Call-ID", "different-call")
        .expect("header");
    assert_eq!(
        registrar.on_response(&unrelated).expect("ignored"),
        RegistrarAction::Ignored
    );

    let mut challenge = response_to(&first, 401, "Unauthorized");
    challenge
        .headers_mut()
        .push(
            "WWW-Authenticate",
            "Digest realm=\"testrealm\",nonce=\"abcdef\",algorithm=MD5",
        )
        .expect("challenge");
    let RegistrarAction::Send(authenticated) =
        registrar.on_response(&challenge).expect("first challenge")
    else {
        panic!("expected retry");
    };
    let second = response_to(&authenticated, 401, "Unauthorized");
    assert!(registrar.on_response(&second).is_err());

    let diagnostic = format!("{registrar:?}");
    assert!(!diagnostic.contains("alice"));
    assert!(!diagnostic.contains("swordfish"));
}

fn dialog_request(method: SipMethod, cseq_method: &str, to: &str) -> SipRequest {
    let mut request = SipRequest::new(method, "sip:resident@client.invalid").expect("request");
    for (name, value) in [
        (
            "Via",
            "SIP/2.0/WSS server.example;branch=z9hG4bK-dialog;rport",
        ),
        ("From", "<sip:door@sip.example>;tag=remote-tag"),
        ("To", to),
        ("Call-ID", "dialog-call-123"),
        ("CSeq", &format!("42 {cseq_method}")),
        ("Contact", "<sip:door@server.example;transport=wss>"),
    ] {
        request
            .headers_mut()
            .push(name, value)
            .expect("valid header");
    }
    request
}

fn incoming_dialog() -> SipDialog {
    let mut invite = dialog_request(SipMethod::Invite, "INVITE", "<sip:resident@sip.example>");
    invite
        .set_body("application/sdp", SDP.as_bytes())
        .expect("SDP body");
    SipDialog::from_invite(
        &invite,
        "<sip:alice@client.invalid;transport=wss>",
        "client.invalid",
    )
    .expect("valid incoming dialog")
}

#[test]
fn incoming_invite_retains_bounded_sdp_and_builds_correlated_ringing() {
    let dialog = incoming_dialog();
    assert_eq!(dialog.offer_sdp(), SDP.as_bytes());
    assert_eq!(dialog.id().call_id(), "dialog-call-123");
    assert_eq!(dialog.id().remote_tag(), "remote-tag");
    assert!(!dialog.id().local_tag().is_empty());
    assert_eq!(dialog.state(), DialogState::Ringing);

    let ringing = dialog.ringing().expect("180 response");
    assert_eq!(ringing.status(), 180);
    assert_eq!(
        ringing.headers().get("Allow"),
        Some("ACK,INVITE,BYE,OPTIONS,CANCEL")
    );
    assert_eq!(
        ringing.headers().get("Via"),
        Some("SIP/2.0/WSS server.example;branch=z9hG4bK-dialog;rport")
    );
    assert_eq!(
        ringing.headers().get("From"),
        Some("<sip:door@sip.example>;tag=remote-tag")
    );
    assert_eq!(ringing.headers().get("Call-ID"), Some("dialog-call-123"));
    assert_eq!(ringing.headers().get("CSeq"), Some("42 INVITE"));
    assert!(
        ringing
            .headers()
            .get("To")
            .expect("To")
            .contains(&format!(";tag={}", dialog.id().local_tag()))
    );
}

#[test]
fn dialog_builds_answer_and_each_local_ringing_disposition() {
    let mut accepted = incoming_dialog();
    let answer = accepted.answer(SDP.as_bytes()).expect("200 answer");
    assert_eq!(answer.status(), 200);
    assert_eq!(answer.headers().get("Allow"), None);
    assert_eq!(
        answer.headers().get("Content-Type"),
        Some("application/sdp")
    );
    assert_eq!(
        answer.headers().get("Contact"),
        Some("<sip:alice@client.invalid;transport=wss>")
    );
    assert_eq!(answer.body(), SDP.as_bytes());
    assert_eq!(accepted.state(), DialogState::Answered);

    for (dialog, response, expected) in [
        {
            let mut dialog = incoming_dialog();
            let response = dialog.reject().expect("reject");
            (dialog, response, (600, "Busy Everywhere"))
        },
        {
            let mut dialog = incoming_dialog();
            let response = dialog.ignore().expect("ignore");
            (dialog, response, (486, "Busy Here"))
        },
        {
            let mut dialog = incoming_dialog();
            let response = dialog.timeout().expect("timeout");
            (dialog, response, (480, "Temporarily Unavailable"))
        },
    ] {
        assert_eq!((response.status(), response.reason()), expected);
        assert_eq!(dialog.state(), DialogState::Terminated);
    }
}

#[test]
fn matching_cancel_returns_cancel_200_and_invite_487() {
    let mut dialog = incoming_dialog();
    let cancel = dialog_request(SipMethod::Cancel, "CANCEL", "<sip:resident@sip.example>");
    let cancelled = dialog.on_cancel(&cancel).expect("matching CANCEL");

    assert_eq!(cancelled.cancel_ok.status(), 200);
    assert_eq!(cancelled.cancel_ok.headers().get("CSeq"), Some("42 CANCEL"));
    assert_eq!(cancelled.invite_terminated.status(), 487);
    assert_eq!(
        cancelled.invite_terminated.headers().get("CSeq"),
        Some("42 INVITE")
    );
    assert_eq!(dialog.state(), DialogState::Terminated);
}

#[test]
fn cancel_requires_the_original_invite_top_via_branch() {
    let mut dialog = incoming_dialog();
    let mut cancel = dialog_request(SipMethod::Cancel, "CANCEL", "<sip:resident@sip.example>");
    cancel
        .headers_mut()
        .set(
            "Via",
            "SIP/2.0/WSS server.example;branch=z9hG4bK-other;rport",
        )
        .expect("Via");

    assert!(dialog.on_cancel(&cancel).is_err());
    assert_eq!(dialog.state(), DialogState::Ringing);
}

#[test]
fn ack_and_bye_are_correlated_to_the_answered_dialog() {
    let mut dialog = incoming_dialog();
    dialog.answer(SDP.as_bytes()).expect("answer");
    let to = format!("<sip:resident@sip.example>;tag={}", dialog.id().local_tag());
    let ack = dialog_request(SipMethod::Ack, "ACK", &to);
    dialog.on_ack(&ack).expect("matching ACK");
    assert_eq!(dialog.state(), DialogState::Established);

    let bye = dialog_request(SipMethod::Bye, "BYE", &to);
    let response = dialog.on_bye(&bye).expect("matching BYE");
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers().get("CSeq"), Some("42 BYE"));
    assert_eq!(dialog.state(), DialogState::Terminated);
}

#[test]
fn outbound_bye_uses_remote_target_and_dialog_tags() {
    let mut dialog = incoming_dialog();
    dialog.answer(SDP.as_bytes()).expect("answer");
    let to = format!("<sip:resident@sip.example>;tag={}", dialog.id().local_tag());
    let ack = dialog_request(SipMethod::Ack, "ACK", &to);
    dialog.on_ack(&ack).expect("ACK");

    let bye = dialog.outbound_bye().expect("BYE");
    assert_eq!(bye.method(), &SipMethod::Bye);
    assert_eq!(bye.uri(), "sip:door@server.example;transport=wss");
    assert_eq!(bye.headers().get("Call-ID"), Some("dialog-call-123"));
    assert!(
        bye.headers()
            .get("From")
            .expect("From")
            .contains(dialog.id().local_tag())
    );
    assert!(bye.headers().get("To").expect("To").contains("remote-tag"));
    assert_eq!(bye.headers().get("CSeq"), Some("1 BYE"));
    assert_eq!(dialog.state(), DialogState::Ending);
}

#[test]
fn dialog_rejects_wrong_session_and_non_sdp_invites() {
    let mut dialog = incoming_dialog();
    let mut wrong = dialog_request(SipMethod::Cancel, "CANCEL", "<sip:resident@sip.example>");
    wrong
        .headers_mut()
        .set("Call-ID", "another-call")
        .expect("header");
    assert!(dialog.on_cancel(&wrong).is_err());

    let wrong_to = dialog_request(
        SipMethod::Cancel,
        "CANCEL",
        "<sip:someone-else@sip.example>",
    );
    assert!(dialog.on_cancel(&wrong_to).is_err());

    let invite = dialog_request(SipMethod::Invite, "INVITE", "<sip:resident@sip.example>");
    assert!(
        SipDialog::from_invite(
            &invite,
            "<sip:alice@client.invalid;transport=wss>",
            "client.invalid"
        )
        .is_err()
    );
}
