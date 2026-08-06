use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Serialize;

use crate::{
    domain::auth::DeviceIdentity,
    error::{AppError, ErrorCode},
};

/// The stable synthetic device shape accepted by MTS Login SDK 5.8.0.
#[derive(Serialize)]
pub struct MtsDeviceDescriptor {
    uuid: uuid::Uuid,
    platform: &'static str,
    vendor: &'static str,
    model: &'static str,
    name: &'static str,
    ip: &'static str,
    timezone: &'static str,
    locale: &'static str,
    useragent: &'static str,
    version: &'static str,
    #[serde(rename = "screenSize")]
    screen_size: &'static str,
    #[serde(rename = "systemVersion")]
    system_version: &'static str,
}

impl MtsDeviceDescriptor {
    pub fn from_identity(identity: &DeviceIdentity) -> Self {
        Self {
            uuid: identity.uuid,
            platform: "Android",
            vendor: "VDome Web",
            model: "Rust Service",
            name: "domru-mts",
            ip: "",
            timezone: "Europe/Moscow",
            locale: "ru_RU",
            useragent: "domru-mts/0.1",
            version: "5.8.0",
            screen_size: "",
            system_version: "server",
        }
    }

    pub fn encode(&self) -> Result<String, AppError> {
        let encoded = serde_json::to_vec(self).map_err(|error| {
            AppError::new(
                ErrorCode::Internal,
                "Failed to encode the MTS device descriptor.",
                false,
            )
            .with_cause(error)
        })?;
        Ok(STANDARD.encode(encoded))
    }
}
