use std::fmt;

use chrono::{DateTime, Utc};
use secrecy::SecretString;
use uuid::Uuid;

#[derive(Clone)]
pub struct TokenBundle {
    pub access_token: SecretString,
    pub refresh_token: SecretString,
    pub web_sso_access_token: SecretString,
    pub device_token: Option<SecretString>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DeviceIdentity {
    pub uuid: Uuid,
}

#[derive(Clone)]
pub struct PersistedSession {
    pub tokens: TokenBundle,
    pub authenticated_at: DateTime<Utc>,
}

impl fmt::Debug for PersistedSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PersistedSession")
            .field("has_access_token", &true)
            .field("has_refresh_token", &true)
            .field("has_web_sso_access_token", &true)
            .field("has_device_token", &self.tokens.device_token.is_some())
            .field("authenticated_at", &self.authenticated_at)
            .finish()
    }
}

#[cfg(test)]
impl TokenBundle {
    pub fn expose_for_test(&self) -> (&str, &str, &str, Option<&str>) {
        use secrecy::ExposeSecret;

        (
            self.access_token.expose_secret(),
            self.refresh_token.expose_secret(),
            self.web_sso_access_token.expose_secret(),
            self.device_token.as_ref().map(ExposeSecret::expose_secret),
        )
    }
}

#[cfg(test)]
impl PersistedSession {
    pub fn synthetic_for_test() -> Self {
        Self {
            tokens: TokenBundle {
                access_token: SecretString::from("access-test"),
                refresh_token: SecretString::from("refresh-test"),
                web_sso_access_token: SecretString::from("sso-test"),
                device_token: Some(SecretString::from("device-token-test")),
            },
            authenticated_at: Utc::now(),
        }
    }
}
