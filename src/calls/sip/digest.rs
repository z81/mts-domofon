use std::fmt;

use md5::{Digest, Md5};
use secrecy::{ExposeSecret, SecretString};

const MAX_CHALLENGE_BYTES: usize = 4096;

#[derive(Clone, PartialEq, Eq)]
pub struct DigestChallenge {
    realm: String,
    nonce: String,
}

impl DigestChallenge {
    pub fn parse(value: &str) -> Result<Self, DigestError> {
        if value.len() > MAX_CHALLENGE_BYTES {
            return Err(DigestError::ChallengeTooLarge);
        }
        let value = value.trim();
        let split_at = value
            .find(char::is_whitespace)
            .ok_or(DigestError::UnsupportedScheme)?;
        let (scheme, parameters) = value.split_at(split_at);
        if !scheme.eq_ignore_ascii_case("Digest") {
            return Err(DigestError::UnsupportedScheme);
        }
        let parameters = parameters.trim_start();

        let mut realm = None;
        let mut nonce = None;
        let mut algorithm = None;
        let mut saw_qop = false;
        for parameter in split_parameters(parameters)? {
            let (name, raw_value) = parameter
                .split_once('=')
                .ok_or(DigestError::MalformedChallenge)?;
            let name = name.trim();
            let parsed = unquote(raw_value.trim())?;
            if name.eq_ignore_ascii_case("realm") {
                set_once(&mut realm, parsed)?;
            } else if name.eq_ignore_ascii_case("nonce") {
                set_once(&mut nonce, parsed)?;
            } else if name.eq_ignore_ascii_case("algorithm") {
                set_once(&mut algorithm, parsed)?;
            } else if name.eq_ignore_ascii_case("qop") {
                saw_qop = true;
            }
        }
        if saw_qop {
            return Err(DigestError::UnsupportedQop);
        }
        if let Some(algorithm) = algorithm
            && !algorithm.eq_ignore_ascii_case("MD5")
        {
            return Err(DigestError::UnsupportedAlgorithm);
        }
        let realm = realm.ok_or(DigestError::MissingRealm)?;
        let nonce = nonce.ok_or(DigestError::MissingNonce)?;
        if !safe_quoted_value(&realm) || !safe_quoted_value(&nonce) {
            return Err(DigestError::MalformedChallenge);
        }
        Ok(Self { realm, nonce })
    }

    pub fn realm(&self) -> &str {
        &self.realm
    }
}

impl fmt::Debug for DigestChallenge {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DigestChallenge")
            .field("algorithm", &"MD5")
            .field("has_realm", &true)
            .field("has_nonce", &true)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct DigestRequest {
    method: String,
    uri: String,
}

impl DigestRequest {
    pub fn new(method: &str, uri: &str) -> Result<Self, DigestError> {
        if method.is_empty()
            || !method.bytes().all(|byte| byte.is_ascii_uppercase())
            || !safe_quoted_value(uri)
            || uri.bytes().any(|byte| byte.is_ascii_whitespace())
        {
            return Err(DigestError::InvalidRequest);
        }
        Ok(Self {
            method: method.to_owned(),
            uri: uri.to_owned(),
        })
    }
}

impl fmt::Debug for DigestRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DigestRequest")
            .field("method", &self.method)
            .field("uri", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SensitiveHeaderValue(String);

impl SensitiveHeaderValue {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SensitiveHeaderValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SensitiveHeaderValue([REDACTED])")
    }
}

pub fn digest_authorization(
    challenge: &DigestChallenge,
    request: &DigestRequest,
    username: &SecretString,
    password: &SecretString,
) -> Result<SensitiveHeaderValue, DigestError> {
    let username = username.expose_secret();
    let password = password.expose_secret();
    if !safe_quoted_value(username) || contains_control(password) {
        return Err(DigestError::InvalidCredentials);
    }

    let ha1 = md5_hex_parts(&[
        username.as_bytes(),
        b":",
        challenge.realm.as_bytes(),
        b":",
        password.as_bytes(),
    ]);
    let ha2 = md5_hex_parts(&[request.method.as_bytes(), b":", request.uri.as_bytes()]);
    let response = md5_hex_parts(&[
        ha1.as_bytes(),
        b":",
        challenge.nonce.as_bytes(),
        b":",
        ha2.as_bytes(),
    ]);
    Ok(SensitiveHeaderValue(format!(
        concat!(
            "Digest algorithm=MD5,username=\"{}\",realm=\"{}\",",
            "nonce=\"{}\",uri=\"{}\",response=\"{}\""
        ),
        username, challenge.realm, challenge.nonce, request.uri, response
    )))
}

fn md5_hex_parts(parts: &[&[u8]]) -> String {
    let mut hasher = Md5::new();
    for part in parts {
        hasher.update(part);
    }
    let digest = hasher.finalize();
    let mut output = String::with_capacity(32);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn split_parameters(input: &str) -> Result<Vec<&str>, DigestError> {
    let mut parts = Vec::new();
    let mut quoted = false;
    let mut escaped = false;
    let mut start = 0;
    for (index, character) in input.char_indices() {
        if escaped {
            escaped = false;
        } else if character == '\\' && quoted {
            escaped = true;
        } else if character == '"' {
            quoted = !quoted;
        } else if character == ',' && !quoted {
            parts.push(input[start..index].trim());
            start = index + 1;
        }
    }
    if quoted || escaped {
        return Err(DigestError::MalformedChallenge);
    }
    parts.push(input[start..].trim());
    if parts.iter().any(|part| part.is_empty()) {
        return Err(DigestError::MalformedChallenge);
    }
    Ok(parts)
}

fn unquote(value: &str) -> Result<String, DigestError> {
    if let Some(inner) = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    {
        if inner.contains(['"', '\\']) {
            return Err(DigestError::MalformedChallenge);
        }
        Ok(inner.to_owned())
    } else if value.contains('"') {
        Err(DigestError::MalformedChallenge)
    } else {
        Ok(value.to_owned())
    }
}

fn set_once(slot: &mut Option<String>, value: String) -> Result<(), DigestError> {
    if slot.replace(value).is_some() {
        return Err(DigestError::DuplicateParameter);
    }
    Ok(())
}

fn safe_quoted_value(value: &str) -> bool {
    !value.is_empty() && !value.contains(['"', '\\', '\r', '\n'])
}

fn contains_control(value: &str) -> bool {
    value.bytes().any(|byte| byte.is_ascii_control())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DigestError {
    #[error("SIP Digest challenge exceeds the configured limit")]
    ChallengeTooLarge,
    #[error("unsupported SIP authorization scheme")]
    UnsupportedScheme,
    #[error("malformed SIP Digest challenge")]
    MalformedChallenge,
    #[error("duplicate SIP Digest parameter")]
    DuplicateParameter,
    #[error("SIP Digest realm is missing")]
    MissingRealm,
    #[error("SIP Digest nonce is missing")]
    MissingNonce,
    #[error("SIP Digest algorithm is unsupported")]
    UnsupportedAlgorithm,
    #[error("SIP Digest qop is unsupported by the observed APK contract")]
    UnsupportedQop,
    #[error("SIP Digest request is invalid")]
    InvalidRequest,
    #[error("SIP Digest credentials are invalid")]
    InvalidCredentials,
}
