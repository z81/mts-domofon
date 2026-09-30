use std::fmt;

use uuid::Uuid;

use super::message::via_branch;
use super::registrar::CALLIBRY_USER_AGENT;
use super::{SipError, SipMethod, SipRequest, SipResponse};

#[derive(Clone, PartialEq, Eq)]
pub struct SipDialogId {
    call_id: String,
    local_tag: String,
    remote_tag: String,
}

impl SipDialogId {
    pub fn call_id(&self) -> &str {
        &self.call_id
    }

    pub fn local_tag(&self) -> &str {
        &self.local_tag
    }

    pub fn remote_tag(&self) -> &str {
        &self.remote_tag
    }
}

impl fmt::Debug for SipDialogId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SipDialogId([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogState {
    Ringing,
    Answered,
    Established,
    Ending,
    Terminated,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DialogCancellation {
    pub cancel_ok: SipResponse,
    pub invite_terminated: SipResponse,
}

pub struct SipDialog {
    id: SipDialogId,
    state: DialogState,
    invite: SipRequest,
    invite_cseq: u32,
    invite_branch: String,
    invite_to: String,
    invite_from: String,
    remote_target: String,
    local_contact: String,
    local_sent_by: String,
    route_set: Vec<String>,
    offer_sdp: Vec<u8>,
    answer_response: Option<SipResponse>,
    next_local_cseq: u32,
}

impl SipDialog {
    pub fn from_invite(
        invite: &SipRequest,
        local_contact: &str,
        local_sent_by: &str,
    ) -> Result<Self, DialogError> {
        if invite.method() != &SipMethod::Invite
            || invite.body().is_empty()
            || !invite
                .headers()
                .get("Content-Type")
                .is_some_and(is_sdp_content_type)
        {
            return Err(DialogError::InvalidInvite);
        }
        validate_header_value(local_contact)?;
        validate_sent_by(local_sent_by)?;

        let call_id = required(invite, "Call-ID")?.to_owned();
        let invite_from = required(invite, "From")?.to_owned();
        let invite_to = required(invite, "To")?.to_owned();
        let remote_tag = header_tag(&invite_from).ok_or(DialogError::MissingRemoteTag)?;
        if header_tag(&invite_to).is_some() {
            return Err(DialogError::InvalidInvite);
        }
        let (invite_cseq, cseq_method) = parse_cseq(required(invite, "CSeq")?)?;
        if cseq_method != "INVITE" {
            return Err(DialogError::InvalidInvite);
        }
        let invite_branch = invite
            .headers()
            .get("Via")
            .and_then(via_branch)
            .ok_or(DialogError::MissingHeader)?
            .to_owned();
        let remote_target = contact_uri(required(invite, "Contact")?)?;
        let route_set = invite
            .headers()
            .get_all("Record-Route")
            .map(split_routes)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .map(str::to_owned)
            .collect();

        Ok(Self {
            id: SipDialogId {
                call_id,
                local_tag: Uuid::new_v4().simple().to_string(),
                remote_tag,
            },
            state: DialogState::Ringing,
            invite: invite.clone(),
            invite_cseq,
            invite_branch,
            invite_to,
            invite_from,
            remote_target,
            local_contact: local_contact.to_owned(),
            local_sent_by: local_sent_by.to_owned(),
            route_set,
            offer_sdp: invite.body().to_vec(),
            answer_response: None,
            next_local_cseq: 1,
        })
    }

    pub fn id(&self) -> &SipDialogId {
        &self.id
    }

    pub fn state(&self) -> DialogState {
        self.state
    }

    pub fn offer_sdp(&self) -> &[u8] {
        &self.offer_sdp
    }

    pub fn ringing(&self) -> Result<SipResponse, DialogError> {
        if self.state != DialogState::Ringing {
            return Err(DialogError::InvalidState);
        }
        let mut response = self.invite_response(180, "Ringing")?;
        response
            .headers_mut()
            .push("Allow", "ACK,INVITE,BYE,OPTIONS,CANCEL")?;
        Ok(response)
    }

    pub fn answer(&mut self, sdp_answer: &[u8]) -> Result<SipResponse, DialogError> {
        if self.state != DialogState::Ringing || sdp_answer.is_empty() {
            return Err(DialogError::InvalidState);
        }
        let mut response = self.invite_response(200, "OK")?;
        response.set_body("application/sdp", sdp_answer)?;
        self.answer_response = Some(response.clone());
        self.state = DialogState::Answered;
        Ok(response)
    }

    pub fn retransmit_answer(&self) -> Result<&SipResponse, DialogError> {
        if self.state != DialogState::Answered {
            return Err(DialogError::InvalidState);
        }
        self.answer_response
            .as_ref()
            .ok_or(DialogError::InvalidState)
    }

    pub fn on_invite_retransmission(
        &self,
        invite: &SipRequest,
    ) -> Result<SipResponse, DialogError> {
        if invite.method() != &SipMethod::Invite
            || !self.matches_initial_transaction(invite, "INVITE")
        {
            return Err(DialogError::SessionMismatch);
        }
        if self.state == DialogState::Ringing {
            return self.ringing();
        }
        self.answer_response
            .clone()
            .ok_or(DialogError::InvalidState)
    }

    pub fn reject(&mut self) -> Result<SipResponse, DialogError> {
        self.finish_ringing(600, "Busy Everywhere")
    }

    pub fn ignore(&mut self) -> Result<SipResponse, DialogError> {
        self.finish_ringing(486, "Busy Here")
    }

    pub fn timeout(&mut self) -> Result<SipResponse, DialogError> {
        self.finish_ringing(480, "Temporarily Unavailable")
    }

    pub fn on_cancel(&mut self, cancel: &SipRequest) -> Result<DialogCancellation, DialogError> {
        if self.state != DialogState::Ringing
            || cancel.method() != &SipMethod::Cancel
            || !self.matches_initial_transaction(cancel, "CANCEL")
        {
            return Err(DialogError::SessionMismatch);
        }
        let cancel_ok = self.response_to(cancel, 200, "OK")?;
        let invite_terminated = self.invite_response(487, "Request Terminated")?;
        self.state = DialogState::Terminated;
        Ok(DialogCancellation {
            cancel_ok,
            invite_terminated,
        })
    }

    pub fn on_ack(&mut self, ack: &SipRequest) -> Result<(), DialogError> {
        if self.state != DialogState::Answered
            || ack.method() != &SipMethod::Ack
            || !self.matches_in_dialog(ack, "ACK", Some(self.invite_cseq))
        {
            return Err(DialogError::SessionMismatch);
        }
        self.state = DialogState::Established;
        Ok(())
    }

    pub fn on_bye(&mut self, bye: &SipRequest) -> Result<SipResponse, DialogError> {
        if self.state != DialogState::Established
            || bye.method() != &SipMethod::Bye
            || !self.matches_in_dialog(bye, "BYE", None)
        {
            return Err(DialogError::SessionMismatch);
        }
        let response = self.response_to(bye, 200, "OK")?;
        self.state = DialogState::Terminated;
        Ok(response)
    }

    pub fn outbound_bye(&mut self) -> Result<SipRequest, DialogError> {
        if self.state != DialogState::Established {
            return Err(DialogError::InvalidState);
        }
        self.build_bye()
    }

    pub fn answer_ack_timeout(&mut self) -> Result<SipRequest, DialogError> {
        if self.state != DialogState::Answered {
            return Err(DialogError::InvalidState);
        }
        self.build_bye()
    }

    fn build_bye(&mut self) -> Result<SipRequest, DialogError> {
        let cseq = self.next_local_cseq;
        self.next_local_cseq = self
            .next_local_cseq
            .checked_add(1)
            .ok_or(DialogError::SequenceExhausted)?;
        let (target, routes) = match self.route_set.first() {
            Some(first) if !has_loose_route(first)? => {
                let mut routes = self.route_set[1..].to_vec();
                routes.push(format!("<{}>", self.remote_target));
                (contact_uri(first)?, routes)
            }
            _ => (self.remote_target.clone(), self.route_set.clone()),
        };
        let mut request = SipRequest::new(SipMethod::Bye, target)?;
        for route in routes {
            request.headers_mut().push("Route", &route)?;
        }
        request.headers_mut().push(
            "Via",
            &format!(
                "SIP/2.0/WSS {};branch=z9hG4bK{};rport",
                self.local_sent_by,
                Uuid::new_v4().simple()
            ),
        )?;
        request.headers_mut().push("Max-Forwards", "70")?;
        request
            .headers_mut()
            .push("From", &with_tag(&self.invite_to, &self.id.local_tag)?)?;
        request.headers_mut().push("To", &self.invite_from)?;
        request.headers_mut().push("Call-ID", &self.id.call_id)?;
        request.headers_mut().push("CSeq", &format!("{cseq} BYE"))?;
        request.headers_mut().push("Contact", &self.local_contact)?;
        request
            .headers_mut()
            .push("User-Agent", CALLIBRY_USER_AGENT)?;
        self.state = DialogState::Ending;
        Ok(request)
    }

    fn finish_ringing(
        &mut self,
        status: u16,
        reason: &'static str,
    ) -> Result<SipResponse, DialogError> {
        if self.state != DialogState::Ringing {
            return Err(DialogError::InvalidState);
        }
        let response = self.invite_response(status, reason)?;
        self.state = DialogState::Terminated;
        Ok(response)
    }

    fn invite_response(&self, status: u16, reason: &str) -> Result<SipResponse, DialogError> {
        self.response_to(&self.invite, status, reason)
    }

    fn response_to(
        &self,
        request: &SipRequest,
        status: u16,
        reason: &str,
    ) -> Result<SipResponse, DialogError> {
        let mut response = SipResponse::new(status, reason)?;
        for via in request.headers().get_all("Via") {
            response.headers_mut().push("Via", via)?;
        }
        response
            .headers_mut()
            .push("From", required(request, "From")?)?;
        response.headers_mut().push(
            "To",
            &with_tag(required(request, "To")?, &self.id.local_tag)?,
        )?;
        response
            .headers_mut()
            .push("Call-ID", required(request, "Call-ID")?)?;
        response
            .headers_mut()
            .push("CSeq", required(request, "CSeq")?)?;
        if request.method() == &SipMethod::Invite && (101..300).contains(&status) {
            for route in request.headers().get_all("Record-Route") {
                response.headers_mut().push("Record-Route", route)?;
            }
        }
        response
            .headers_mut()
            .push("Contact", &self.local_contact)?;
        response
            .headers_mut()
            .push("User-Agent", CALLIBRY_USER_AGENT)?;
        Ok(response)
    }

    fn matches_initial_transaction(&self, request: &SipRequest, method: &str) -> bool {
        request.headers().get("Call-ID") == Some(self.id.call_id.as_str())
            && request.headers().get("From") == Some(self.invite_from.as_str())
            && request.headers().get("To") == Some(self.invite_to.as_str())
            && request.headers().get("Via").and_then(via_branch)
                == Some(self.invite_branch.as_str())
            && parse_cseq(request.headers().get("CSeq").unwrap_or_default()).is_ok_and(
                |(number, parsed_method)| number == self.invite_cseq && parsed_method == method,
            )
    }

    fn matches_in_dialog(
        &self,
        request: &SipRequest,
        method: &str,
        expected_cseq: Option<u32>,
    ) -> bool {
        request.headers().get("Call-ID") == Some(self.id.call_id.as_str())
            && header_tag(request.headers().get("From").unwrap_or_default()).as_deref()
                == Some(self.id.remote_tag.as_str())
            && header_tag(request.headers().get("To").unwrap_or_default()).as_deref()
                == Some(self.id.local_tag.as_str())
            && parse_cseq(request.headers().get("CSeq").unwrap_or_default()).is_ok_and(
                |(number, parsed_method)| {
                    parsed_method == method
                        && expected_cseq.is_none_or(|expected| expected == number)
                },
            )
    }
}

impl fmt::Debug for SipDialog {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SipDialog")
            .field("id", &self.id)
            .field("state", &self.state)
            .field("offer_bytes", &self.offer_sdp.len())
            .finish()
    }
}

fn required<'a>(request: &'a SipRequest, name: &str) -> Result<&'a str, DialogError> {
    request
        .headers()
        .get(name)
        .filter(|value| !value.is_empty())
        .ok_or(DialogError::MissingHeader)
}

fn parse_cseq(value: &str) -> Result<(u32, &str), DialogError> {
    let mut parts = value.split_ascii_whitespace();
    let number = parts
        .next()
        .ok_or(DialogError::InvalidCseq)?
        .parse::<u32>()
        .map_err(|_| DialogError::InvalidCseq)?;
    let method = parts.next().ok_or(DialogError::InvalidCseq)?;
    if parts.next().is_some() {
        return Err(DialogError::InvalidCseq);
    }
    Ok((number, method))
}

fn header_tag(value: &str) -> Option<String> {
    value.split(';').skip(1).find_map(|parameter| {
        let (name, value) = parameter.trim().split_once('=')?;
        name.eq_ignore_ascii_case("tag")
            .then(|| value.trim().to_owned())
    })
}

fn with_tag(value: &str, tag: &str) -> Result<String, DialogError> {
    validate_header_value(value)?;
    if header_tag(value).is_some() {
        Ok(value.to_owned())
    } else {
        Ok(format!("{value};tag={tag}"))
    }
}

fn contact_uri(value: &str) -> Result<String, DialogError> {
    validate_header_value(value)?;
    let value = value.trim();
    let uri = if let Some((_, inner)) = value.split_once('<') {
        inner.split_once('>').map(|(uri, _)| uri)
    } else {
        Some(value)
    }
    .ok_or(DialogError::InvalidContact)?;
    if !(uri.starts_with("sip:") || uri.starts_with("sips:"))
        || uri.bytes().any(|byte| byte.is_ascii_whitespace())
    {
        return Err(DialogError::InvalidContact);
    }
    Ok(uri.to_owned())
}

fn has_loose_route(route: &str) -> Result<bool, DialogError> {
    Ok(contact_uri(route)?.split(';').skip(1).any(|parameter| {
        parameter
            .split_once('=')
            .map_or(parameter, |(name, _)| name)
            .eq_ignore_ascii_case("lr")
    }))
}

fn split_routes(value: &str) -> Result<Vec<&str>, DialogError> {
    let mut routes = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    let mut bracketed = false;
    for (index, byte) in value.bytes().enumerate() {
        if escaped {
            escaped = false;
        } else if quoted && byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            quoted = !quoted;
        } else if !quoted {
            match byte {
                b'<' => bracketed = true,
                b'>' => bracketed = false,
                b',' if !bracketed => {
                    let route = value[start..index].trim();
                    contact_uri(route)?;
                    routes.push(route);
                    start = index + 1;
                }
                _ => {}
            }
        }
    }
    if quoted || escaped || bracketed {
        return Err(DialogError::InvalidContact);
    }
    let route = value[start..].trim();
    contact_uri(route)?;
    routes.push(route);
    Ok(routes)
}

fn is_sdp_content_type(value: &str) -> bool {
    value
        .split(';')
        .next()
        .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("application/sdp"))
}

fn validate_header_value(value: &str) -> Result<(), DialogError> {
    if value.is_empty() || value.contains(['\r', '\n']) {
        Err(DialogError::InvalidHeader)
    } else {
        Ok(())
    }
}

fn validate_sent_by(value: &str) -> Result<(), DialogError> {
    validate_header_value(value)?;
    if value
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
    {
        Err(DialogError::InvalidHeader)
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DialogError {
    #[error("SIP INVITE is invalid")]
    InvalidInvite,
    #[error("required SIP dialog header is missing")]
    MissingHeader,
    #[error("SIP INVITE remote tag is missing")]
    MissingRemoteTag,
    #[error("SIP CSeq is invalid")]
    InvalidCseq,
    #[error("SIP Contact is invalid")]
    InvalidContact,
    #[error("SIP dialog header is invalid")]
    InvalidHeader,
    #[error("SIP dialog is in an invalid state")]
    InvalidState,
    #[error("SIP request does not correlate to this dialog")]
    SessionMismatch,
    #[error("SIP dialog sequence is exhausted")]
    SequenceExhausted,
    #[error(transparent)]
    Message(#[from] SipError),
}
