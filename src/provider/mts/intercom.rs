use std::{
    collections::{BTreeMap, BTreeSet, btree_map::Entry},
    sync::Arc,
};

use async_trait::async_trait;
use futures_util::future::join_all;
use reqwest::Method;
use secrecy::{ExposeSecret, SecretString};
use serde::de::DeserializeOwned;
use url::form_urlencoded;

use crate::{
    domain::{AccessPoint, AccessPointKind, Camera, CameraCapabilities, Capability, FlatId, Lock},
    error::{AppError, ErrorCode},
};

use super::{
    MtsClient, ProviderBase,
    dto::intercom::{AccessPointDto, BoundedVec, CameraDto, LockDto, SpringPage},
    ivideon::{IvideonLiveResolver, IvideonLiveUrlResolver},
};

const ACCESS_POINTS_PATH: &str = "/vdome-smart-facade/devices/api/v1/accessPoints";
const AVAILABLE_PATH: &str = "/vdome-smart-facade/devices/api/v1/flats";
const CAMERAS_PATH: &str = "/vdome-smart-facade/devices/api/v1/cameras";
const ACCESS_POINT_CATEGORY: &str = "accessPoint";
const DRIVE_POINT_CATEGORY: &str = "drivePoint";
const PAGE_SIZE: u32 = 100;
const MAX_PAGES: u32 = 50;
const MAX_ITEMS: u64 = 5_000;
const MAX_AVAILABLE_ALIASES: usize = 32;
const MAX_SCOPE_BYTES: usize = 512;
const MAX_ID_BYTES: usize = 512;
const MAX_NAME_BYTES: usize = 2_048;
const MAX_LOCK_NAME_BYTES: usize = 512;
const MAX_LOCKS_PER_ACCESS_POINT: usize = 64;
const MAX_CAMERAS_PER_ACCESS_POINT: usize = 64;
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

#[async_trait]
pub trait LiveSourceProvider: Send + Sync {
    async fn live_source(&self, flat: &FlatId, camera_id: &str) -> Result<SecretString, AppError>;
}

#[derive(Clone)]
pub struct MtsDeviceProvider {
    client: MtsClient,
    ivideon: Arc<dyn IvideonLiveUrlResolver>,
}

impl MtsDeviceProvider {
    pub fn new(client: MtsClient) -> Self {
        Self {
            client,
            ivideon: Arc::new(IvideonLiveResolver::production()),
        }
    }

    #[cfg(feature = "test-support")]
    pub fn new_with_live_url_resolver(
        client: MtsClient,
        ivideon: Arc<dyn IvideonLiveUrlResolver>,
    ) -> Self {
        Self { client, ivideon }
    }

    async fn fold_pages<T, S, F>(
        &self,
        path: &str,
        flat: &FlatId,
        category: Option<&str>,
        max_items: u64,
        mut state: S,
        mut merge: F,
    ) -> Result<(S, u64), AppError>
    where
        T: DeserializeOwned,
        F: FnMut(&mut S, T) -> Result<(), AppError>,
    {
        validate_scope(&flat.0)?;
        if let Some(category) = category {
            validate_scope(category)?;
        }

        let mut raw_count = 0_u64;
        let mut expected_total_pages = None;
        let mut expected_total_elements = None;
        for number in 0..MAX_PAGES {
            let request_path = page_path(path, flat, category, number);
            let request =
                self.client
                    .authorized(Method::GET, ProviderBase::Gateway, &request_path)?;
            let page: SpringPage<T> = self.client.send_json(request).await?;
            validate_page(
                &page,
                number,
                expected_total_pages,
                expected_total_elements,
                max_items,
            )?;

            let total_pages = *expected_total_pages.get_or_insert(page.total_pages);
            let total_elements = *expected_total_elements.get_or_insert(page.total_elements);
            raw_count = raw_count.saturating_add(page.content.len() as u64);
            if raw_count > max_items || raw_count > total_elements {
                return Err(schema_error());
            }
            for item in page.content {
                merge(&mut state, item)?;
            }

            if page.last {
                if raw_count != total_elements {
                    return Err(schema_error());
                }
                return Ok((state, raw_count));
            }
            if total_pages == 0 || number + 1 >= total_pages {
                return Err(schema_error());
            }
        }
        Err(schema_error())
    }

    async fn available_access_categories(
        &self,
        flat: &FlatId,
    ) -> Result<Vec<&'static str>, AppError> {
        validate_scope(&flat.0)?;
        let request = self.client.authorized_with_path_segment(
            Method::GET,
            ProviderBase::Gateway,
            AVAILABLE_PATH,
            &flat.0,
            Some("available"),
        )?;
        let aliases: Option<BoundedVec<String, MAX_AVAILABLE_ALIASES>> =
            self.client.send_json(request).await?;
        let mut available = BTreeSet::new();
        for alias in aliases.map(BoundedVec::into_vec).unwrap_or_default() {
            if alias.len() > MAX_SCOPE_BYTES {
                return Err(schema_error());
            }
            let alias = alias.trim();
            if alias.eq_ignore_ascii_case(ACCESS_POINT_CATEGORY) {
                available.insert(ACCESS_POINT_CATEGORY);
            } else if alias.eq_ignore_ascii_case(DRIVE_POINT_CATEGORY) {
                available.insert(DRIVE_POINT_CATEGORY);
            }
        }
        Ok([ACCESS_POINT_CATEGORY, DRIVE_POINT_CATEGORY]
            .into_iter()
            .filter(|category| available.contains(category))
            .collect())
    }

    async fn fold_access_point_category(
        &self,
        flat: &FlatId,
        category: &str,
        max_items: u64,
    ) -> Result<(BTreeMap<String, AccessPointAccumulator>, u64), AppError> {
        self.fold_pages(
            ACCESS_POINTS_PATH,
            flat,
            Some(category),
            max_items,
            BTreeMap::new(),
            merge_access_point,
        )
        .await
    }
}

#[async_trait]
impl DeviceProvider for MtsDeviceProvider {
    async fn access_points(
        &self,
        flat: &FlatId,
        category: Option<&str>,
    ) -> Result<Vec<AccessPoint>, AppError> {
        let merged = if let Some(category) = category {
            self.fold_access_point_category(flat, category, MAX_ITEMS)
                .await?
                .0
        } else {
            let categories = self.available_access_categories(flat).await?;
            let mut merged = BTreeMap::new();
            let mut remaining_items = MAX_ITEMS;
            let mut successful_categories = 0_usize;
            let mut category_error = None;
            let sources = join_all(
                categories
                    .iter()
                    .map(|category| self.fold_access_point_category(flat, category, MAX_ITEMS)),
            )
            .await;
            for source in sources {
                match source {
                    Ok((_, raw_count)) if raw_count > remaining_items => {
                        category_error.get_or_insert_with(schema_error);
                    }
                    Ok((source, raw_count)) => {
                        let mut candidate = merged.clone();
                        match merge_access_point_map(&mut candidate, source) {
                            Ok(()) => {
                                merged = candidate;
                                remaining_items = remaining_items.saturating_sub(raw_count);
                                successful_categories += 1;
                            }
                            Err(error) => {
                                category_error.get_or_insert(error);
                            }
                        }
                    }
                    Err(error) => {
                        category_error.get_or_insert(error);
                    }
                }
            }
            if successful_categories == 0 && !categories.is_empty() {
                return Err(category_error.unwrap_or_else(schema_error));
            }
            merged
        };
        Ok(merged
            .into_values()
            .map(|point| point.into_access_point(flat))
            .collect())
    }

    async fn cameras(&self, flat: &FlatId) -> Result<Vec<Camera>, AppError> {
        let merged = self
            .fold_pages(
                CAMERAS_PATH,
                flat,
                None,
                MAX_ITEMS,
                BTreeMap::new(),
                |merged, dto| merge_camera(flat, merged, dto),
            )
            .await?
            .0;
        Ok(merged.into_values().collect())
    }
}

#[async_trait]
impl LiveSourceProvider for MtsDeviceProvider {
    async fn live_source(&self, flat: &FlatId, camera_id: &str) -> Result<SecretString, AppError> {
        validate_optional_id(camera_id)?;
        let source = self
            .fold_pages(
                CAMERAS_PATH,
                flat,
                None,
                MAX_ITEMS,
                LiveSourceSelection::default(),
                |selected, dto: CameraDto| {
                    if dto.id != camera_id {
                        return Ok(());
                    }
                    let direct = dto
                        .streams_to_watch
                        .as_ref()
                        .and_then(|streams| streams.selected())
                        .or_else(|| dto.streams.as_ref().and_then(|streams| streams.selected()));
                    selected.merge_direct(direct)?;
                    selected.merge_ivideon(ivideon_candidate(dto.ivideon.as_ref())?)
                },
            )
            .await?
            .0;
        if let Some(ivideon) = source.ivideon {
            return self
                .ivideon
                .resolve_live_url(&ivideon.camera_id, &ivideon.access_token)
                .await;
        }
        source.direct.ok_or_else(|| {
            AppError::new(
                ErrorCode::NotFound,
                "Live camera source was not found.",
                false,
            )
        })
    }
}

#[derive(Default)]
struct LiveSourceSelection {
    ivideon: Option<IvideonCandidate>,
    direct: Option<SecretString>,
}

impl LiveSourceSelection {
    fn merge_ivideon(&mut self, candidate: Option<IvideonCandidate>) -> Result<(), AppError> {
        let Some(candidate) = candidate else {
            return Ok(());
        };
        if let Some(existing) = self.ivideon.as_ref()
            && (existing.camera_id != candidate.camera_id
                || existing.access_token.expose_secret() != candidate.access_token.expose_secret())
        {
            return Err(schema_error());
        }
        self.ivideon.get_or_insert(candidate);
        Ok(())
    }

    fn merge_direct(&mut self, candidate: Option<SecretString>) -> Result<(), AppError> {
        let Some(candidate) = candidate else {
            return Ok(());
        };
        if self
            .direct
            .as_ref()
            .is_some_and(|existing| existing.expose_secret() != candidate.expose_secret())
        {
            return Err(schema_error());
        }
        self.direct.get_or_insert(candidate);
        Ok(())
    }
}

struct IvideonCandidate {
    camera_id: String,
    access_token: SecretString,
}

fn ivideon_candidate(
    evidence: Option<&super::dto::intercom::IvideonEvidence>,
) -> Result<Option<IvideonCandidate>, AppError> {
    let Some(evidence) = evidence else {
        return Ok(None);
    };
    if let Some(access_token) = evidence.token.as_ref() {
        validate_private_value(access_token.expose_secret(), 16 * 1024)?;
    }
    let Some(camera_id) = evidence.camera_id.as_ref() else {
        return Ok(None);
    };
    validate_optional_id(camera_id)?;
    validate_private_value(camera_id, MAX_ID_BYTES)?;
    let Some(access_token) = evidence.token.clone() else {
        return Ok(None);
    };
    Ok(Some(IvideonCandidate {
        camera_id: camera_id.clone(),
        access_token,
    }))
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
    max_items: u64,
) -> Result<(), AppError> {
    let computed_total_pages = if page.total_elements == 0 {
        0
    } else {
        page.total_elements.div_ceil(u64::from(PAGE_SIZE)) as u32
    };
    let next_page_number = page.number.checked_add(1).ok_or_else(schema_error)?;
    let expected_last = page.total_pages == 0 || next_page_number == page.total_pages;
    let expected_content_len = if page.total_pages == 0 {
        0
    } else {
        let consumed = u64::from(page.number) * u64::from(PAGE_SIZE);
        page.total_elements
            .saturating_sub(consumed)
            .min(u64::from(PAGE_SIZE)) as usize
    };
    if page.number != expected_number
        || page.size != PAGE_SIZE
        || page.total_pages > MAX_PAGES
        || page.total_elements > max_items
        || page.total_pages != computed_total_pages
        || (page.total_pages > 0 && page.number >= page.total_pages)
        || page.content.len() != expected_content_len
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

#[derive(Clone)]
struct AccessPointAccumulator {
    id: String,
    name: String,
    locks: BTreeMap<String, Lock>,
    camera_ids: BTreeSet<String>,
}

impl AccessPointAccumulator {
    fn new(id: String, name: String) -> Self {
        Self {
            id,
            name,
            locks: BTreeMap::new(),
            camera_ids: BTreeSet::new(),
        }
    }

    fn merge_locks(&mut self, candidates: Vec<Lock>) -> Result<(), AppError> {
        for candidate in candidates {
            if let Some(existing) = self.locks.get_mut(&candidate.number) {
                if existing.name != candidate.name {
                    return Err(schema_error());
                }
                match (existing.permanently_open, candidate.permanently_open) {
                    (Some(left), Some(right)) if left != right => return Err(schema_error()),
                    (None, Some(value)) => existing.permanently_open = Some(value),
                    _ => {}
                }
            } else {
                if self.locks.len() >= MAX_LOCKS_PER_ACCESS_POINT {
                    return Err(schema_error());
                }
                self.locks.insert(candidate.number.clone(), candidate);
            }
        }
        Ok(())
    }

    fn merge_camera_id(&mut self, candidate: Option<String>) -> Result<(), AppError> {
        let Some(candidate) = candidate else {
            return Ok(());
        };
        validate_optional_id(&candidate)?;
        if !self.camera_ids.contains(&candidate)
            && self.camera_ids.len() >= MAX_CAMERAS_PER_ACCESS_POINT
        {
            return Err(schema_error());
        }
        self.camera_ids.insert(candidate);
        Ok(())
    }

    fn into_access_point(self, flat: &FlatId) -> AccessPoint {
        AccessPoint {
            id: self.id,
            flat_id: flat.clone(),
            name: self.name,
            kind: AccessPointKind::Unknown(UNKNOWN_KIND.to_owned()),
            locks: self.locks.into_values().collect(),
            camera_ids: self.camera_ids.into_iter().collect(),
            available: true,
        }
    }
}

fn merge_access_point(
    merged: &mut BTreeMap<String, AccessPointAccumulator>,
    dto: AccessPointDto,
) -> Result<(), AppError> {
    validate_identity(&dto.id, &dto.name)?;
    let locks = parse_locks(dto.locks)?;
    match merged.entry(dto.id.clone()) {
        Entry::Vacant(entry) => {
            let mut point = AccessPointAccumulator::new(dto.id, dto.name);
            point.merge_locks(locks)?;
            point.merge_camera_id(dto.camera)?;
            entry.insert(point);
        }
        Entry::Occupied(mut entry) => {
            let existing = entry.get_mut();
            if existing.name != dto.name {
                return Err(schema_error());
            }
            existing.merge_locks(locks)?;
            existing.merge_camera_id(dto.camera)?;
        }
    }
    Ok(())
}

fn merge_access_point_map(
    target: &mut BTreeMap<String, AccessPointAccumulator>,
    source: BTreeMap<String, AccessPointAccumulator>,
) -> Result<(), AppError> {
    for (id, source_point) in source {
        match target.entry(id) {
            Entry::Vacant(entry) => {
                entry.insert(source_point);
            }
            Entry::Occupied(mut entry) => {
                let target_point = entry.get_mut();
                if target_point.name != source_point.name {
                    return Err(schema_error());
                }
                target_point.merge_locks(source_point.locks.into_values().collect())?;
                for camera_id in source_point.camera_ids {
                    target_point.merge_camera_id(Some(camera_id))?;
                }
            }
        }
    }
    Ok(())
}

fn parse_locks<const LIMIT: usize>(
    dtos: Option<BoundedVec<LockDto, LIMIT>>,
) -> Result<Vec<Lock>, AppError> {
    let dtos = dtos.map(BoundedVec::into_vec).unwrap_or_default();
    dtos.into_iter()
        .map(|dto| {
            if dto.name.trim().is_empty() || dto.name.len() > MAX_LOCK_NAME_BYTES {
                return Err(schema_error());
            }
            Ok(Lock {
                number: dto.number.to_string(),
                name: dto.name,
                permanently_open: dto.is_permanently_open,
            })
        })
        .collect()
}

fn merge_camera(
    flat: &FlatId,
    merged: &mut BTreeMap<String, Camera>,
    dto: CameraDto,
) -> Result<(), AppError> {
    validate_identity(&dto.id, &dto.name)?;
    let camera = map_camera(flat, dto)?;
    match merged.entry(camera.id.clone()) {
        Entry::Vacant(entry) => {
            entry.insert(camera);
        }
        Entry::Occupied(mut entry) => {
            let existing = entry.get_mut();
            if existing.name != camera.name {
                return Err(schema_error());
            }
            existing.access_point_id = [existing.access_point_id.take(), camera.access_point_id]
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
    Ok(())
}

fn map_camera(flat: &FlatId, dto: CameraDto) -> Result<Camera, AppError> {
    let live = ivideon_candidate(dto.ivideon.as_ref())?.is_some()
        || dto
            .streams_to_watch
            .as_ref()
            .is_some_and(|streams| streams.available())
        || dto
            .streams
            .as_ref()
            .is_some_and(|streams| streams.available());
    let archive = dto
        .ivideon
        .as_ref()
        .and_then(|ivideon| ivideon.depth)
        .is_some_and(|depth| depth > 0);
    let intercoms = dto.intercoms.map(BoundedVec::into_vec).unwrap_or_default();
    let mut access_point_ids = Vec::new();
    for intercom in intercoms {
        if let Some(id) = intercom.id {
            validate_optional_id(&id)?;
            access_point_ids.push(id);
        }
    }
    access_point_ids.sort();
    access_point_ids.dedup();
    let access_point_id = access_point_ids.into_iter().next();
    Ok(Camera {
        id: dto.id,
        flat_id: flat.clone(),
        name: dto.name,
        access_point_id,
        capabilities: CameraCapabilities {
            snapshot: Capability::Available,
            live: capability(live),
            archive: capability(archive),
        },
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

fn validate_optional_id(id: &str) -> Result<(), AppError> {
    if id.trim().is_empty() || id.len() > MAX_ID_BYTES {
        return Err(schema_error());
    }
    Ok(())
}

fn validate_private_value(value: &str, max_bytes: usize) -> Result<(), AppError> {
    if value.trim().is_empty() || value.len() > max_bytes || value.chars().any(char::is_control) {
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
