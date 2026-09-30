use std::{collections::BTreeMap, sync::Arc};

use crate::{
    domain::{AccessPoint, Camera, DeviceInventory, FlatId},
    error::{AppError, ErrorCode},
    provider::mts::intercom::DeviceProvider,
};

pub struct DeviceService {
    provider: Arc<dyn DeviceProvider>,
}

impl DeviceService {
    pub fn new(provider: Arc<dyn DeviceProvider>) -> Self {
        Self { provider }
    }

    pub async fn inventory(&self, flat: &FlatId) -> Result<DeviceInventory, AppError> {
        let (access_points, cameras) = self.sources(flat).await;
        let (mut access_points, mut cameras) = match (access_points, cameras) {
            (Ok(access_points), Ok(cameras)) => (access_points, cameras),
            (Ok(access_points), Err(_)) => (access_points, Vec::new()),
            (Err(_), Ok(cameras)) => (Vec::new(), cameras),
            (Err(error), Err(_)) => return Err(error),
        };

        stamp_and_sort_access_points(flat, &mut access_points);
        stamp_associate_and_sort_cameras(flat, &access_points, &mut cameras);
        Ok(DeviceInventory {
            access_points,
            cameras,
        })
    }

    pub async fn camera(&self, flat: &FlatId, camera_id: &str) -> Result<Camera, AppError> {
        // Camera ownership and capabilities come from the flat-scoped camera inventory.
        // Door discovery is only needed to associate cameras when displaying the full inventory.
        let mut cameras = self.provider.cameras(flat).await?;
        stamp_associate_and_sort_cameras(flat, &[], &mut cameras);
        cameras
            .into_iter()
            .find(|camera| camera.id == camera_id)
            .ok_or_else(|| {
                AppError::new(
                    ErrorCode::NotFound,
                    "Camera was not found for the requested flat.",
                    false,
                )
            })
    }

    pub async fn ensure_lock(
        &self,
        flat: &FlatId,
        access_point_id: &str,
        lock_number: &str,
    ) -> Result<(), AppError> {
        let access_points = self.provider.access_points(flat, None).await?;
        if access_points.iter().any(|point| {
            point.available
                && point.id == access_point_id
                && point.locks.iter().any(|lock| lock.number == lock_number)
        }) {
            return Ok(());
        }
        Err(AppError::new(
            ErrorCode::NotFound,
            "Door lock was not found for the requested flat.",
            false,
        ))
    }

    async fn sources(
        &self,
        flat: &FlatId,
    ) -> (
        Result<Vec<AccessPoint>, AppError>,
        Result<Vec<Camera>, AppError>,
    ) {
        tokio::join!(
            self.provider.access_points(flat, None),
            self.provider.cameras(flat)
        )
    }
}

fn stamp_and_sort_access_points(flat: &FlatId, access_points: &mut [AccessPoint]) {
    for point in &mut *access_points {
        point.flat_id = flat.clone();
        point.locks.sort_by(|left, right| {
            (&left.number, &left.name, left.permanently_open).cmp(&(
                &right.number,
                &right.name,
                right.permanently_open,
            ))
        });
        point.camera_ids.sort();
        point.camera_ids.dedup();
    }
    access_points.sort_by(|left, right| left.id.cmp(&right.id));
}

fn stamp_associate_and_sort_cameras(
    flat: &FlatId,
    access_points: &[AccessPoint],
    cameras: &mut [Camera],
) {
    let mut associations: BTreeMap<&str, &str> = BTreeMap::new();
    for point in access_points {
        for camera_id in &point.camera_ids {
            associations
                .entry(camera_id)
                .and_modify(|point_id| *point_id = (*point_id).min(point.id.as_str()))
                .or_insert(&point.id);
        }
    }
    for camera in &mut *cameras {
        camera.flat_id = flat.clone();
        camera.access_point_id = [
            camera.access_point_id.take(),
            associations
                .get(camera.id.as_str())
                .map(|id| (*id).to_owned()),
        ]
        .into_iter()
        .flatten()
        .min();
    }
    cameras.sort_by(|left, right| left.id.cmp(&right.id));
}
