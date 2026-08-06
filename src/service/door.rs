use std::{collections::HashMap, sync::Arc, time::Duration};

use chrono::{DateTime, TimeDelta, Utc};
use tokio::sync::Mutex;

use crate::{
    domain::{DoorOpenIntentId, DoorOpenIntentView, FlatId},
    error::{AppError, ErrorCode},
    provider::mts::door::DoorOpenProvider,
    service::{devices::DeviceService, property::PropertyService},
};

const DEFAULT_INTENT_TTL: Duration = Duration::from_secs(30);
const DEFAULT_MAX_PENDING_INTENTS: usize = 256;
const MAX_OPAQUE_ID_BYTES: usize = 512;

pub trait DoorOpenClock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

#[derive(Clone, Copy, Debug, Default)]
struct SystemDoorOpenClock;

impl DoorOpenClock for SystemDoorOpenClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

pub struct DoorOpenService {
    properties: Arc<PropertyService>,
    devices: Arc<DeviceService>,
    provider: Arc<dyn DoorOpenProvider>,
    clock: Arc<dyn DoorOpenClock>,
    ttl: TimeDelta,
    max_pending: usize,
    pending: Mutex<HashMap<DoorOpenIntentId, PendingIntent>>,
}

struct PendingIntent {
    access_point_id: String,
    lock_number: String,
    expires_at: DateTime<Utc>,
}

impl DoorOpenService {
    pub fn new(
        properties: Arc<PropertyService>,
        devices: Arc<DeviceService>,
        provider: Arc<dyn DoorOpenProvider>,
    ) -> Self {
        Self::try_new(
            properties,
            devices,
            provider,
            Arc::new(SystemDoorOpenClock),
            DEFAULT_INTENT_TTL,
            DEFAULT_MAX_PENDING_INTENTS,
        )
        .expect("production door-open intent limits are valid")
    }

    #[cfg(feature = "test-support")]
    pub fn new_for_test(
        properties: Arc<PropertyService>,
        devices: Arc<DeviceService>,
        provider: Arc<dyn DoorOpenProvider>,
        clock: Arc<dyn DoorOpenClock>,
        ttl: Duration,
        max_pending: usize,
    ) -> Result<Self, AppError> {
        Self::try_new(properties, devices, provider, clock, ttl, max_pending)
    }

    fn try_new(
        properties: Arc<PropertyService>,
        devices: Arc<DeviceService>,
        provider: Arc<dyn DoorOpenProvider>,
        clock: Arc<dyn DoorOpenClock>,
        ttl: Duration,
        max_pending: usize,
    ) -> Result<Self, AppError> {
        if ttl.is_zero() || max_pending == 0 {
            return Err(AppError::invalid_request("invalid door-open intent limits"));
        }
        let ttl = TimeDelta::from_std(ttl)
            .map_err(|_| AppError::invalid_request("invalid door-open intent lifetime"))?;
        Ok(Self {
            properties,
            devices,
            provider,
            clock,
            ttl,
            max_pending,
            pending: Mutex::new(HashMap::new()),
        })
    }

    pub async fn create_intent(
        &self,
        flat_id: &FlatId,
        access_point_id: &str,
        lock_number: &str,
    ) -> Result<DoorOpenIntentView, AppError> {
        validate_id(&flat_id.0)?;
        validate_id(access_point_id)?;
        validate_id(lock_number)?;
        self.properties.ensure_linked_flat(flat_id).await?;
        self.devices
            .ensure_lock(flat_id, access_point_id, lock_number)
            .await?;

        let now = self.clock.now();
        let expires_at = now.checked_add_signed(self.ttl).ok_or_else(|| {
            AppError::new(
                ErrorCode::Internal,
                "Door-open intent deadline could not be created.",
                false,
            )
        })?;
        let mut pending = self.pending.lock().await;
        pending.retain(|_, intent| intent.expires_at > now);
        if pending.len() >= self.max_pending {
            return Err(capacity_error());
        }
        let intent_id = loop {
            let candidate = DoorOpenIntentId::generate();
            if !pending.contains_key(&candidate) {
                break candidate;
            }
        };
        pending.insert(
            intent_id.clone(),
            PendingIntent {
                access_point_id: access_point_id.to_owned(),
                lock_number: lock_number.to_owned(),
                expires_at,
            },
        );
        Ok(DoorOpenIntentView {
            intent_id,
            expires_at,
        })
    }

    pub async fn confirm(&self, intent_id: &DoorOpenIntentId) -> Result<(), AppError> {
        let now = self.clock.now();
        let intent = {
            let mut pending = self.pending.lock().await;
            pending.retain(|_, intent| intent.expires_at > now);
            pending.remove(intent_id).ok_or_else(intent_not_found)?
        };
        self.provider
            .open(&intent.access_point_id, &intent.lock_number)
            .await
    }
}

fn validate_id(value: &str) -> Result<(), AppError> {
    if value.is_empty()
        || value.len() > MAX_OPAQUE_ID_BYTES
        || matches!(value, "." | "..")
        || value.chars().any(char::is_control)
    {
        return Err(AppError::invalid_request(
            "Invalid door resource identifier.",
        ));
    }
    Ok(())
}

fn intent_not_found() -> AppError {
    AppError::new(
        ErrorCode::DoorOpenIntentNotFound,
        "Door-open confirmation intent was not found.",
        false,
    )
}

fn capacity_error() -> AppError {
    AppError::new(
        ErrorCode::RateLimited,
        "Too many pending door-open confirmation intents.",
        true,
    )
}
