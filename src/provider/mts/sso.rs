use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use http::{HeaderMap, HeaderValue};
use rand::RngCore;
use subtle::ConstantTimeEq;
use url::Url;

use crate::error::AppError;

const AUTHORIZE_URL: &str = "https://login.mts.ru/amserver/oauth2/authorize";
const CALLBACK_URL: &str = "https://gateway.vdome.mts.ru/user-service/api/auth/callback";
const CALLBACK_HOST: &str = "gateway.vdome.mts.ru";
const SCOPE: &str = "account address sso profile ssom openid phone";

/// A server-side MTS ID authorization flow request.
pub struct SsoRequest {
    device_id: String,
    state: String,
    state_bytes: [u8; 32],
}

impl SsoRequest {
    pub fn new(device_id: impl Into<String>, state: impl Into<String>) -> Result<Self, AppError> {
        let state = state.into();
        let state_bytes = decode_state(&state)
            .ok_or_else(|| AppError::invalid_request("invalid MTS authorization state"))?;

        Ok(Self {
            device_id: device_id.into(),
            state,
            state_bytes,
        })
    }

    pub fn with_fresh_state(device_id: impl Into<String>) -> Self {
        Self::new(device_id, generate_state()).expect("generated MTS state is valid")
    }

    pub fn state(&self) -> &str {
        &self.state
    }

    pub fn authorize_url(&self) -> Url {
        let mut url = Url::parse(AUTHORIZE_URL).expect("MTS authorization URL is valid");
        url.query_pairs_mut()
            .append_pair("client_id", "VDome")
            .append_pair("redirect_uri", CALLBACK_URL)
            .append_pair("response_type", "code")
            .append_pair("scope", SCOPE)
            .append_pair("state", &self.state)
            .append_pair("device_id", &self.device_id);
        url
    }

    pub fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::with_capacity(5);
        headers.insert("X-SDK-APPVersion", HeaderValue::from_static("3.0.1"));
        headers.insert("X-SDK-Name", HeaderValue::from_static("Login"));
        headers.insert("X-SDK-ClientId", HeaderValue::from_static("VDome"));
        headers.insert("X-SDK-Platform", HeaderValue::from_static("Android"));
        headers.insert("X-SDK-Version", HeaderValue::from_static("5.8.0"));
        headers
    }

    pub fn validate_callback(&self, callback_raw: &str) -> Result<(), AppError> {
        if raw_callback_authority(callback_raw) != Some(CALLBACK_HOST) {
            return Err(AppError::invalid_request("invalid MTS callback URL"));
        }

        let callback = Url::parse(callback_raw)
            .map_err(|_| AppError::invalid_request("invalid MTS callback URL"))?;
        let is_expected_callback = callback.scheme() == "https"
            && callback.username().is_empty()
            && callback.password().is_none()
            && callback.host_str() == Some(CALLBACK_HOST)
            && callback.port().is_none()
            && callback.path() == "/user-service/api/auth/callback"
            && callback.fragment().is_none();
        if !is_expected_callback {
            return Err(AppError::invalid_request("invalid MTS callback URL"));
        }

        let mut state_count = 0_u8;
        let mut received_state = None;
        for (name, value) in callback.query_pairs() {
            if name == "state" {
                state_count = state_count.saturating_add(1);
                if received_state.is_none() {
                    received_state = Some(value);
                }
            }
        }

        let mut received_bytes = [0_u8; 32];
        let mut decoded = 0_u8;
        if let Some(value) = received_state.and_then(|value| decode_state(&value)) {
            received_bytes = value;
            decoded = 1;
        }
        let matches = self.state_bytes.ct_eq(&received_bytes).unwrap_u8();
        if state_count != 1 || decoded & matches != 1 {
            return Err(AppError::invalid_request("invalid MTS callback state"));
        }

        Ok(())
    }
}

pub fn generate_state() -> String {
    let mut bytes = [0_u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn raw_callback_authority(callback: &str) -> Option<&str> {
    let (scheme, remainder) = callback.split_once("://")?;
    if scheme != "https" {
        return None;
    }
    let authority = remainder
        .split(['/', '?', '#'])
        .next()
        .expect("split always returns at least one segment");
    (authority == CALLBACK_HOST).then_some(authority)
}

fn decode_state(state: &str) -> Option<[u8; 32]> {
    if state.len() != 43
        || !state
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return None;
    }

    URL_SAFE_NO_PAD.decode(state).ok()?.try_into().ok()
}
