use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use http::{HeaderMap, HeaderValue};
use rand::RngCore;
use subtle::ConstantTimeEq;
use url::Url;

use crate::error::AppError;

const AUTHORIZE_URL: &str = "https://login.mts.ru/amserver/oauth2/authorize";
const CALLBACK_URL: &str = "https://gateway.vdome.mts.ru/user-service/api/auth/callback";
const SCOPE: &str = "account address sso profile ssom openid phone";

/// A server-side MTS ID authorization flow request.
pub struct SsoRequest {
    device_id: String,
    state: String,
}

impl SsoRequest {
    pub fn new(device_id: impl Into<String>, state: impl Into<String>) -> Self {
        Self {
            device_id: device_id.into(),
            state: state.into(),
        }
    }

    pub fn with_fresh_state(device_id: impl Into<String>) -> Self {
        Self::new(device_id, generate_state())
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

    pub fn validate_callback(&self, callback: &Url) -> Result<(), AppError> {
        let is_expected_callback = callback.scheme() == "https"
            && callback.username().is_empty()
            && callback.password().is_none()
            && callback.host_str() == Some("gateway.vdome.mts.ru")
            && callback.port().is_none()
            && callback.path() == "/user-service/api/auth/callback"
            && callback.fragment().is_none();
        if !is_expected_callback {
            return Err(AppError::invalid_request("invalid MTS callback URL"));
        }

        let states: Vec<_> = callback
            .query_pairs()
            .filter_map(|(name, value)| (name == "state").then_some(value))
            .collect();
        let Some(received_state) = states.first() else {
            return Err(AppError::invalid_request("invalid MTS callback state"));
        };
        if states.len() != 1
            || self
                .state
                .as_bytes()
                .ct_eq(received_state.as_bytes())
                .unwrap_u8()
                != 1
        {
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
