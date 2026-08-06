use std::{collections::HashSet, fmt};

use reqwest::Method;

use crate::error::{AppError, ErrorCode};

use super::{
    MtsClient,
    dto::bootstrap::{ConfigItem, FeatureItem},
};

const MAX_BOOTSTRAP_ITEMS: usize = 1_024;
const MAX_NAME_LEN: usize = 256;
const MAX_VALUE_LEN: usize = 64 * 1_024;
const ANDROID_PLATFORM: &str = "ANDROID";
const ANDROID_VERSION_CONFIG: &str = "ANDROID_APP_VERSION";
const ANDROID_UPDATE_CONFIG: &str = "ANDROID_UPDATE";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthType {
    WebSso,
}

#[derive(Clone, Eq, PartialEq)]
pub struct BootstrapConfig {
    pub config_name: String,
    pub platform: String,
    pub value: String,
}

impl fmt::Debug for BootstrapConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BootstrapConfig")
            .field("config_name", &self.config_name)
            .field("platform", &self.platform)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderBootstrap {
    pub auth_type: AuthType,
    pub android_version: Option<BootstrapConfig>,
    pub android_update: Option<BootstrapConfig>,
    pub unknown_config_names: Vec<String>,
    pub unknown_feature_names: Vec<String>,
}

#[derive(Clone)]
pub struct BootstrapProvider {
    client: MtsClient,
}

impl BootstrapProvider {
    pub fn new(client: MtsClient) -> Self {
        Self { client }
    }

    pub async fn fetch(&self) -> Result<ProviderBootstrap, AppError> {
        let configs: Option<Vec<ConfigItem>> = self
            .client
            .send_json(self.client.anonymous_gateway(Method::GET, "/config")?)
            .await?;
        let features: Option<Vec<FeatureItem>> = self
            .client
            .send_json(self.client.anonymous_gateway(Method::GET, "/feature")?)
            .await?;

        normalize(configs.unwrap_or_default(), features.unwrap_or_default())
    }
}

fn normalize(
    configs: Vec<ConfigItem>,
    features: Vec<FeatureItem>,
) -> Result<ProviderBootstrap, AppError> {
    if configs.len() > MAX_BOOTSTRAP_ITEMS || features.len() > MAX_BOOTSTRAP_ITEMS {
        return Err(schema_error());
    }

    let mut config_names = HashSet::with_capacity(configs.len());
    let mut android_version = None;
    let mut android_update = None;
    let mut unknown_config_names = Vec::new();
    for config in configs {
        validate_identifier(&config.config_name)?;
        validate_identifier(&config.platform)?;
        if !config_names.insert(config.config_name.clone()) {
            return Err(schema_error());
        }

        let destination = match (config.config_name.as_str(), config.platform.as_str()) {
            (ANDROID_VERSION_CONFIG, ANDROID_PLATFORM) => &mut android_version,
            (ANDROID_UPDATE_CONFIG, ANDROID_PLATFORM) => &mut android_update,
            _ => {
                validate_unknown_value(&config.value)?;
                unknown_config_names.push(config.config_name);
                continue;
            }
        };
        validate_known_value(&config.value)?;
        *destination = Some(BootstrapConfig {
            config_name: config.config_name,
            platform: config.platform,
            value: config.value,
        });
    }

    let mut feature_names = HashSet::with_capacity(features.len());
    let mut auth_value = None;
    let mut unknown_feature_names = Vec::new();
    for feature in features {
        validate_identifier(&feature.feature_name)?;
        if !feature_names.insert(feature.feature_name.clone()) {
            return Err(schema_error());
        }

        if feature.feature_name == "AUTH_TYPE" {
            validate_unknown_value(&feature.value)?;
            auth_value = Some(feature.value);
        } else {
            validate_unknown_value(&feature.value)?;
            unknown_feature_names.push(feature.feature_name);
        }
    }

    if auth_value.as_deref() != Some("WEB_SSO") {
        return Err(compatibility_error());
    }

    Ok(ProviderBootstrap {
        auth_type: AuthType::WebSso,
        android_version,
        android_update,
        unknown_config_names,
        unknown_feature_names,
    })
}

fn validate_identifier(value: &str) -> Result<(), AppError> {
    if value.is_empty()
        || value.len() > MAX_NAME_LEN
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(schema_error());
    }
    Ok(())
}

fn validate_known_value(value: &str) -> Result<(), AppError> {
    if value.is_empty() || value.len() > MAX_VALUE_LEN {
        return Err(schema_error());
    }
    Ok(())
}

fn validate_unknown_value(value: &str) -> Result<(), AppError> {
    if value.len() > MAX_VALUE_LEN {
        return Err(schema_error());
    }
    Ok(())
}

fn schema_error() -> AppError {
    AppError::new(
        ErrorCode::UpstreamSchemaChanged,
        "Provider bootstrap response has an unsupported schema.",
        false,
    )
}

fn compatibility_error() -> AppError {
    AppError::new(
        ErrorCode::UpstreamSchemaChanged,
        "Provider authentication mode is not supported.",
        false,
    )
}
