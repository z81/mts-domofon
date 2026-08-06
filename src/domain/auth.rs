use std::{fmt, str::FromStr};

use chrono::{DateTime, Utc};
use secrecy::SecretString;
use uuid::Uuid;

#[derive(Clone, Eq, Hash, PartialEq, serde::Serialize)]
#[serde(transparent)]
pub struct AuthFlowId(String);

impl AuthFlowId {
    pub(crate) fn fresh() -> Self {
        Self(Uuid::new_v4().simple().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AuthFlowId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("AuthFlowId").field(&self.0).finish()
    }
}

impl fmt::Display for AuthFlowId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for AuthFlowId {
    type Err = crate::error::AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let uuid = Uuid::parse_str(value)
            .map_err(|_| crate::error::AppError::invalid_request("invalid auth flow ID"))?;
        Ok(Self(uuid.simple().to_string()))
    }
}

impl<'de> serde::Deserialize<'de> for AuthFlowId {
    fn deserialize<Deserializer>(deserializer: Deserializer) -> Result<Self, Deserializer::Error>
    where
        Deserializer: serde::Deserializer<'de>,
    {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        value
            .parse()
            .map_err(|_| serde::de::Error::custom("invalid auth flow ID"))
    }
}

#[derive(Clone, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct AuthAccountOption {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Eq, PartialEq, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AuthChallenge {
    Phone,
    SmsOtp { resend_after_seconds: u64 },
    Captcha { image_data_url: String },
    Password,
    AccountSelection { options: Vec<AuthAccountOption> },
    Consent { title: String, body: String },
}

#[derive(Clone, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ChallengeInput {
    Phone { phone: String },
    SmsOtp { code: SecretString },
    Captcha { answer: SecretString },
    Password { password: SecretString },
    Account { id: String },
    Consent { accepted: bool },
}

impl fmt::Debug for ChallengeInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::Phone { .. } => "phone",
            Self::SmsOtp { .. } => "sms_otp",
            Self::Captcha { .. } => "captcha",
            Self::Password { .. } => "password",
            Self::Account { .. } => "account",
            Self::Consent { .. } => "consent",
        };
        formatter
            .debug_struct("ChallengeInput")
            .field("kind", &kind)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Anonymous,
    Active,
    Expired,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct SessionView {
    pub authenticated: bool,
    pub status: SessionStatus,
    pub authenticated_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
}

impl SessionView {
    pub fn anonymous() -> Self {
        Self {
            authenticated: false,
            status: SessionStatus::Anonymous,
            authenticated_at: None,
            expires_at: None,
        }
    }
}

#[derive(serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AuthProgress {
    BrowserRedirect {
        flow_id: AuthFlowId,
        authorization_url: url::Url,
    },
    Challenge {
        flow_id: AuthFlowId,
        challenge: AuthChallenge,
    },
    Authenticated {
        session: SessionView,
    },
}

impl fmt::Debug for AuthProgress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BrowserRedirect {
                flow_id,
                authorization_url,
            } => formatter
                .debug_struct("BrowserRedirect")
                .field("flow_id", flow_id)
                .field(
                    "authorization_origin",
                    &authorization_url.origin().ascii_serialization(),
                )
                .field("authorization_path", &authorization_url.path())
                .finish(),
            Self::Challenge { flow_id, .. } => formatter
                .debug_struct("Challenge")
                .field("flow_id", flow_id)
                .field("challenge", &"[REDACTED]")
                .finish(),
            Self::Authenticated { session } => formatter
                .debug_struct("Authenticated")
                .field("session", session)
                .finish(),
        }
    }
}

#[derive(Clone)]
pub struct TokenBundle {
    pub access_token: SecretString,
    pub refresh_token: SecretString,
    pub web_sso_access_token: Option<SecretString>,
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
            .field(
                "has_web_sso_access_token",
                &self.tokens.web_sso_access_token.is_some(),
            )
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
            self.web_sso_access_token
                .as_ref()
                .map(ExposeSecret::expose_secret)
                .unwrap_or(""),
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
                web_sso_access_token: Some(SecretString::from("sso-test")),
                device_token: Some(SecretString::from("device-token-test")),
            },
            authenticated_at: Utc::now(),
        }
    }
}
