use std::{
    collections::{HashMap, HashSet},
    fmt,
};

use async_trait::async_trait;
use reqwest::Method;

use crate::{
    domain::{CallMode, CapabilitySet, Flat, FlatId},
    error::{AppError, ErrorCode},
};

use super::{
    MtsClient, ProviderBase,
    dto::dashboard::{CallSettingsDto, CallSettingsRequest, FlatServiceDto, ResidentFlatDto},
};

const FLATS_PATH: &str = "/vdome-smart-facade/estate/api/v1/residents/flats";
const CALL_SETTINGS_PATH: &str = "/vdome-smart-facade/devices/api/v1/call-settings";
const MAX_ARRAY_ITEMS: usize = 512;
const MAX_TOTAL_DTO_ITEMS: usize = 1_024;
const MAX_FLATS: usize = 512;
const MAX_ID_BYTES: usize = 512;
const MAX_LABEL_BYTES: usize = 1_024;
const MAX_ADDRESS_BYTES: usize = 4_096;
const MAX_PROVIDER_VALUE_BYTES: usize = 512;

#[derive(Clone)]
pub struct MtsDashboardProvider {
    client: MtsClient,
}

/// Bounded provider capability names retained only for internal diagnostics.
pub struct AvailabilitySnapshot {
    raw_names: Vec<String>,
}

impl fmt::Debug for AvailabilitySnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AvailabilitySnapshot")
            .field(
                "raw_names",
                &format_args!("[REDACTED; {}]", self.raw_names.len()),
            )
            .finish()
    }
}

/// Bounded service aliases retained internally; only confirmed aliases compose capabilities.
pub struct ServiceSnapshot {
    aliases: Vec<ServiceAlias>,
}

impl fmt::Debug for ServiceSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServiceSnapshot")
            .field(
                "aliases",
                &format_args!("[REDACTED; {}]", self.aliases.len()),
            )
            .finish()
    }
}

enum ServiceAlias {
    AppCall,
    CloudArchive,
    Unknown(String),
}

impl ServiceSnapshot {
    pub(crate) fn has_app_call(&self) -> bool {
        self.aliases
            .iter()
            .any(|alias| matches!(alias, ServiceAlias::AppCall))
    }

    pub(crate) fn has_cloud_archive(&self) -> bool {
        self.aliases
            .iter()
            .any(|alias| matches!(alias, ServiceAlias::CloudArchive))
    }

    pub(crate) fn retained_value_count(&self) -> usize {
        self.aliases
            .iter()
            .map(|alias| match alias {
                ServiceAlias::AppCall | ServiceAlias::CloudArchive => 1,
                ServiceAlias::Unknown(value) => usize::from(!value.is_empty()),
            })
            .sum()
    }
}

impl AvailabilitySnapshot {
    pub(crate) fn retained_value_count(&self) -> usize {
        self.raw_names.len()
    }
}

#[async_trait]
pub trait PropertyProvider: Send + Sync {
    async fn flats(&self) -> Result<Vec<Flat>, AppError>;
    async fn availability(&self, flat: &FlatId) -> Result<AvailabilitySnapshot, AppError>;
    async fn services(&self, flat: &FlatId) -> Result<ServiceSnapshot, AppError>;
    async fn call_settings(&self) -> Result<CallMode, AppError>;
    async fn update_call_settings(&self, mode: &CallMode) -> Result<(), AppError>;
}

impl MtsDashboardProvider {
    pub fn new(client: MtsClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl PropertyProvider for MtsDashboardProvider {
    async fn flats(&self) -> Result<Vec<Flat>, AppError> {
        let request = self
            .client
            .authorized(Method::GET, ProviderBase::Gateway, FLATS_PATH)?;
        let resident_flats: Option<Vec<ResidentFlatDto>> = self.client.send_json(request).await?;
        map_flats(resident_flats.unwrap_or_default())
    }

    async fn availability(&self, flat: &FlatId) -> Result<AvailabilitySnapshot, AppError> {
        validate_required(&flat.0, MAX_ID_BYTES)?;
        let request = self.client.authorized_with_path_segment(
            Method::GET,
            ProviderBase::Gateway,
            "/vdome-smart-facade/devices/api/v1/flats",
            &flat.0,
            Some("available"),
        )?;
        let names: Option<Vec<String>> = self.client.send_json(request).await?;
        let raw_names = names.unwrap_or_default();
        validate_provider_values(&raw_names)?;
        Ok(AvailabilitySnapshot { raw_names })
    }

    async fn services(&self, flat: &FlatId) -> Result<ServiceSnapshot, AppError> {
        validate_required(&flat.0, MAX_ID_BYTES)?;
        let request = self.client.authorized_with_path_segment(
            Method::GET,
            ProviderBase::Gateway,
            "/vdome-smart-facade/services/api/v1/residents/flats",
            &flat.0,
            None,
        )?;
        let services: Option<Vec<FlatServiceDto>> = self.client.send_json(request).await?;
        let services = services.unwrap_or_default();
        if services.len() > MAX_ARRAY_ITEMS {
            return Err(schema_error());
        }
        let mut aliases = Vec::with_capacity(services.len());
        for service in services {
            validate_provider_value(&service.alias)?;
            aliases.push(match service.alias.as_str() {
                "app_call" => ServiceAlias::AppCall,
                "cloud_archive" => ServiceAlias::CloudArchive,
                _ => ServiceAlias::Unknown(service.alias),
            });
        }
        Ok(ServiceSnapshot { aliases })
    }

    async fn call_settings(&self) -> Result<CallMode, AppError> {
        let request =
            self.client
                .authorized(Method::GET, ProviderBase::Gateway, CALL_SETTINGS_PATH)?;
        let settings: CallSettingsDto = self.client.send_json(request).await?;
        validate_provider_value(&settings.mode)?;
        Ok(map_call_mode(settings.mode))
    }

    async fn update_call_settings(&self, mode: &CallMode) -> Result<(), AppError> {
        let raw = writable_call_mode(mode)?;
        let request = self
            .client
            .authorized(Method::POST, ProviderBase::Gateway, CALL_SETTINGS_PATH)?
            .json(&CallSettingsRequest { mode: raw })?;
        self.client.send_empty(request).await
    }
}

fn map_flats(residents: Vec<ResidentFlatDto>) -> Result<Vec<Flat>, AppError> {
    if residents.len() > MAX_ARRAY_ITEMS {
        return Err(schema_error());
    }
    let mut flats = Vec::new();
    let mut ids = HashSet::new();
    let mut total_dto_items = 0_usize;
    let mut total_flats = 0_usize;
    for resident in residents {
        let parts = resident.parts.unwrap_or_default();
        let buildings = resident.buildings.unwrap_or_default();
        if parts.len() > MAX_ARRAY_ITEMS || buildings.len() > MAX_ARRAY_ITEMS {
            return Err(schema_error());
        }
        total_dto_items = total_dto_items
            .checked_add(parts.len())
            .and_then(|total| total.checked_add(buildings.len()))
            .filter(|total| *total <= MAX_TOTAL_DTO_ITEMS)
            .ok_or_else(schema_error)?;
        total_flats = total_flats
            .checked_add(parts.len())
            .filter(|total| *total <= MAX_FLATS)
            .ok_or_else(schema_error)?;

        let mut buildings_by_id = HashMap::with_capacity(buildings.len());
        for building in &buildings {
            validate_required(&building.id, MAX_ID_BYTES)?;
            if buildings_by_id
                .insert(building.id.as_str(), building)
                .is_some()
            {
                return Err(schema_error());
            }
        }
        for part in parts {
            validate_required(&part.id, MAX_ID_BYTES)?;
            validate_required(&part.building_id, MAX_ID_BYTES)?;
            if !ids.insert(part.id.clone()) {
                return Err(schema_error());
            }
            let building = buildings_by_id
                .get(part.building_id.as_str())
                .copied()
                .ok_or_else(schema_error)?;
            let label = part
                .display_name
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(&part.name)
                .trim();
            validate_required(label, MAX_LABEL_BYTES)?;
            let address = building
                .address
                .full_address
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .or_else(|| {
                    building
                        .address
                        .short_address
                        .as_deref()
                        .filter(|value| !value.trim().is_empty())
                })
                .ok_or_else(schema_error)?
                .trim();
            validate_required(address, MAX_ADDRESS_BYTES)?;
            flats.push(Flat {
                id: FlatId(part.id),
                label: label.to_owned(),
                address: address.to_owned(),
                capabilities: CapabilitySet::unavailable(),
            });
        }
    }
    Ok(flats)
}

fn validate_required(value: &str, maximum: usize) -> Result<(), AppError> {
    if value.trim().is_empty() || value.len() > maximum {
        return Err(schema_error());
    }
    Ok(())
}

fn validate_provider_values(values: &[String]) -> Result<(), AppError> {
    if values.len() > MAX_ARRAY_ITEMS {
        return Err(schema_error());
    }
    values
        .iter()
        .try_for_each(|value| validate_provider_value(value))
}

fn validate_provider_value(value: &str) -> Result<(), AppError> {
    validate_required(value, MAX_PROVIDER_VALUE_BYTES)
}

fn map_call_mode(value: String) -> CallMode {
    match value.as_str() {
        "gsm" => CallMode::Gsm,
        "sip" => CallMode::Sip,
        "both" => CallMode::Both,
        _ => CallMode::Unknown(value),
    }
}

fn writable_call_mode(mode: &CallMode) -> Result<&'static str, AppError> {
    match mode {
        CallMode::Gsm => Ok("gsm"),
        CallMode::Sip => Ok("sip"),
        CallMode::Both => Ok("both"),
        CallMode::Unknown(_) => Err(AppError::invalid_request(
            "Unknown call settings cannot be written.",
        )),
    }
}

fn schema_error() -> AppError {
    AppError::new(
        ErrorCode::UpstreamSchemaChanged,
        "Provider dashboard response has an unsupported schema.",
        false,
    )
}
