use std::sync::Arc;

use crate::{
    domain::{FlatId, ProxiedMedia},
    error::AppError,
    provider::mts::media::SnapshotProvider,
};

use super::devices::DeviceService;

pub struct MediaService {
    devices: Arc<DeviceService>,
    snapshots: Arc<dyn SnapshotProvider>,
}

impl MediaService {
    pub fn new(devices: Arc<DeviceService>, snapshots: Arc<dyn SnapshotProvider>) -> Self {
        Self { devices, snapshots }
    }

    pub async fn snapshot(&self, flat: &FlatId, camera_id: &str) -> Result<ProxiedMedia, AppError> {
        let camera = self.devices.camera(flat, camera_id).await?;
        self.snapshots.snapshot(&camera.id).await
    }
}
