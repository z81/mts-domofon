use std::fmt;

const SIP_VERSION: &str = "SIP/2.0";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SipLimits {
    pub max_message_bytes: usize,
    pub max_header_bytes: usize,
    pub max_header_count: usize,
    pub max_line_bytes: usize,
    pub max_body_bytes: usize,
}

impl Default for SipLimits {
    fn default() -> Self {
        Self {
            max_message_bytes: 64 * 1024,
            max_header_bytes: 16 * 1024,
            max_header_count: 96,
            max_line_bytes: 4 * 1024,
            max_body_bytes: 48 * 1024,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum SipMethod {
    Register,
    Invite,
    Ack,
    Cancel,
    Bye,
    Options,
    Other(String),
}

impl SipMethod {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Register => "REGISTER",
            Self::Invite => "INVITE",
            Self::Ack => "ACK",
            Self::Cancel => "CANCEL",
            Self::Bye => "BYE",
            Self::Options => "OPTIONS",
            Self::Other(method) => method,
        }
    }

    fn parse(value: &str) -> Result<Self, SipError> {
        if !is_token(value) {
            return Err(SipError::InvalidStartLine);
        }
        Ok(match value {
            "REGISTER" => Self::Register,
            "INVITE" => Self::Invite,
            "ACK" => Self::Ack,
            "CANCEL" => Self::Cancel,
            "BYE" => Self::Bye,
            "OPTIONS" => Self::Options,
            other => Self::Other(other.to_owned()),
        })
    }
}

impl fmt::Debug for SipMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SipHeader {
    name: String,
    value: String,
}

impl SipHeader {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn value(&self) -> &str {
        &self.value
    }
}

impl fmt::Debug for SipHeader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SipHeader")
            .field("name", &self.name)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct SipHeaders(Vec<SipHeader>);

impl SipHeaders {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|header| header_name_eq(header.name(), name))
            .map(SipHeader::value)
    }

    pub fn get_all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.0
            .iter()
            .filter(move |header| header_name_eq(header.name(), name))
            .map(SipHeader::value)
    }

    pub fn push(&mut self, name: &str, value: &str) -> Result<(), SipError> {
        if !is_token(name) || contains_line_break(value) {
            return Err(SipError::InvalidHeader);
        }
        self.0.push(SipHeader {
            name: name.to_owned(),
            value: value.trim().to_owned(),
        });
        Ok(())
    }

    pub fn set(&mut self, name: &str, value: &str) -> Result<(), SipError> {
        self.remove(name);
        self.push(name, value)
    }

    pub fn remove(&mut self, name: &str) {
        self.0.retain(|header| !header_name_eq(header.name(), name));
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &SipHeader> {
        self.0.iter()
    }
}

impl fmt::Debug for SipHeaders {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SipHeaders")
            .field("count", &self.0.len())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SipRequest {
    method: SipMethod,
    uri: String,
    headers: SipHeaders,
    body: Vec<u8>,
}

impl SipRequest {
    pub fn new(method: SipMethod, uri: impl Into<String>) -> Result<Self, SipError> {
        let uri = uri.into();
        if !valid_start_line_part(&uri) {
            return Err(SipError::InvalidStartLine);
        }
        Ok(Self {
            method,
            uri,
            headers: SipHeaders::default(),
            body: Vec::new(),
        })
    }

    pub fn method(&self) -> &SipMethod {
        &self.method
    }

    pub fn uri(&self) -> &str {
        &self.uri
    }

    pub fn headers(&self) -> &SipHeaders {
        &self.headers
    }

    pub fn headers_mut(&mut self) -> &mut SipHeaders {
        &mut self.headers
    }

    pub fn body(&self) -> &[u8] {
        &self.body
    }

    pub fn set_body(&mut self, content_type: &str, body: &[u8]) -> Result<(), SipError> {
        self.headers.set("Content-Type", content_type)?;
        self.body = body.to_vec();
        Ok(())
    }

    pub fn to_bytes(&self, limits: SipLimits) -> Result<Vec<u8>, SipError> {
        serialize(
            &format!("{} {} {SIP_VERSION}", self.method.as_str(), self.uri),
            &self.headers,
            &self.body,
            limits,
        )
    }
}

impl fmt::Debug for SipRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SipRequest")
            .field("method", &self.method)
            .field("header_count", &self.headers.len())
            .field("body_bytes", &self.body.len())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SipResponse {
    status: u16,
    reason: String,
    headers: SipHeaders,
    body: Vec<u8>,
}

impl SipResponse {
    pub fn new(status: u16, reason: impl Into<String>) -> Result<Self, SipError> {
        let reason = reason.into();
        if !(100..=699).contains(&status) || contains_line_break(&reason) {
            return Err(SipError::InvalidStartLine);
        }
        Ok(Self {
            status,
            reason,
            headers: SipHeaders::default(),
            body: Vec::new(),
        })
    }

    pub fn status(&self) -> u16 {
        self.status
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }

    pub fn headers(&self) -> &SipHeaders {
        &self.headers
    }

    pub fn headers_mut(&mut self) -> &mut SipHeaders {
        &mut self.headers
    }

    pub fn body(&self) -> &[u8] {
        &self.body
    }

    pub fn set_body(&mut self, content_type: &str, body: &[u8]) -> Result<(), SipError> {
        self.headers.set("Content-Type", content_type)?;
        self.body = body.to_vec();
        Ok(())
    }

    pub fn to_bytes(&self, limits: SipLimits) -> Result<Vec<u8>, SipError> {
        let start = if self.reason.is_empty() {
            format!("{SIP_VERSION} {}", self.status)
        } else {
            format!("{SIP_VERSION} {} {}", self.status, self.reason)
        };
        serialize(&start, &self.headers, &self.body, limits)
    }
}

impl fmt::Debug for SipResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SipResponse")
            .field("status", &self.status)
            .field("header_count", &self.headers.len())
            .field("body_bytes", &self.body.len())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum SipMessage {
    Request(SipRequest),
    Response(SipResponse),
}

impl SipMessage {
    pub fn to_bytes(&self, limits: SipLimits) -> Result<Vec<u8>, SipError> {
        match self {
            Self::Request(request) => request.to_bytes(limits),
            Self::Response(response) => response.to_bytes(limits),
        }
    }
}

impl fmt::Debug for SipMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(request) => request.fmt(formatter),
            Self::Response(response) => response.fmt(formatter),
        }
    }
}

pub fn parse_frame(frame: &[u8], limits: SipLimits) -> Result<SipMessage, SipError> {
    validate_limits(limits)?;
    if frame.len() > limits.max_message_bytes {
        return Err(SipError::MessageTooLarge);
    }

    let separator = frame
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or(SipError::IncompleteHeaders)?;
    let head_end = separator + 4;
    if head_end > limits.max_header_bytes {
        return Err(SipError::HeadersTooLarge);
    }
    let head = std::str::from_utf8(&frame[..separator]).map_err(|_| SipError::InvalidText)?;
    let mut lines = head.split("\r\n");
    let start = lines.next().ok_or(SipError::InvalidStartLine)?;
    if start.is_empty() || start.len() > limits.max_line_bytes {
        return Err(SipError::InvalidStartLine);
    }

    let mut headers = SipHeaders::default();
    for line in lines {
        if line.is_empty()
            || line.len() > limits.max_line_bytes
            || line.starts_with([' ', '\t'])
            || headers.len() == limits.max_header_count
        {
            return Err(if headers.len() == limits.max_header_count {
                SipError::TooManyHeaders
            } else {
                SipError::InvalidHeader
            });
        }
        let (name, value) = line.split_once(':').ok_or(SipError::InvalidHeader)?;
        headers.push(name, value.trim())?;
    }

    let content_length = {
        let mut content_lengths = headers.get_all("Content-Length");
        match (content_lengths.next(), content_lengths.next()) {
            (None, _) => 0,
            (Some(value), None) => value
                .parse::<usize>()
                .map_err(|_| SipError::InvalidContentLength)?,
            (Some(_), Some(_)) => return Err(SipError::AmbiguousContentLength),
        }
    };
    if content_length > limits.max_body_bytes {
        return Err(SipError::BodyTooLarge);
    }
    let body = &frame[head_end..];
    if body.len() != content_length {
        return Err(SipError::BodyLengthMismatch);
    }

    if let Some(rest) = start.strip_prefix("SIP/2.0 ") {
        let (status, reason) = rest.split_once(' ').unwrap_or((rest, ""));
        if status.len() != 3 || !status.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(SipError::InvalidStartLine);
        }
        let mut response = SipResponse::new(
            status
                .parse::<u16>()
                .map_err(|_| SipError::InvalidStartLine)?,
            reason,
        )?;
        response.headers = headers;
        response.body = body.to_vec();
        Ok(SipMessage::Response(response))
    } else {
        let mut parts = start.split(' ');
        let method = parts.next().ok_or(SipError::InvalidStartLine)?;
        let uri = parts.next().ok_or(SipError::InvalidStartLine)?;
        let version = parts.next().ok_or(SipError::InvalidStartLine)?;
        if parts.next().is_some() || version != SIP_VERSION {
            return Err(SipError::InvalidStartLine);
        }
        let mut request = SipRequest::new(SipMethod::parse(method)?, uri)?;
        request.headers = headers;
        request.body = body.to_vec();
        Ok(SipMessage::Request(request))
    }
}

fn serialize(
    start: &str,
    headers: &SipHeaders,
    body: &[u8],
    limits: SipLimits,
) -> Result<Vec<u8>, SipError> {
    validate_limits(limits)?;
    if start.len() > limits.max_line_bytes {
        return Err(SipError::InvalidStartLine);
    }
    if body.len() > limits.max_body_bytes {
        return Err(SipError::BodyTooLarge);
    }
    let emitted_headers = headers
        .iter()
        .filter(|header| !header_name_eq(header.name(), "Content-Length"));
    if headers
        .iter()
        .filter(|header| !header_name_eq(header.name(), "Content-Length"))
        .count()
        + 1
        > limits.max_header_count
    {
        return Err(SipError::TooManyHeaders);
    }

    let mut wire = Vec::new();
    append_line(&mut wire, start, limits.max_line_bytes)?;
    for header in emitted_headers {
        let line = format!("{}: {}", header.name(), header.value());
        append_line(&mut wire, &line, limits.max_line_bytes)?;
    }
    append_line(
        &mut wire,
        &format!("Content-Length: {}", body.len()),
        limits.max_line_bytes,
    )?;
    wire.extend_from_slice(b"\r\n");
    if wire.len() > limits.max_header_bytes {
        return Err(SipError::HeadersTooLarge);
    }
    wire.extend_from_slice(body);
    if wire.len() > limits.max_message_bytes {
        return Err(SipError::MessageTooLarge);
    }
    Ok(wire)
}

fn append_line(wire: &mut Vec<u8>, line: &str, max: usize) -> Result<(), SipError> {
    if line.len() > max || contains_line_break(line) {
        return Err(SipError::InvalidHeader);
    }
    wire.extend_from_slice(line.as_bytes());
    wire.extend_from_slice(b"\r\n");
    Ok(())
}

fn validate_limits(limits: SipLimits) -> Result<(), SipError> {
    if limits.max_message_bytes == 0
        || limits.max_header_bytes < 4
        || limits.max_header_count == 0
        || limits.max_line_bytes == 0
        || limits.max_body_bytes > limits.max_message_bytes
        || limits.max_header_bytes > limits.max_message_bytes
    {
        return Err(SipError::InvalidLimits);
    }
    Ok(())
}

fn header_name_eq(left: &str, right: &str) -> bool {
    match (compact_header_name(left), compact_header_name(right)) {
        (Some(left), Some(right)) => left == right,
        (None, None) => left.eq_ignore_ascii_case(right),
        _ => false,
    }
}

fn compact_header_name(name: &str) -> Option<&'static str> {
    if name.eq_ignore_ascii_case("v") || name.eq_ignore_ascii_case("via") {
        Some("via")
    } else if name.eq_ignore_ascii_case("f") || name.eq_ignore_ascii_case("from") {
        Some("from")
    } else if name.eq_ignore_ascii_case("t") || name.eq_ignore_ascii_case("to") {
        Some("to")
    } else if name.eq_ignore_ascii_case("i") || name.eq_ignore_ascii_case("call-id") {
        Some("call-id")
    } else if name.eq_ignore_ascii_case("m") || name.eq_ignore_ascii_case("contact") {
        Some("contact")
    } else if name.eq_ignore_ascii_case("c") || name.eq_ignore_ascii_case("content-type") {
        Some("content-type")
    } else if name.eq_ignore_ascii_case("l") || name.eq_ignore_ascii_case("content-length") {
        Some("content-length")
    } else {
        None
    }
}

fn is_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'-' | b'.' | b'!' | b'%' | b'*' | b'_' | b'+' | b'`' | b'\''
                )
        })
}

fn contains_line_break(value: &str) -> bool {
    value.contains(['\r', '\n'])
}

fn valid_start_line_part(value: &str) -> bool {
    !value.is_empty()
        && !value
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
}

pub(crate) fn via_branch(value: &str) -> Option<&str> {
    value.split(';').skip(1).find_map(|parameter| {
        let (name, value) = parameter.trim().split_once('=')?;
        let value = value.trim();
        (name.eq_ignore_ascii_case("branch") && !value.is_empty()).then_some(value)
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SipError {
    #[error("SIP limits are invalid")]
    InvalidLimits,
    #[error("SIP message exceeds the configured limit")]
    MessageTooLarge,
    #[error("SIP header section exceeds the configured limit")]
    HeadersTooLarge,
    #[error("SIP body exceeds the configured limit")]
    BodyTooLarge,
    #[error("SIP message has too many headers")]
    TooManyHeaders,
    #[error("SIP headers are incomplete")]
    IncompleteHeaders,
    #[error("SIP text is invalid")]
    InvalidText,
    #[error("SIP start line is invalid")]
    InvalidStartLine,
    #[error("SIP header is invalid")]
    InvalidHeader,
    #[error("SIP Content-Length is invalid")]
    InvalidContentLength,
    #[error("SIP Content-Length is ambiguous")]
    AmbiguousContentLength,
    #[error("SIP body length does not match Content-Length")]
    BodyLengthMismatch,
}
