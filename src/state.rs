use std::sync::Arc;

use crate::{
    auth::AuthCoordinator,
    provider::mts::{
        MtsClient, MtsDashboardProvider, archive::MtsArchiveProvider, door::MtsDoorOpenProvider,
        events::MtsEventProvider, intercom::LiveSourceProvider, intercom::MtsDeviceProvider,
        media::MtsMediaProvider, sms::MtsLegacySmsAuth,
    },
    service::{
        archive::ArchiveService, auto_open::AutoOpenService, calls::CallService,
        devices::DeviceService, door::DoorOpenService, events::EventService,
        live::LiveSessionManager, media::MediaService, property::PropertyService,
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
    live_source_provider: Option<Arc<dyn LiveSourceProvider>>,
    live_session_manager: Option<Arc<LiveSessionManager>>,
    door_open_service: Option<Arc<DoorOpenService>>,
    event_service: Option<Arc<EventService>>,
    archive_service: Option<Arc<ArchiveService>>,
    legacy_sms_auth: Option<Arc<MtsLegacySmsAuth>>,
    call_service: Option<Arc<CallService>>,
    auto_open_service: Option<Arc<AutoOpenService>>,
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
            live_source_provider: None,
            live_session_manager: None,
            door_open_service: None,
            event_service: None,
            archive_service: None,
            legacy_sms_auth: None,
            call_service: None,
            auto_open_service: None,
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
        let mts_devices = Arc::new(MtsDeviceProvider::new((*mts_client).clone()));
        let device_service = Arc::new(DeviceService::new(mts_devices.clone()));
        let media_service = Arc::new(MediaService::new(
            Arc::clone(&device_service),
            Arc::new(MtsMediaProvider::new((*mts_client).clone())),
        ));
        let door_open_service = Arc::new(DoorOpenService::new(
            Arc::clone(&property_service),
            Arc::clone(&device_service),
            Arc::new(MtsDoorOpenProvider::new((*mts_client).clone())),
        ));
        let event_service = Arc::new(EventService::new(
            Arc::clone(&property_service),
            Arc::clone(&device_service),
            Arc::new(MtsEventProvider::new((*mts_client).clone())),
        ));
        let live_session_manager = Arc::new(LiveSessionManager::production());
        let archive_service = Arc::new(ArchiveService::new(
            Arc::clone(&property_service),
            Arc::clone(&device_service),
            Arc::new(MtsArchiveProvider::new((*mts_client).clone())),
            Arc::clone(&live_session_manager),
        ));
        Self {
            secret_store: Some(Arc::new(secret_store)),
            auth_coordinator: Some(auth_coordinator),
            mts_client: Some(mts_client),
            property_service: Some(property_service),
            device_service: Some(device_service),
            media_service: Some(media_service),
            live_source_provider: Some(mts_devices),
            live_session_manager: Some(live_session_manager),
            door_open_service: Some(door_open_service),
            event_service: Some(event_service),
            archive_service: Some(archive_service),
            legacy_sms_auth: None,
            call_service: None,
            auto_open_service: None,
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

    #[cfg(feature = "test-support")]
    pub fn with_intercom_video_services(
        property_service: Arc<PropertyService>,
        device_service: Arc<DeviceService>,
        live_source_provider: Arc<dyn LiveSourceProvider>,
        live_session_manager: Arc<LiveSessionManager>,
    ) -> Self {
        Self {
            property_service: Some(property_service),
            device_service: Some(device_service),
            live_source_provider: Some(live_source_provider),
            live_session_manager: Some(live_session_manager),
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

    pub fn with_event_service(mut self, event_service: Arc<EventService>) -> Self {
        self.event_service = Some(event_service);
        self
    }

    pub fn with_archive_service(mut self, archive_service: Arc<ArchiveService>) -> Self {
        self.archive_service = Some(archive_service);
        self
    }

    pub fn with_call_service(mut self, call_service: Arc<CallService>) -> Self {
        self.call_service = Some(call_service);
        self
    }

    pub fn with_auto_open_service(mut self, auto_open_service: Arc<AutoOpenService>) -> Self {
        self.auto_open_service = Some(auto_open_service);
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

    pub fn live_source_provider(&self) -> Option<&Arc<dyn LiveSourceProvider>> {
        self.live_source_provider.as_ref()
    }

    pub fn live_session_manager(&self) -> Option<&Arc<LiveSessionManager>> {
        self.live_session_manager.as_ref()
    }

    pub fn door_open_service(&self) -> Option<&Arc<DoorOpenService>> {
        self.door_open_service.as_ref()
    }

    pub fn event_service(&self) -> Option<&Arc<EventService>> {
        self.event_service.as_ref()
    }

    pub fn archive_service(&self) -> Option<&Arc<ArchiveService>> {
        self.archive_service.as_ref()
    }

    pub fn legacy_sms_auth(&self) -> Option<&Arc<MtsLegacySmsAuth>> {
        self.legacy_sms_auth.as_ref()
    }

    pub fn call_service(&self) -> Option<&Arc<CallService>> {
        self.call_service.as_ref()
    }

    pub fn auto_open_service(&self) -> Option<&Arc<AutoOpenService>> {
        self.auto_open_service.as_ref()
    }
}
