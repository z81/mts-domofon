use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SpringPage<T> {
    pub(crate) content: Vec<T>,
    pub(crate) total_elements: u64,
    pub(crate) total_pages: u32,
    pub(crate) number: u32,
    pub(crate) size: u32,
    pub(crate) last: bool,
    pub(crate) first: bool,
    pub(crate) empty: bool,
    #[serde(flatten)]
    _extra: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
pub(crate) struct AccessPointDto {
    pub(crate) id: String,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) camera: Value,
    #[serde(default)]
    _addresses: Value,
    #[serde(default)]
    pub(crate) locks: Value,
    #[serde(flatten)]
    _extra: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CameraDto {
    pub(crate) id: String,
    pub(crate) name: String,
    #[serde(default)]
    _addresses: Value,
    #[serde(default)]
    pub(crate) streams: Value,
    #[serde(default)]
    pub(crate) streams_to_watch: Value,
    #[serde(default)]
    _stream_mode: Value,
    #[serde(default)]
    pub(crate) ivideon: Value,
    #[serde(default)]
    pub(crate) intercoms: Value,
    #[serde(flatten)]
    _extra: BTreeMap<String, Value>,
}
