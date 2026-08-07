use std::sync::Arc;

use chrono::{DateTime, TimeDelta, Utc};

use crate::{
    domain::{
        Capability, FlatId,
        archive::{ArchiveDistribution, ArchiveInterval},
    },
    error::{AppError, ErrorCode},
    provider::mts::archive::ArchiveProvider,
};

use super::{
    devices::DeviceService,
    live::{LiveSessionManager, LiveSessionView},
    property::PropertyService,
};

const MAX_PLAYBACK_WINDOW: TimeDelta = TimeDelta::minutes(30);

pub struct ArchiveService {
    properties: Arc<PropertyService>,
    devices: Arc<DeviceService>,
    provider: Arc<dyn ArchiveProvider>,
    sessions: Arc<LiveSessionManager>,
}

impl ArchiveService {
    pub fn new(
        properties: Arc<PropertyService>,
        devices: Arc<DeviceService>,
        provider: Arc<dyn ArchiveProvider>,
        sessions: Arc<LiveSessionManager>,
    ) -> Self {
        Self {
            properties,
            devices,
            provider,
            sessions,
        }
    }

    pub async fn distribution(
        &self,
        flat: &FlatId,
        camera_id: &str,
    ) -> Result<ArchiveDistribution, AppError> {
        self.ensure_owned_archive_camera(flat, camera_id).await?;
        let intervals = self.provider.distribution(camera_id).await?;
        Ok(ArchiveDistribution { intervals })
    }

    pub async fn start(
        &self,
        flat: &FlatId,
        camera_id: &str,
        start: DateTime<Utc>,
    ) -> Result<LiveSessionView, AppError> {
        self.ensure_owned_archive_camera(flat, camera_id).await?;
        let intervals = self.provider.distribution(camera_id).await?;
        let interval = containing_interval(&intervals, start).ok_or_else(archive_not_found)?;
        let bounded_end = start
            .checked_add_signed(MAX_PLAYBACK_WINDOW)
            .ok_or_else(archive_not_found)?
            .min(interval.to);
        if bounded_end <= start {
            return Err(archive_not_found());
        }
        let source = self.provider.source(camera_id, start, bounded_end).await?;
        let key = archive_key(flat, camera_id, start, bounded_end);
        self.sessions.start(key, source).await
    }

    async fn ensure_owned_archive_camera(
        &self,
        flat: &FlatId,
        camera_id: &str,
    ) -> Result<(), AppError> {
        self.properties.ensure_linked_flat(flat).await?;
        let camera = self.devices.camera(flat, camera_id).await?;
        if !matches!(camera.capabilities.archive, Capability::Available) {
            return Err(archive_not_found());
        }
        Ok(())
    }
}

fn containing_interval(
    intervals: &[ArchiveInterval],
    start: DateTime<Utc>,
) -> Option<&ArchiveInterval> {
    intervals
        .iter()
        .find(|interval| interval.from <= start && start < interval.to)
}

fn archive_key(flat: &FlatId, camera_id: &str, start: DateTime<Utc>, end: DateTime<Utc>) -> String {
    format!(
        "archive:{:04x}{}{:04x}{}:{}:{}",
        flat.0.len(),
        flat.0,
        camera_id.len(),
        camera_id,
        start.timestamp_millis(),
        end.timestamp_millis()
    )
}

fn archive_not_found() -> AppError {
    AppError::new(
        ErrorCode::NotFound,
        "Archive recording was not found for the requested camera and time.",
        false,
    )
}
