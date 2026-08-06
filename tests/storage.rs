use std::path::Path;

use chrono::Utc;
use domru_mts::{
    domain::auth::{PersistedSession, TokenBundle},
    error::ErrorCode,
    storage::SecretStore,
};
use secrecy::{ExposeSecret, SecretString};

fn synthetic_session() -> PersistedSession {
    PersistedSession {
        tokens: TokenBundle {
            access_token: SecretString::from("access-test"),
            refresh_token: SecretString::from("refresh-test"),
            web_sso_access_token: SecretString::from("sso-test"),
            device_token: Some(SecretString::from("device-token-test")),
        },
        authenticated_at: Utc::now(),
    }
}

#[tokio::test]
async fn stored_session_is_encrypted_and_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let store = SecretStore::open(dir.path(), Some([7_u8; 32]))
        .await
        .unwrap();
    let session = synthetic_session();

    store.save_session(&session).await.unwrap();

    let raw = tokio::fs::read(dir.path().join("session.enc"))
        .await
        .unwrap();
    assert!(raw.starts_with(b"VDM1"));
    assert!(!String::from_utf8_lossy(&raw).contains("access-test"));
    assert!(!format!("{session:?}").contains("access-test"));
    let loaded = store.load_session().await.unwrap().unwrap();
    assert_eq!(loaded.authenticated_at, session.authenticated_at);
    assert_eq!(
        loaded.tokens.access_token.expose_secret(),
        session.tokens.access_token.expose_secret()
    );
    assert_eq!(
        loaded.tokens.refresh_token.expose_secret(),
        session.tokens.refresh_token.expose_secret()
    );
    assert_eq!(
        loaded.tokens.web_sso_access_token.expose_secret(),
        session.tokens.web_sso_access_token.expose_secret()
    );
    assert_eq!(
        loaded
            .tokens
            .device_token
            .as_ref()
            .map(ExposeSecret::expose_secret),
        session
            .tokens
            .device_token
            .as_ref()
            .map(ExposeSecret::expose_secret)
    );
}

#[tokio::test]
async fn altered_ciphertext_is_a_storage_failure() {
    let dir = tempfile::tempdir().unwrap();
    let store = SecretStore::open(dir.path(), Some([9_u8; 32]))
        .await
        .unwrap();
    store.save_session(&synthetic_session()).await.unwrap();

    let path = dir.path().join("session.enc");
    let mut raw = tokio::fs::read(&path).await.unwrap();
    *raw.last_mut().unwrap() ^= 1;
    tokio::fs::write(path, raw).await.unwrap();

    let err = store.load_session().await.unwrap_err();
    assert_eq!(err.code, ErrorCode::StorageFailure);
}

#[tokio::test]
async fn clear_session_retains_the_stable_encrypted_device() {
    let dir = tempfile::tempdir().unwrap();
    let store = SecretStore::open(dir.path(), Some([3_u8; 32]))
        .await
        .unwrap();
    store.save_session(&synthetic_session()).await.unwrap();
    store.load_or_create_device().await.unwrap();

    store.clear_session().await.unwrap();

    assert!(!dir.path().join("session.enc").exists());
    assert!(dir.path().join("device.enc").exists());
}

#[tokio::test]
async fn device_identity_is_stable_across_store_opens() {
    let dir = tempfile::tempdir().unwrap();
    let first = SecretStore::open(dir.path(), Some([4_u8; 32]))
        .await
        .unwrap()
        .load_or_create_device()
        .await
        .unwrap();
    let second = SecretStore::open(dir.path(), Some([4_u8; 32]))
        .await
        .unwrap()
        .load_or_create_device()
        .await
        .unwrap();

    assert_eq!(first, second);
}

#[tokio::test]
async fn external_key_does_not_create_a_master_key() {
    let dir = tempfile::tempdir().unwrap();
    SecretStore::open(dir.path(), Some([2_u8; 32]))
        .await
        .unwrap();

    assert!(!dir.path().join("master.key").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn secret_files_are_owner_read_write_only() {
    let dir = tempfile::tempdir().unwrap();
    let store = SecretStore::open(dir.path(), None).await.unwrap();
    store.save_session(&synthetic_session()).await.unwrap();
    store.load_or_create_device().await.unwrap();

    assert_eq!(
        tokio::fs::read(dir.path().join("master.key"))
            .await
            .unwrap()
            .len(),
        32
    );
    for path in ["master.key", "session.enc", "device.enc"] {
        assert_eq!(mode(dir.path().join(path).as_path()), 0o600, "{path}");
    }
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;

    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}
