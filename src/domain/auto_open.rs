use std::{fmt, str::FromStr};

use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoOpenMode {
    Disabled,
    NextCall,
    Until,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoOpenResultStatus {
    Opened,
    Failed,
    OutcomeUnknown,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoOpenResultView {
    pub status: AutoOpenResultStatus,
    pub completed_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoOpenPolicyView {
    pub mode: AutoOpenMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<DateTime<Utc>>,
    pub dismiss_after_open: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_result: Option<AutoOpenResultView>,
}

impl AutoOpenPolicyView {
    pub fn disabled() -> Self {
        Self {
            mode: AutoOpenMode::Disabled,
            until: None,
            dismiss_after_open: false,
            last_result: None,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(transparent)]
pub struct AutoOpenIntentId(Uuid);

impl AutoOpenIntentId {
    pub(crate) fn generate() -> Self {
        Self(Uuid::new_v4())
    }
}

impl fmt::Display for AutoOpenIntentId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for AutoOpenIntentId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoOpenIntentView {
    pub intent_id: AutoOpenIntentId,
    pub expires_at: DateTime<Utc>,
}
