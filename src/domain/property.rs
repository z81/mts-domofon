use serde::{Deserialize, Serialize};

/// An opaque identifier for a flat linked to the authenticated account.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FlatId(pub String);

/// A linked flat and the optional capabilities available for it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Flat {
    pub id: FlatId,
    pub label: String,
    pub address: String,
    pub capabilities: CapabilitySet,
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
