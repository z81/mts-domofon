use std::sync::Arc;

use crate::{
    auth::AuthCoordinator,
    provider::mts::{
        MtsClient, MtsDashboardProvider, door::MtsDoorOpenProvider, intercom::MtsDeviceProvider,
        media::MtsMediaProvider, sms::MtsLegacySmsAuth,
    },
    service::{
        devices::DeviceService, door::DoorOpenService, media::MediaService,
        property::PropertyService,
    },
    storage::SecretStore,
};

#[derive(Clone, Default)]
pub struct AppState {
    secret_store: Option<Arc<SecretStore>>,
    auth_coordinator: Option<Arc<AuthCoordinator>>,
    mts_client: Option<Arc<MtsClient>>,
    property_service: Option<Arc<PropertyService>>,
    device_service: Option<Arc<DeviceService>>,
    media_service: Option<Arc<MediaService>>,
    door_open_service: Option<Arc<DoorOpenService>>,
    legacy_sms_auth: Option<Arc<MtsLegacySmsAuth>>,
}

impl AppState {
    pub fn with_secret_store(secret_store: SecretStore) -> Self {
        Self {
            secret_store: Some(Arc::new(secret_store)),
            auth_coordinator: None,
            mts_client: None,
            property_service: None,
            device_service: None,
            media_service: None,
            door_open_service: None,
            legacy_sms_auth: None,
        }
    }

    pub fn with_dependencies(
        secret_store: SecretStore,
        auth_coordinator: Arc<AuthCoordinator>,
        mts_client: Arc<MtsClient>,
    ) -> Self {
        let property_service = Arc::new(PropertyService::new(
            Arc::new(MtsDashboardProvider::new((*mts_client).clone())),
            secret_store.clone(),
        ));
        let device_service = Arc::new(DeviceService::new(Arc::new(MtsDeviceProvider::new(
            (*mts_client).clone(),
        ))));
        let media_service = Arc::new(MediaService::new(
            Arc::clone(&device_service),
            Arc::new(MtsMediaProvider::new((*mts_client).clone())),
        ));
        let door_open_service = Arc::new(DoorOpenService::new(
            Arc::clone(&property_service),
            Arc::clone(&device_service),
            Arc::new(MtsDoorOpenProvider::new((*mts_client).clone())),
        ));
        Self {
            secret_store: Some(Arc::new(secret_store)),
            auth_coordinator: Some(auth_coordinator),
            mts_client: Some(mts_client),
            property_service: Some(property_service),
            device_service: Some(device_service),
            media_service: Some(media_service),
            door_open_service: Some(door_open_service),
            legacy_sms_auth: None,
        }
    }

    pub fn with_core_services(
        property_service: Arc<PropertyService>,
        device_service: Arc<DeviceService>,
        media_service: Arc<MediaService>,
    ) -> Self {
        Self {
            property_service: Some(property_service),
            device_service: Some(device_service),
            media_service: Some(media_service),
            ..Self::default()
        }
    }

    pub fn with_door_open_service(mut self, door_open_service: Arc<DoorOpenService>) -> Self {
        self.door_open_service = Some(door_open_service);
        self
    }

    pub fn with_legacy_sms_auth(mut self, legacy_sms_auth: Arc<MtsLegacySmsAuth>) -> Self {
        self.legacy_sms_auth = Some(legacy_sms_auth);
        self
    }

    pub fn secret_store(&self) -> Option<&Arc<SecretStore>> {
        self.secret_store.as_ref()
    }

    pub fn auth_coordinator(&self) -> Option<&Arc<AuthCoordinator>> {
        self.auth_coordinator.as_ref()
    }

    pub fn mts_client(&self) -> Option<&Arc<MtsClient>> {
        self.mts_client.as_ref()
    }

    pub fn property_service(&self) -> Option<&Arc<PropertyService>> {
        self.property_service.as_ref()
    }

    pub fn device_service(&self) -> Option<&Arc<DeviceService>> {
        self.device_service.as_ref()
    }

    pub fn media_service(&self) -> Option<&Arc<MediaService>> {
        self.media_service.as_ref()
    }

    pub fn door_open_service(&self) -> Option<&Arc<DoorOpenService>> {
        self.door_open_service.as_ref()
    }

    pub fn legacy_sms_auth(&self) -> Option<&Arc<MtsLegacySmsAuth>> {
        self.legacy_sms_auth.as_ref()
    }
}
