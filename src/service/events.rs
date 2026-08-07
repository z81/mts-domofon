use std::{collections::BTreeSet, sync::Arc};

use chrono::TimeDelta;

use crate::{
    domain::event::{CallOutcome, EventItem, EventKind, EventPage, EventQuery},
    error::AppError,
    provider::mts::events::{EventProvider, ProviderEvent, ProviderEventQuery},
};

use super::{devices::DeviceService, property::PropertyService};

const MAX_LOCAL_PAGE: u32 = 20;
const MAX_LOCAL_PAGE_SIZE: u32 = 50;
const PROVIDER_PAGE_SIZE: u32 = 100;
const MAX_PROVIDER_SCAN_PAGES: u32 = 20;
const MAX_EVENT_RANGE: TimeDelta = TimeDelta::days(31);
const MAX_EVENT_DURATION_SECONDS: i64 = 24 * 60 * 60;

pub struct EventService {
    properties: Arc<PropertyService>,
    devices: Arc<DeviceService>,
    provider: Arc<dyn EventProvider>,
}

impl EventService {
    pub fn new(
        properties: Arc<PropertyService>,
        devices: Arc<DeviceService>,
        provider: Arc<dyn EventProvider>,
    ) -> Self {
        Self {
            properties,
            devices,
            provider,
        }
    }

    pub async fn events(&self, query: EventQuery) -> Result<EventPage, AppError> {
        validate_query(&query)?;
        self.properties.ensure_linked_flat(&query.flat_id).await?;
        let inventory = self.devices.inventory(&query.flat_id).await?;
        let mut owned = BTreeSet::from([query.flat_id.0.clone()]);
        let mut owned_cameras = BTreeSet::new();
        for point in inventory.access_points {
            owned.insert(point.id);
            owned.extend(point.camera_ids);
        }
        for camera in inventory.cameras {
            owned.insert(camera.id.clone());
            owned_cameras.insert(camera.id);
        }

        let skip = usize::try_from(query.page)
            .ok()
            .and_then(|page| page.checked_mul(query.size as usize))
            .ok_or_else(|| AppError::invalid_request("invalid event page"))?;
        let take = query.size as usize;
        let mut matched = 0_usize;
        let mut items = Vec::with_capacity(take.saturating_add(1));
        let mut seen = BTreeSet::new();
        let mut provider_exhausted = false;

        for provider_page in 0..MAX_PROVIDER_SCAN_PAGES {
            let page = self
                .provider
                .page(ProviderEventQuery {
                    page: provider_page,
                    size: PROVIDER_PAGE_SIZE,
                    from: query.from,
                    to: query.to,
                })
                .await?;
            for event in page.events {
                if !seen.insert(event.id.clone()) || !is_owned(&event, &owned) {
                    continue;
                }
                let item = normalize_event(event, &owned_cameras);
                if query.kind.is_some_and(|kind| kind != item.kind) {
                    continue;
                }
                if matched < skip {
                    matched += 1;
                    continue;
                }
                items.push(item);
                if items.len() > take {
                    break;
                }
            }
            if items.len() > take {
                break;
            }
            if page.last {
                provider_exhausted = true;
                break;
            }
        }

        let extra_item = items.len() > take;
        items.truncate(take);
        Ok(EventPage {
            items,
            page: query.page,
            has_more: extra_item || !provider_exhausted,
        })
    }
}

fn validate_query(query: &EventQuery) -> Result<(), AppError> {
    let range = query.to.signed_duration_since(query.from);
    if query.page > MAX_LOCAL_PAGE
        || query.size == 0
        || query.size > MAX_LOCAL_PAGE_SIZE
        || range < TimeDelta::zero()
        || range > MAX_EVENT_RANGE
    {
        return Err(AppError::invalid_request("invalid event query"));
    }
    Ok(())
}

fn is_owned(event: &ProviderEvent, owned: &BTreeSet<String>) -> bool {
    event
        .identity_ids
        .iter()
        .any(|identity| owned.contains(identity))
}

fn normalize_event(event: ProviderEvent, owned_cameras: &BTreeSet<String>) -> EventItem {
    let camera_id = event
        .camera_id
        .as_ref()
        .filter(|camera_id| owned_cameras.contains(*camera_id));
    let alias = event.alias.as_deref().unwrap_or_default();
    let kind = if alias == "intercom.incoming.call" {
        EventKind::Call
    } else if alias == "intercom.unlocked" || event.unlocked {
        EventKind::Access
    } else if camera_id.is_some() || alias.starts_with("camera.") {
        EventKind::Camera
    } else {
        EventKind::Other
    };
    let call_outcome = (kind == EventKind::Call).then(|| call_outcome(&event));
    let title = match (kind, call_outcome) {
        (EventKind::Call, Some(CallOutcome::Accepted)) => "Принятый вызов",
        (EventKind::Call, Some(CallOutcome::Missed)) => "Пропущенный вызов",
        (EventKind::Call, Some(CallOutcome::Rejected)) => "Отклонённый вызов",
        (EventKind::Call, Some(CallOutcome::Ignored)) => "Проигнорированный вызов",
        (EventKind::Call, _) => "Вызов домофона",
        (EventKind::Access, _) => "Дверь открыта",
        (EventKind::Camera, _) => "Событие камеры",
        (EventKind::Other, _) => "Событие домофона",
    }
    .to_owned();
    let duration_seconds = event
        .started_at
        .zip(event.finished_at)
        .map(|(started, finished)| finished.signed_duration_since(started).num_seconds())
        .filter(|seconds| (0..=MAX_EVENT_DURATION_SECONDS).contains(seconds))
        .map(|seconds| seconds as u64);
    let camera_id = camera_id.cloned();
    let archive_at = camera_id.as_ref().map(|_| event.occurred_at);

    EventItem {
        id: event.id,
        occurred_at: event.occurred_at,
        kind,
        title,
        source_label: event.source_label,
        call_outcome,
        duration_seconds,
        camera_id,
        archive_at,
    }
}

fn call_outcome(event: &ProviderEvent) -> CallOutcome {
    let status = event.status.as_deref().unwrap_or_default();
    if event.accepted || matches!(status, "answered" | "accepted") {
        CallOutcome::Accepted
    } else if event.missed || status == "missed" {
        CallOutcome::Missed
    } else if event.rejected || matches!(status, "cancelled" | "rejected" | "blocked") {
        CallOutcome::Rejected
    } else if event.ignored || status == "ignored" {
        CallOutcome::Ignored
    } else {
        CallOutcome::Unknown
    }
}
