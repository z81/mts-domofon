use std::{fmt, str::FromStr};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A short-lived, one-use local confirmation token for a physical door action.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DoorOpenIntentId(Uuid);

impl DoorOpenIntentId {
    pub(crate) fn generate() -> Self {
        Self(Uuid::new_v4())
    }
}

impl FromStr for DoorOpenIntentId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

impl fmt::Debug for DoorOpenIntentId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DoorOpenIntentId([REDACTED])")
    }
}

/// Public metadata returned when an open-door confirmation intent is created.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DoorOpenIntentView {
    pub intent_id: DoorOpenIntentId,
    pub expires_at: DateTime<Utc>,
}

impl fmt::Debug for DoorOpenIntentView {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DoorOpenIntentView")
            .field("intent_id", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}
