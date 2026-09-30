use std::{collections::BTreeMap, sync::Arc};

use tokio::sync::Mutex;

use crate::{
    domain::{
        CallMode, CallSettings, CallSettingsPatch, CallSettingsStatus, Capability, FlatId,
        PropertyDashboard,
    },
    error::{AppError, ErrorCode},
    provider::mts::{AvailabilitySnapshot, PropertyProvider, ServiceSnapshot},
    storage::SecretStore,
};

pub struct PropertyService {
    provider: Arc<dyn PropertyProvider>,
    store: SecretStore,
    state: Mutex<ServiceState>,
}

#[derive(Default)]
struct ServiceState {
    selection_loaded: bool,
    selected_flat_id: Option<FlatId>,
    call_revision: u64,
    diagnostics: BTreeMap<FlatId, FlatDiagnostics>,
}

#[derive(Default)]
struct FlatDiagnostics {
    availability: Option<Result<AvailabilitySnapshot, ErrorCode>>,
    services: Option<Result<ServiceSnapshot, ErrorCode>>,
}

impl PropertyService {
    pub fn new(provider: Arc<dyn PropertyProvider>, store: SecretStore) -> Self {
        Self {
            provider,
            store,
            state: Mutex::new(ServiceState::default()),
        }
    }

    pub async fn dashboard(&self) -> Result<PropertyDashboard, AppError> {
        let (mut flats, selected_flat_id, call_settings) = {
            let mut state = self.state.lock().await;
            let (flats, call_settings) =
                tokio::join!(self.provider.flats(), self.provider.call_settings());
            let flats = flats?;
            if !state.selection_loaded {
                state.selected_flat_id = self.store.load_selected_flat().await?;
                state.selection_loaded = true;
            }
            if state
                .selected_flat_id
                .as_ref()
                .is_some_and(|selected| !flats.iter().any(|flat| &flat.id == selected))
            {
                self.store.clear_selected_flat().await?;
                state.selected_flat_id = None;
            }
            let call_settings = match call_settings {
                Ok(mode) => CallSettingsStatus::Available(CallSettings {
                    mode,
                    revision: state.call_revision,
                }),
                Err(_) => CallSettingsStatus::Unavailable,
            };
            (flats, state.selected_flat_id.clone(), call_settings)
        };

        let call_settings_available = matches!(call_settings, CallSettingsStatus::Available(_));
        let mut diagnostics = BTreeMap::new();
        for flat in &mut flats {
            let (availability, services) = tokio::join!(
                self.provider.availability(&flat.id),
                self.provider.services(&flat.id)
            );
            if let Ok(services) = &services {
                if services.has_app_call() {
                    flat.capabilities.calls = Capability::Available;
                }
                if services.has_cloud_archive() {
                    flat.capabilities.archive = Capability::Available;
                }
            }
            if call_settings_available {
                flat.capabilities.call_settings = Capability::Available;
            }
            diagnostics.insert(
                flat.id.clone(),
                FlatDiagnostics {
                    availability: Some(availability.map_err(|error| error.code)),
                    services: Some(services.map_err(|error| error.code)),
                },
            );
        }
        retain_diagnostics(&diagnostics);
        self.state.lock().await.diagnostics = diagnostics;

        Ok(PropertyDashboard {
            flats,
            selected_flat_id,
            call_settings,
        })
    }

    pub async fn ensure_linked_flat(&self, flat_id: &FlatId) -> Result<(), AppError> {
        if self
            .provider
            .flats()
            .await?
            .iter()
            .any(|flat| &flat.id == flat_id)
        {
            return Ok(());
        }
        Err(AppError::new(
            ErrorCode::NotFound,
            "Linked flat was not found.",
            false,
        ))
    }

    pub async fn select_flat(&self, flat_id: FlatId) -> Result<(), AppError> {
        let mut state = self.state.lock().await;
        let flats = self.provider.flats().await?;
        if !flats.iter().any(|flat| flat.id == flat_id) {
            return Err(AppError::new(
                ErrorCode::NotFound,
                "Linked flat was not found.",
                false,
            ));
        }
        self.store.save_selected_flat(&flat_id).await?;
        state.selected_flat_id = Some(flat_id);
        state.selection_loaded = true;
        Ok(())
    }

    pub async fn update_call_settings(
        &self,
        patch: CallSettingsPatch,
    ) -> Result<CallSettings, AppError> {
        if matches!(patch.mode, CallMode::Unknown(_)) {
            return Err(AppError::invalid_request(
                "Unknown call settings cannot be written.",
            ));
        }
        let mut state = self.state.lock().await;
        if patch.expected_revision != state.call_revision {
            return Err(AppError::invalid_request(
                "Call settings revision is stale.",
            ));
        }
        self.provider.update_call_settings(&patch.mode).await?;
        state.call_revision = state.call_revision.checked_add(1).ok_or_else(|| {
            AppError::new(
                ErrorCode::Internal,
                "Call settings revision is exhausted.",
                false,
            )
        })?;
        Ok(CallSettings {
            mode: patch.mode,
            revision: state.call_revision,
        })
    }
}

fn retain_diagnostics(diagnostics: &BTreeMap<FlatId, FlatDiagnostics>) {
    for diagnostic in diagnostics.values() {
        if let Some(Ok(availability)) = &diagnostic.availability {
            let _ = availability.retained_value_count();
        }
        if let Some(Ok(services)) = &diagnostic.services {
            let _ = services.retained_value_count();
        }
    }
}
