use std::collections::{BTreeMap, btree_map::Entry};

use async_trait::async_trait;
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde_json::Value;
use url::form_urlencoded;

use crate::{
    domain::{AccessPoint, AccessPointKind, Camera, CameraCapabilities, Capability, FlatId, Lock},
    error::{AppError, ErrorCode},
};

use super::{
    MtsClient, ProviderBase,
    dto::intercom::{AccessPointDto, CameraDto, SpringPage},
};

const ACCESS_POINTS_PATH: &str = "/vdome-smart-facade/devices/api/v1/accessPoints";
const CAMERAS_PATH: &str = "/vdome-smart-facade/devices/api/v1/cameras";
const PAGE_SIZE: u32 = 100;
const MAX_PAGES: u32 = 50;
const MAX_ITEMS: u64 = 5_000;
const MAX_SCOPE_BYTES: usize = 512;
const MAX_ID_BYTES: usize = 512;
const MAX_NAME_BYTES: usize = 2_048;
const UNKNOWN_KIND: &str = "unknown";

#[async_trait]
pub trait DeviceProvider: Send + Sync {
    async fn access_points(
        &self,
        flat: &FlatId,
        category: Option<&str>,
    ) -> Result<Vec<AccessPoint>, AppError>;

    async fn cameras(&self, flat: &FlatId) -> Result<Vec<Camera>, AppError>;
}

#[derive(Clone)]
pub struct MtsDeviceProvider {
    client: MtsClient,
}

impl MtsDeviceProvider {
    pub fn new(client: MtsClient) -> Self {
        Self { client }
    }

    async fn pages<T>(
        &self,
        path: &str,
        flat: &FlatId,
        category: Option<&str>,
    ) -> Result<Vec<T>, AppError>
    where
        T: DeserializeOwned,
    {
        validate_scope(&flat.0)?;
        if let Some(category) = category {
            validate_scope(category)?;
        }

        let mut items = Vec::new();
        let mut expected_total_pages = None;
        let mut expected_total_elements = None;
        for number in 0..MAX_PAGES {
            let request_path = page_path(path, flat, category, number);
            let request =
                self.client
                    .authorized(Method::GET, ProviderBase::Gateway, &request_path)?;
            let page: SpringPage<T> = self.client.send_json(request).await?;
            validate_page(&page, number, expected_total_pages, expected_total_elements)?;

            let total_pages = *expected_total_pages.get_or_insert(page.total_pages);
            let total_elements = *expected_total_elements.get_or_insert(page.total_elements);
            if (items.len() as u64).saturating_add(page.content.len() as u64) > MAX_ITEMS
                || (items.len() as u64).saturating_add(page.content.len() as u64) > total_elements
            {
                return Err(schema_error());
            }
            items.extend(page.content);

            if page.last {
                return Ok(items);
            }
            if total_pages == 0 || number + 1 >= total_pages {
                return Err(schema_error());
            }
        }
        Err(schema_error())
    }
}

#[async_trait]
impl DeviceProvider for MtsDeviceProvider {
    async fn access_points(
        &self,
        flat: &FlatId,
        category: Option<&str>,
    ) -> Result<Vec<AccessPoint>, AppError> {
        let dtos = self.pages(ACCESS_POINTS_PATH, flat, category).await?;
        merge_access_points(flat, dtos)
    }

    async fn cameras(&self, flat: &FlatId) -> Result<Vec<Camera>, AppError> {
        let dtos = self.pages(CAMERAS_PATH, flat, None).await?;
        merge_cameras(flat, dtos)
    }
}

fn page_path(path: &str, flat: &FlatId, category: Option<&str>, page: u32) -> String {
    let mut query = form_urlencoded::Serializer::new(String::new());
    query.append_pair("flatId", &flat.0);
    if let Some(category) = category {
        query.append_pair("category", category);
    }
    query.append_pair("page", &page.to_string());
    query.append_pair("size", &PAGE_SIZE.to_string());
    query.append_pair("sort", "name,asc");
    format!("{path}?{}", query.finish())
}

fn validate_scope(value: &str) -> Result<(), AppError> {
    if value.is_empty() || value.len() > MAX_SCOPE_BYTES {
        return Err(AppError::invalid_request(
            "provider device scope is invalid",
        ));
    }
    Ok(())
}

fn validate_page<T>(
    page: &SpringPage<T>,
    expected_number: u32,
    expected_total_pages: Option<u32>,
    expected_total_elements: Option<u64>,
) -> Result<(), AppError> {
    let expected_last = page.total_pages == 0 || page.number + 1 == page.total_pages;
    if page.number != expected_number
        || page.size == 0
        || page.size > PAGE_SIZE
        || page.content.len() > page.size as usize
        || page.total_pages > MAX_PAGES
        || page.total_elements > MAX_ITEMS
        || page.first != (page.number == 0)
        || page.empty != page.content.is_empty()
        || page.last != expected_last
        || expected_total_pages.is_some_and(|total| total != page.total_pages)
        || expected_total_elements.is_some_and(|total| total != page.total_elements)
    {
        return Err(schema_error());
    }
    Ok(())
}

fn merge_access_points(
    flat: &FlatId,
    dtos: Vec<AccessPointDto>,
) -> Result<Vec<AccessPoint>, AppError> {
    let mut merged: BTreeMap<String, AccessPoint> = BTreeMap::new();
    for dto in dtos {
        validate_identity(&dto.id, &dto.name)?;
        let point = map_access_point(flat, dto);
        match merged.entry(point.id.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(point);
            }
            Entry::Occupied(mut entry) => {
                let existing = entry.get_mut();
                if existing.name != point.name {
                    return Err(schema_error());
                }
                existing.available |= point.available;
                existing.locks.extend(point.locks);
                existing.camera_ids.extend(point.camera_ids);
                normalize_locks(&mut existing.locks);
                existing.camera_ids.sort();
                existing.camera_ids.dedup();
            }
        }
    }
    Ok(merged.into_values().collect())
}

fn map_access_point(flat: &FlatId, dto: AccessPointDto) -> AccessPoint {
    let mut locks = parse_locks(&dto.locks);
    normalize_locks(&mut locks);
    let camera_ids = dto
        .camera
        .as_str()
        .filter(|id| !id.trim().is_empty() && id.len() <= MAX_ID_BYTES)
        .map(|id| vec![id.to_owned()])
        .unwrap_or_default();
    AccessPoint {
        id: dto.id,
        flat_id: flat.clone(),
        name: dto.name,
        kind: AccessPointKind::Unknown(UNKNOWN_KIND.to_owned()),
        locks,
        camera_ids,
        available: true,
    }
}

fn parse_locks(value: &Value) -> Vec<Lock> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|lock| {
            let lock = lock.as_object()?;
            let number = lock.get("number")?.as_i64()?.to_string();
            let name = lock.get("name")?.as_str()?;
            if name.len() > MAX_NAME_BYTES {
                return None;
            }
            Some(Lock {
                number,
                name: name.to_owned(),
                permanently_open: lock
                    .get("isPermanentlyOpen")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect()
}

fn normalize_locks(locks: &mut Vec<Lock>) {
    locks.sort_by(|left, right| {
        (&left.number, &left.name, left.permanently_open).cmp(&(
            &right.number,
            &right.name,
            right.permanently_open,
        ))
    });
    locks.dedup();
}

fn merge_cameras(flat: &FlatId, dtos: Vec<CameraDto>) -> Result<Vec<Camera>, AppError> {
    let mut merged: BTreeMap<String, Camera> = BTreeMap::new();
    for dto in dtos {
        validate_identity(&dto.id, &dto.name)?;
        let camera = map_camera(flat, dto);
        match merged.entry(camera.id.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(camera);
            }
            Entry::Occupied(mut entry) => {
                let existing = entry.get_mut();
                if existing.name != camera.name {
                    return Err(schema_error());
                }
                existing.access_point_id =
                    [existing.access_point_id.take(), camera.access_point_id]
                        .into_iter()
                        .flatten()
                        .min();
                merge_capability(&mut existing.capabilities.live, camera.capabilities.live);
                merge_capability(
                    &mut existing.capabilities.archive,
                    camera.capabilities.archive,
                );
            }
        }
    }
    Ok(merged.into_values().collect())
}

fn map_camera(flat: &FlatId, dto: CameraDto) -> Camera {
    let live = has_stream(&dto.streams_to_watch) || has_stream(&dto.streams);
    let archive = dto
        .ivideon
        .as_object()
        .and_then(|ivideon| ivideon.get("depth"))
        .is_some_and(|depth| {
            depth.as_u64().is_some_and(|depth| depth > 0)
                || depth.as_i64().is_some_and(|depth| depth > 0)
        });
    let access_point_id = dto
        .intercoms
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|intercom| intercom.get("id")?.as_str())
        .filter(|id| !id.trim().is_empty() && id.len() <= MAX_ID_BYTES)
        .min()
        .map(str::to_owned);
    Camera {
        id: dto.id,
        flat_id: flat.clone(),
        name: dto.name,
        access_point_id,
        capabilities: CameraCapabilities {
            snapshot: Capability::Available,
            live: capability(live),
            archive: capability(archive),
        },
    }
}

fn has_stream(value: &Value) -> bool {
    value.as_object().is_some_and(|streams| {
        streams
            .values()
            .filter_map(Value::as_str)
            .any(|stream| !stream.trim().is_empty())
    })
}

fn capability(available: bool) -> Capability {
    if available {
        Capability::Available
    } else {
        Capability::Unavailable { reason: None }
    }
}

fn merge_capability(target: &mut Capability, candidate: Capability) {
    if matches!(candidate, Capability::Available) {
        *target = Capability::Available;
    }
}

fn validate_identity(id: &str, name: &str) -> Result<(), AppError> {
    if id.trim().is_empty()
        || id.len() > MAX_ID_BYTES
        || name.trim().is_empty()
        || name.len() > MAX_NAME_BYTES
    {
        return Err(schema_error());
    }
    Ok(())
}

fn schema_error() -> AppError {
    AppError::new(
        ErrorCode::UpstreamSchemaChanged,
        "Provider device response has an unexpected schema.",
        false,
    )
}
