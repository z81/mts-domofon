use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An opaque identifier for a flat linked to the authenticated account.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FlatId(pub String);

impl fmt::Debug for FlatId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FlatId([REDACTED])")
    }
}

/// A linked flat and the optional capabilities available for it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Flat {
    pub id: FlatId,
    pub label: String,
    pub address: String,
    pub capabilities: CapabilitySet,
}

impl fmt::Debug for Flat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Flat")
            .field("id", &self.id)
            .field("label", &"[REDACTED]")
            .field("address", &"[REDACTED]")
            .field("capabilities", &self.capabilities)
            .finish()
    }
}

/// The account-scoped capabilities used by the dashboard and device views.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilitySet {
    pub access_points: Capability,
    pub cameras: Capability,
    pub archive: Capability,
    pub events: Capability,
    pub calls: Capability,
    pub call_settings: Capability,
}

/// Whether a capability can currently be used.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum Capability {
    Available,
    Unavailable {
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

impl CapabilitySet {
    pub(crate) fn unavailable() -> Self {
        Self {
            access_points: unavailable_capability(),
            cameras: unavailable_capability(),
            archive: unavailable_capability(),
            events: unavailable_capability(),
            calls: unavailable_capability(),
            call_settings: unavailable_capability(),
        }
    }
}

fn unavailable_capability() -> Capability {
    Capability::Unavailable { reason: None }
}

/// The account-scoped route used for incoming calls.
#[non_exhaustive]
#[derive(Clone, PartialEq, Eq)]
pub enum CallMode {
    Gsm,
    Sip,
    Both,
    Unknown(String),
}

impl fmt::Debug for CallMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gsm => formatter.write_str("Gsm"),
            Self::Sip => formatter.write_str("Sip"),
            Self::Both => formatter.write_str("Both"),
            Self::Unknown(_) => formatter.write_str("Unknown([REDACTED])"),
        }
    }
}

impl Serialize for CallMode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(match self {
            Self::Gsm => "gsm",
            Self::Sip => "sip",
            Self::Both => "both",
            Self::Unknown(_) => "unknown",
        })
    }
}

impl<'de> Deserialize<'de> for CallMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(match value.as_str() {
            "gsm" => Self::Gsm,
            "sip" => Self::Sip,
            "both" => Self::Both,
            _ => Self::Unknown(value),
        })
    }
}

/// A call setting paired with a local optimistic-concurrency revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallSettings {
    pub mode: CallMode,
    pub revision: u64,
}

/// A locally guarded call-setting mutation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallSettingsPatch {
    pub mode: CallMode,
    pub expected_revision: u64,
}

/// Account-scoped call settings as rendered beside the local dashboard.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "state", content = "settings")]
pub enum CallSettingsStatus {
    Available(CallSettings),
    Unavailable,
}

/// The linked-property dashboard. Capability failures remain attached to each flat.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PropertyDashboard {
    pub flats: Vec<Flat>,
    pub selected_flat_id: Option<FlatId>,
    pub call_settings: CallSettingsStatus,
}
