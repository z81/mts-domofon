use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{Capability, FlatId};

/// A device through which the resident can enter the property.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessPoint {
    pub id: String,
    pub flat_id: FlatId,
    pub name: String,
    pub kind: AccessPointKind,
    pub locks: Vec<Lock>,
    pub camera_ids: Vec<String>,
    pub available: bool,
}

/// The stable category of an access point.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccessPointKind {
    Intercom,
    Barrier,
    Gate,
    Elevator,
    Door,
    Unknown(String),
}

impl Serialize for AccessPointKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let value = match self {
            Self::Intercom => "intercom",
            Self::Barrier => "barrier",
            Self::Gate => "gate",
            Self::Elevator => "elevator",
            Self::Door => "door",
            Self::Unknown(value) => value,
        };

        serializer.serialize_str(value)
    }
}

impl<'de> Deserialize<'de> for AccessPointKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;

        Ok(match value.as_str() {
            "intercom" => Self::Intercom,
            "barrier" => Self::Barrier,
            "gate" => Self::Gate,
            "elevator" => Self::Elevator,
            "door" => Self::Door,
            _ => Self::Unknown(value),
        })
    }
}

/// A separately controllable lock on an access point.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lock {
    pub number: String,
    pub name: String,
    pub permanently_open: bool,
}

/// A camera associated with a linked flat.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Camera {
    pub id: String,
    pub flat_id: FlatId,
    pub name: String,
    pub access_point_id: Option<String>,
    pub capabilities: CameraCapabilities,
}

/// The independently available camera features.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraCapabilities {
    pub snapshot: Capability,
    pub live: Capability,
    pub archive: Capability,
}

/// All discovered devices for one flat.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInventory {
    pub access_points: Vec<AccessPoint>,
    pub cameras: Vec<Camera>,
}
