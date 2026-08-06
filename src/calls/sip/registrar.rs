use std::fmt;

use secrecy::{ExposeSecret, SecretString};
use uuid::Uuid;

use super::message::via_branch;
use super::{
    DigestChallenge, DigestError, DigestRequest, SipError, SipMethod, SipRequest, SipResponse,
    digest_authorization,
};

pub const CALLIBRY_USER_AGENT: &str = "mts-ndv-ua/sip/android/1.0";
pub const DEFAULT_REGISTER_EXPIRES: u32 = 600;

#[derive(Clone)]
pub struct RegisterConfig {
    registrar: String,
    username: SecretString,
    password: SecretString,
    contact_host: String,
}

impl RegisterConfig {
    pub fn new(
        registrar: impl Into<String>,
        username: SecretString,
        password: SecretString,
        contact_host: impl Into<String>,
    ) -> Result<Self, RegistrarError> {
        let registrar = registrar.into();
        let contact_host = contact_host.into();
        if !safe_address_part(&registrar)
            || !safe_address_part(&contact_host)
            || !safe_user(username.expose_secret())
            || password
                .expose_secret()
                .bytes()
                .any(|byte| byte.is_ascii_control())
        {
            return Err(RegistrarError::InvalidConfig);
        }
        Ok(Self {
            registrar,
            username,
            password,
            contact_host,
        })
    }
}

impl fmt::Debug for RegisterConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RegisterConfig")
            .field("registrar", &"[REDACTED]")
            .field("username", &"[REDACTED]")
            .field("password", &"[REDACTED]")
            .field("contact_host", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RegistrarState {
    #[default]
    Idle,
    Registering,
    Authenticating,
    Registered,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistrarAction {
    Send(SipRequest),
    Registered { expires: u32 },
    Failed { status: u16 },
    Ignored,
}

#[derive(Clone)]
struct PendingTransaction {
    call_id: String,
    branch: String,
    cseq: String,
}

pub struct SipRegistrar {
    config: RegisterConfig,
    state: RegistrarState,
    call_id: String,
    local_tag: String,
    next_cseq: u32,
    pending: Option<PendingTransaction>,
}

impl SipRegistrar {
    pub fn new(config: RegisterConfig) -> Self {
        let call_id = format!("{}@{}", Uuid::new_v4().simple(), config.contact_host);
        Self {
            config,
            state: RegistrarState::Idle,
            call_id,
            local_tag: Uuid::new_v4().simple().to_string(),
            next_cseq: 1,
            pending: None,
        }
    }

    pub fn state(&self) -> RegistrarState {
        self.state
    }

    pub fn begin(&mut self) -> Result<SipRequest, RegistrarError> {
        if self.state != RegistrarState::Idle {
            return Err(RegistrarError::InvalidState);
        }
        let request = self.build_register(None)?;
        self.state = RegistrarState::Registering;
        Ok(request)
    }

    pub fn on_response(
        &mut self,
        response: &SipResponse,
    ) -> Result<RegistrarAction, RegistrarError> {
        if !self.is_correlated(response) {
            return Ok(RegistrarAction::Ignored);
        }
        match response.status() {
            200 => {
                self.pending = None;
                self.state = RegistrarState::Registered;
                Ok(RegistrarAction::Registered {
                    expires: DEFAULT_REGISTER_EXPIRES,
                })
            }
            401 if self.state == RegistrarState::Registering => {
                let challenge = response
                    .headers()
                    .get("WWW-Authenticate")
                    .ok_or(RegistrarError::MissingChallenge)
                    .and_then(|value| {
                        DigestChallenge::parse(value).map_err(RegistrarError::Digest)
                    })?;
                let uri = format!("sip:{}", self.config.registrar);
                let digest_request = DigestRequest::new("REGISTER", &uri)?;
                let authorization = digest_authorization(
                    &challenge,
                    &digest_request,
                    &self.config.username,
                    &self.config.password,
                )?;
                let request = self.build_register(Some(authorization.expose()))?;
                self.state = RegistrarState::Authenticating;
                Ok(RegistrarAction::Send(request))
            }
            401 => {
                self.pending = None;
                self.state = RegistrarState::Failed;
                Err(RegistrarError::AuthenticationRejected)
            }
            status => {
                self.pending = None;
                self.state = RegistrarState::Failed;
                Ok(RegistrarAction::Failed { status })
            }
        }
    }

    fn build_register(
        &mut self,
        authorization: Option<&str>,
    ) -> Result<SipRequest, RegistrarError> {
        let uri = format!("sip:{}", self.config.registrar);
        let mut request = SipRequest::new(SipMethod::Register, uri)?;
        let branch = format!("z9hG4bK{}", Uuid::new_v4().simple());
        let via = format!(
            "SIP/2.0/WSS {};branch={branch};rport",
            self.config.contact_host
        );
        let username = self.config.username.expose_secret();
        let address = format!("<sip:{username}@{}>", self.config.registrar);
        let contact = format!(
            "<sip:{username}@{};transport=wss>",
            self.config.contact_host
        );
        let cseq = self.next_cseq;
        self.next_cseq = self
            .next_cseq
            .checked_add(1)
            .ok_or(RegistrarError::SequenceExhausted)?;

        request.headers_mut().push("Via", &via)?;
        request.headers_mut().push("Max-Forwards", "70")?;
        request
            .headers_mut()
            .push("From", &format!("{address};tag={}", self.local_tag))?;
        request.headers_mut().push("To", &address)?;
        request.headers_mut().push("Call-ID", &self.call_id)?;
        request
            .headers_mut()
            .push("CSeq", &format!("{cseq} REGISTER"))?;
        request.headers_mut().push("Contact", &contact)?;
        request
            .headers_mut()
            .push("Expires", &DEFAULT_REGISTER_EXPIRES.to_string())?;
        request
            .headers_mut()
            .push("User-Agent", CALLIBRY_USER_AGENT)?;
        if let Some(authorization) = authorization {
            request.headers_mut().push("Authorization", authorization)?;
        }
        self.pending = Some(PendingTransaction {
            call_id: self.call_id.clone(),
            branch,
            cseq: format!("{cseq} REGISTER"),
        });
        Ok(request)
    }

    fn is_correlated(&self, response: &SipResponse) -> bool {
        self.pending.as_ref().is_some_and(|pending| {
            response.headers().get("Call-ID") == Some(pending.call_id.as_str())
                && response.headers().get("Via").and_then(via_branch)
                    == Some(pending.branch.as_str())
                && response.headers().get("CSeq") == Some(pending.cseq.as_str())
        })
    }
}

impl fmt::Debug for SipRegistrar {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SipRegistrar")
            .field("state", &self.state)
            .field("has_pending_transaction", &self.pending.is_some())
            .finish()
    }
}

fn safe_address_part(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1024
        && !value
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
        && !value.contains(['<', '>', '"', '\\'])
}

fn safe_user(value: &str) -> bool {
    safe_address_part(value) && !value.contains(['@', ':', ';'])
}

#[derive(Debug, thiserror::Error)]
pub enum RegistrarError {
    #[error("SIP registration configuration is invalid")]
    InvalidConfig,
    #[error("SIP registrar is in an invalid state")]
    InvalidState,
    #[error("SIP REGISTER sequence is exhausted")]
    SequenceExhausted,
    #[error("SIP authentication challenge is missing")]
    MissingChallenge,
    #[error("SIP registration authentication was rejected")]
    AuthenticationRejected,
    #[error(transparent)]
    Digest(#[from] DigestError),
    #[error(transparent)]
    Message(#[from] SipError),
}
