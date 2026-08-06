use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResidentFlatDto {
    #[serde(default)]
    pub(crate) parts: Option<Vec<ResidentPartDto>>,
    #[serde(default)]
    pub(crate) buildings: Option<Vec<BuildingDto>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResidentPartDto {
    pub(crate) id: String,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) display_name: Option<String>,
    pub(crate) building_id: String,
}

#[derive(Deserialize)]
pub(crate) struct BuildingDto {
    pub(crate) id: String,
    pub(crate) address: AddressDto,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AddressDto {
    #[serde(default)]
    pub(crate) full_address: Option<String>,
    #[serde(default)]
    pub(crate) short_address: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct FlatServiceDto {
    pub(crate) alias: String,
}

#[derive(Deserialize)]
pub(crate) struct CallSettingsDto {
    #[serde(rename = "type")]
    pub(crate) mode: String,
}

#[derive(Serialize)]
pub(crate) struct CallSettingsRequest<'a> {
    #[serde(rename = "type")]
    pub(crate) mode: &'a str,
}
