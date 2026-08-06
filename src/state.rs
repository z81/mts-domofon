use std::sync::Arc;

use crate::storage::SecretStore;

#[derive(Clone, Default)]
pub struct AppState {
    secret_store: Option<Arc<SecretStore>>,
}

impl AppState {
    pub fn with_secret_store(secret_store: SecretStore) -> Self {
        Self {
            secret_store: Some(Arc::new(secret_store)),
        }
    }

    pub fn secret_store(&self) -> Option<&Arc<SecretStore>> {
        self.secret_store.as_ref()
    }
}
