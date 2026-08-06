use std::{
    io::Read,
    path::Path,
    process::{Child, Command, ExitStatus},
    sync::Arc,
    time::{Duration, Instant},
};

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::Utc;
use domru_mts::{
    domain::auth::{PersistedSession, TokenBundle},
    error::ErrorCode,
    state::AppState,
    storage::SecretStore,
};
use secrecy::{ExposeSecret, SecretString};
use tokio::sync::Barrier;

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
async fn app_state_shares_the_production_secret_store_across_clones() {
    let dir = tempfile::tempdir().unwrap();
    let store = SecretStore::open(dir.path(), Some([17_u8; 32]))
        .await
        .unwrap();
    let state = AppState::with_secret_store(store);
    let cloned = state.clone();
    let original_store = state.secret_store().expect("production store is present");
    let cloned_store = cloned.secret_store().expect("cloned store is present");

    assert!(Arc::ptr_eq(original_store, cloned_store));
    original_store
        .save_session(&synthetic_session())
        .await
        .unwrap();
    assert!(cloned_store.load_session().await.unwrap().is_some());
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_opens_publish_one_complete_master_key() {
    const CALLERS: usize = 32;

    let dir = tempfile::tempdir().unwrap();
    let barrier = Arc::new(Barrier::new(CALLERS));
    let mut tasks = Vec::with_capacity(CALLERS);
    for _ in 0..CALLERS {
        let data_dir = dir.path().to_path_buf();
        let barrier = Arc::clone(&barrier);
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            SecretStore::open(data_dir, None).await
        }));
    }

    let mut stores = Vec::with_capacity(CALLERS);
    for task in tasks {
        stores.push(task.await.unwrap().unwrap());
    }
    assert_eq!(
        tokio::fs::read(dir.path().join("master.key"))
            .await
            .unwrap()
            .len(),
        32
    );

    stores[0].save_session(&synthetic_session()).await.unwrap();
    for store in stores {
        assert!(store.load_session().await.unwrap().is_some());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_device_creation_returns_the_persisted_winner() {
    const CALLERS: usize = 32;

    let dir = tempfile::tempdir().unwrap();
    let store = SecretStore::open(dir.path(), Some([6_u8; 32]))
        .await
        .unwrap();
    let barrier = Arc::new(Barrier::new(CALLERS));
    let mut tasks = Vec::with_capacity(CALLERS);
    for _ in 0..CALLERS {
        let store = store.clone();
        let barrier = Arc::clone(&barrier);
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            store.load_or_create_device().await.unwrap()
        }));
    }

    let mut identities = Vec::with_capacity(CALLERS);
    for task in tasks {
        identities.push(task.await.unwrap());
    }
    assert!(identities.iter().all(|identity| identity == &identities[0]));
    assert_eq!(store.load_or_create_device().await.unwrap(), identities[0]);
}

#[tokio::test]
async fn partial_master_key_is_recovered_when_no_encrypted_data_exists() {
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::write(dir.path().join("master.key"), [1_u8, 2, 3])
        .await
        .unwrap();

    let store = SecretStore::open(dir.path(), None).await.unwrap();

    assert_eq!(
        tokio::fs::read(dir.path().join("master.key"))
            .await
            .unwrap()
            .len(),
        32
    );
    store.save_session(&synthetic_session()).await.unwrap();
    assert!(store.load_session().await.unwrap().is_some());
}

#[tokio::test]
async fn partial_master_key_is_not_replaced_when_encrypted_data_exists() {
    let dir = tempfile::tempdir().unwrap();
    let store = SecretStore::open(dir.path(), None).await.unwrap();
    store.save_session(&synthetic_session()).await.unwrap();
    tokio::fs::write(dir.path().join("master.key"), [1_u8, 2, 3])
        .await
        .unwrap();

    let err = SecretStore::open(dir.path(), None)
        .await
        .err()
        .expect("partial key with encrypted data must fail closed");

    assert_eq!(err.code, ErrorCode::StorageFailure);
    assert_eq!(
        tokio::fs::read(dir.path().join("master.key"))
            .await
            .unwrap(),
        [1_u8, 2, 3]
    );
    assert!(dir.path().join("session.enc").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_opens_recover_a_partial_master_key_once() {
    const CALLERS: usize = 16;

    let dir = tempfile::tempdir().unwrap();
    tokio::fs::write(dir.path().join("master.key"), [1_u8, 2, 3])
        .await
        .unwrap();
    let barrier = Arc::new(Barrier::new(CALLERS));
    let mut tasks = Vec::with_capacity(CALLERS);
    for _ in 0..CALLERS {
        let data_dir = dir.path().to_path_buf();
        let barrier = Arc::clone(&barrier);
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            SecretStore::open(data_dir, None).await
        }));
    }

    let mut stores = Vec::with_capacity(CALLERS);
    for task in tasks {
        stores.push(task.await.unwrap().unwrap());
    }
    stores[0].save_session(&synthetic_session()).await.unwrap();
    for store in stores {
        assert!(store.load_session().await.unwrap().is_some());
    }
}

#[tokio::test]
async fn failed_atomic_replace_removes_its_temporary_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = SecretStore::open(dir.path(), Some([8_u8; 32]))
        .await
        .unwrap();
    tokio::fs::create_dir(dir.path().join("session.enc"))
        .await
        .unwrap();

    let err = store.save_session(&synthetic_session()).await.unwrap_err();

    assert_eq!(err.code, ErrorCode::StorageFailure);
    let mut entries = tokio::fs::read_dir(dir.path()).await.unwrap();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        assert!(!name.starts_with(".session.enc."), "leaked {name}");
    }
}

#[tokio::test]
async fn repeated_session_saves_use_distinct_nonces_and_keep_latest_data() {
    let dir = tempfile::tempdir().unwrap();
    let store = SecretStore::open(dir.path(), Some([10_u8; 32]))
        .await
        .unwrap();
    let session = synthetic_session();
    store.save_session(&session).await.unwrap();
    let first_raw = tokio::fs::read(dir.path().join("session.enc"))
        .await
        .unwrap();

    store.save_session(&session).await.unwrap();

    let second_raw = tokio::fs::read(dir.path().join("session.enc"))
        .await
        .unwrap();
    assert_eq!(&first_raw[..4], b"VDM1");
    assert_eq!(&second_raw[..4], b"VDM1");
    assert_ne!(&first_raw[4..16], &second_raw[4..16]);
    assert_eq!(
        store
            .load_session()
            .await
            .unwrap()
            .unwrap()
            .authenticated_at,
        session.authenticated_at
    );
}

#[tokio::test]
async fn session_ciphertext_cannot_be_replayed_as_a_device_file() {
    let dir = tempfile::tempdir().unwrap();
    let key = [12_u8; 32];
    let store = SecretStore::open(dir.path(), Some(key)).await.unwrap();
    let nonce_bytes = [13_u8; 12];
    let nonce = Nonce::try_from(nonce_bytes.as_slice()).unwrap();
    let ciphertext = Aes256Gcm::new_from_slice(&key)
        .unwrap()
        .encrypt(
            &nonce,
            Payload {
                msg: br#"{"uuid":"3de699f0-87cb-4670-a828-cf3954bcfc10"}"#,
                aad: b"domru-mts/session/v1",
            },
        )
        .unwrap();
    let mut replay = b"VDM1".to_vec();
    replay.extend_from_slice(&nonce_bytes);
    replay.extend_from_slice(&ciphertext);
    tokio::fs::write(dir.path().join("device.enc"), replay)
        .await
        .unwrap();

    let err = store.load_or_create_device().await.unwrap_err();

    assert_eq!(err.code, ErrorCode::StorageFailure);
}

#[tokio::test]
async fn external_key_does_not_create_a_master_key() {
    let dir = tempfile::tempdir().unwrap();
    SecretStore::open(dir.path(), Some([2_u8; 32]))
        .await
        .unwrap();

    assert!(!dir.path().join("master.key").exists());
    assert!(!dir.path().join(".master.key.lock").exists());
}

#[tokio::test]
async fn base64_external_key_is_decoded_without_creating_a_master_key() {
    let dir = tempfile::tempdir().unwrap();
    let encoded = STANDARD.encode([14_u8; 32]);
    let store = SecretStore::open_with_encoded_key(dir.path(), Some(&encoded))
        .await
        .unwrap();
    store.save_session(&synthetic_session()).await.unwrap();

    assert!(!dir.path().join("master.key").exists());
    assert!(!dir.path().join(".master.key.lock").exists());
    let reopened = SecretStore::open(dir.path(), Some([14_u8; 32]))
        .await
        .unwrap();
    assert!(reopened.load_session().await.unwrap().is_some());
}

#[tokio::test]
async fn invalid_base64_external_key_is_rejected_without_persisting_it() {
    for encoded in [
        "synthetic-invalid-secret".to_owned(),
        STANDARD.encode([16_u8; 31]),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let err = SecretStore::open_with_encoded_key(dir.path(), Some(&encoded))
            .await
            .err()
            .expect("invalid key must be rejected");

        assert_eq!(err.code, ErrorCode::InvalidRequest);
        assert!(!err.to_string().contains(&encoded));
        assert!(!dir.path().join("master.key").exists());
    }
}

#[test]
fn vdome_secret_key_environment_is_used_by_the_production_constructor() {
    let dir = tempfile::tempdir().unwrap();
    let encoded = STANDARD.encode([15_u8; 32]);
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "open_from_env_child_process"])
        .env("VDOME_SECRET_KEY", encoded)
        .env("VDOME_STORAGE_ENV_CHILD_DIR", dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!dir.path().join("master.key").exists());
    assert!(!dir.path().join(".master.key.lock").exists());

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let store = SecretStore::open(dir.path(), Some([15_u8; 32]))
            .await
            .unwrap();
        assert!(store.load_session().await.unwrap().is_some());
    });
}

#[test]
fn production_startup_rejects_invalid_vdome_secret_key_without_disclosing_it() {
    let dir = tempfile::tempdir().unwrap();
    let invalid_key = "synthetic-invalid-startup-secret";
    let mut child = Command::new(env!("CARGO_BIN_EXE_domru-mts"))
        .env_clear()
        .env("VDOME_BIND", "127.0.0.1:0")
        .env("VDOME_DATA_DIR", dir.path())
        .env("VDOME_SECRET_KEY", invalid_key)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    let Some(status) = wait_for_exit(&mut child, Duration::from_secs(2)) else {
        child.kill().unwrap();
        child.wait().unwrap();
        panic!("production startup did not reject invalid VDOME_SECRET_KEY");
    };
    let mut stderr = String::new();
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();

    assert!(!status.success());
    assert!(!stdout.contains(invalid_key));
    assert!(!stderr.contains(invalid_key));
    assert!(!dir.path().join("master.key").exists());
}

fn wait_for_exit(child: &mut Child, timeout: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return Some(status);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "subprocess helper for VDOME_SECRET_KEY isolation"]
fn open_from_env_child_process() {
    let Some(data_dir) = std::env::var_os("VDOME_STORAGE_ENV_CHILD_DIR") else {
        return;
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let store = SecretStore::open_from_env(data_dir).await.unwrap();
        store.save_session(&synthetic_session()).await.unwrap();
    });
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
    for path in [
        "master.key",
        ".master.key.lock",
        "session.enc",
        "device.enc",
    ] {
        assert_eq!(mode(dir.path().join(path).as_path()), 0o600, "{path}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn reopening_repairs_existing_secret_file_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let store = SecretStore::open(dir.path(), None).await.unwrap();
    store.save_session(&synthetic_session()).await.unwrap();
    store.load_or_create_device().await.unwrap();
    for path in [
        "master.key",
        ".master.key.lock",
        "session.enc",
        "device.enc",
    ] {
        tokio::fs::set_permissions(
            dir.path().join(path),
            std::fs::Permissions::from_mode(0o644),
        )
        .await
        .unwrap();
    }

    SecretStore::open(dir.path(), None).await.unwrap();

    for path in [
        "master.key",
        ".master.key.lock",
        "session.enc",
        "device.enc",
    ] {
        assert_eq!(mode(dir.path().join(path).as_path()), 0o600, "{path}");
    }
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;

    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}
