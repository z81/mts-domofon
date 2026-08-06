use std::sync::Arc;

use crate::{auth::AuthCoordinator, provider::mts::MtsClient, storage::SecretStore};

#[derive(Clone, Default)]
pub struct AppState {
    secret_store: Option<Arc<SecretStore>>,
    auth_coordinator: Option<Arc<AuthCoordinator>>,
    mts_client: Option<Arc<MtsClient>>,
}

impl AppState {
    pub fn with_secret_store(secret_store: SecretStore) -> Self {
        Self {
            secret_store: Some(Arc::new(secret_store)),
            auth_coordinator: None,
            mts_client: None,
        }
    }

    pub fn with_dependencies(
        secret_store: SecretStore,
        auth_coordinator: Arc<AuthCoordinator>,
        mts_client: Arc<MtsClient>,
    ) -> Self {
        Self {
            secret_store: Some(Arc::new(secret_store)),
            auth_coordinator: Some(auth_coordinator),
            mts_client: Some(mts_client),
        }
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
}
