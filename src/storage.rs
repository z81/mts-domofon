use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use fs2::FileExt;
use rand::RngCore;
use secrecy::ExposeSecret;
use tokio::{fs, io::AsyncWriteExt};
use zeroize::Zeroizing;

use crate::{
    domain::FlatId,
    domain::auth::{AuthFlavor, DeviceIdentity, PersistedSession, TokenBundle},
    error::{AppError, ErrorCode},
};

#[cfg(not(unix))]
compile_error!("encrypted atomic storage currently supports Unix targets only");

const MAGIC: &[u8; 4] = b"VDM1";
const NONCE_LEN: usize = 12;
const SESSION_FILE: &str = "session.enc";
const DEVICE_FILE: &str = "device.enc";
const SELECTED_FLAT_FILE: &str = "selected-flat.enc";
const MASTER_KEY_FILE: &str = "master.key";
const SESSION_AAD: &[u8] = b"domru-mts/session/v1";
const DEVICE_AAD: &[u8] = b"domru-mts/device/v1";
const SELECTED_FLAT_AAD: &[u8] = b"domru-mts/selected-flat/v1";

/// Encrypted durable storage for the currently supported Linux and macOS targets.
#[derive(Clone)]
pub struct SecretStore {
    data_dir: PathBuf,
    key: Zeroizing<[u8; 32]>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum PublishOutcome {
    Published,
    AlreadyExists,
}

struct TemporaryFile {
    path: Option<PathBuf>,
}

struct MasterKeyLock(std::fs::File);

impl Drop for MasterKeyLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

impl TemporaryFile {
    fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    fn path(&self) -> &Path {
        self.path.as_deref().expect("temporary file is active")
    }

    fn disarm(&mut self) {
        self.path = None;
    }

    async fn cleanup(&mut self) -> Result<(), AppError> {
        let Some(path) = self.path.take() else {
            return Ok(());
        };
        match fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(_) => {
                self.path = Some(path);
                Err(storage_failure())
            }
        }
    }
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StoredSession {
    access_token: Zeroizing<String>,
    refresh_token: Zeroizing<String>,
    #[serde(default)]
    web_sso_access_token: Option<Zeroizing<String>>,
    device_token: Option<Zeroizing<String>>,
    #[serde(default)]
    auth_flavor: Option<AuthFlavor>,
    authenticated_at: chrono::DateTime<chrono::Utc>,
}

impl From<&PersistedSession> for StoredSession {
    fn from(session: &PersistedSession) -> Self {
        Self {
            access_token: Zeroizing::new(session.tokens.access_token.expose_secret().to_owned()),
            refresh_token: Zeroizing::new(session.tokens.refresh_token.expose_secret().to_owned()),
            web_sso_access_token: session
                .tokens
                .web_sso_access_token
                .as_ref()
                .map(|token| Zeroizing::new(token.expose_secret().to_owned())),
            device_token: session
                .tokens
                .device_token
                .as_ref()
                .map(|token| Zeroizing::new(token.expose_secret().to_owned())),
            auth_flavor: Some(session.auth_flavor),
            authenticated_at: session.authenticated_at,
        }
    }
}

impl From<StoredSession> for PersistedSession {
    fn from(session: StoredSession) -> Self {
        let auth_flavor = session.auth_flavor.unwrap_or_else(|| {
            if session.web_sso_access_token.is_some() {
                AuthFlavor::WebSso
            } else {
                AuthFlavor::LegacySms
            }
        });
        Self {
            tokens: TokenBundle {
                access_token: secrecy::SecretString::from(session.access_token.to_string()),
                refresh_token: secrecy::SecretString::from(session.refresh_token.to_string()),
                web_sso_access_token: session
                    .web_sso_access_token
                    .map(|token| secrecy::SecretString::from(token.to_string())),
                device_token: session
                    .device_token
                    .map(|token| secrecy::SecretString::from(token.to_string())),
            },
            auth_flavor,
            authenticated_at: session.authenticated_at,
        }
    }
}

impl SecretStore {
    /// Opens production storage using the optional Base64-encoded
    /// `VDOME_SECRET_KEY`, falling back to `master.key` when it is absent.
    pub async fn open_from_env(data_dir: impl AsRef<Path>) -> Result<Self, AppError> {
        use std::os::unix::ffi::OsStringExt;

        let external_key = std::env::var_os("VDOME_SECRET_KEY")
            .map(|encoded| {
                let encoded = Zeroizing::new(encoded.into_vec());
                let encoded = std::str::from_utf8(&encoded).map_err(|_| invalid_external_key())?;
                decode_external_key(encoded)
            })
            .transpose()?;
        Self::open_with_key(data_dir, external_key).await
    }

    /// Opens storage with an explicitly supplied Base64 key source.
    pub async fn open_with_encoded_key(
        data_dir: impl AsRef<Path>,
        encoded_key: Option<&str>,
    ) -> Result<Self, AppError> {
        let external_key = encoded_key.map(decode_external_key).transpose()?;
        Self::open_with_key(data_dir, external_key).await
    }

    pub async fn open(
        data_dir: impl AsRef<Path>,
        external_key: Option<[u8; 32]>,
    ) -> Result<Self, AppError> {
        Self::open_with_key(data_dir, external_key.map(Zeroizing::new)).await
    }

    async fn open_with_key(
        data_dir: impl AsRef<Path>,
        external_key: Option<Zeroizing<[u8; 32]>>,
    ) -> Result<Self, AppError> {
        let data_dir = data_dir.as_ref().to_path_buf();
        fs::create_dir_all(&data_dir)
            .await
            .map_err(|_| storage_failure())?;
        repair_existing_secret_permissions(&data_dir).await?;
        let key = match external_key {
            Some(key) => key,
            None => load_or_create_master_key(&data_dir).await?,
        };

        Ok(Self { data_dir, key })
    }

    pub async fn load_session(&self) -> Result<Option<PersistedSession>, AppError> {
        let Some(plaintext) = self.read_encrypted(SESSION_FILE, SESSION_AAD).await? else {
            return Ok(None);
        };
        let stored =
            serde_json::from_slice::<StoredSession>(&plaintext).map_err(|_| storage_failure())?;
        Ok(Some(stored.into()))
    }

    pub async fn save_session(&self, session: &PersistedSession) -> Result<(), AppError> {
        let stored = StoredSession::from(session);
        let plaintext = Zeroizing::new(serde_json::to_vec(&stored).map_err(|_| storage_failure())?);
        let encrypted = self.encrypt(&plaintext, SESSION_AAD)?;
        self.atomic_replace(SESSION_FILE, &encrypted).await
    }

    pub async fn clear_session(&self) -> Result<(), AppError> {
        let path = self.data_dir.join(SESSION_FILE);
        match fs::remove_file(path).await {
            Ok(()) => sync_parent(&self.data_dir).await,
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(_) => Err(storage_failure()),
        }
    }

    pub async fn load_selected_flat(&self) -> Result<Option<FlatId>, AppError> {
        let Some(plaintext) = self
            .read_encrypted(SELECTED_FLAT_FILE, SELECTED_FLAT_AAD)
            .await?
        else {
            return Ok(None);
        };
        serde_json::from_slice(&plaintext)
            .map(Some)
            .map_err(|_| storage_failure())
    }

    pub async fn save_selected_flat(&self, flat_id: &FlatId) -> Result<(), AppError> {
        let plaintext = Zeroizing::new(serde_json::to_vec(flat_id).map_err(|_| storage_failure())?);
        let encrypted = self.encrypt(&plaintext, SELECTED_FLAT_AAD)?;
        self.atomic_replace(SELECTED_FLAT_FILE, &encrypted).await
    }

    pub async fn clear_selected_flat(&self) -> Result<(), AppError> {
        let path = self.data_dir.join(SELECTED_FLAT_FILE);
        match fs::remove_file(path).await {
            Ok(()) => sync_parent(&self.data_dir).await,
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(_) => Err(storage_failure()),
        }
    }

    pub async fn load_or_create_device(&self) -> Result<DeviceIdentity, AppError> {
        if let Some(plaintext) = self.read_encrypted(DEVICE_FILE, DEVICE_AAD).await? {
            return serde_json::from_slice(&plaintext).map_err(|_| storage_failure());
        }

        let device = DeviceIdentity {
            uuid: uuid::Uuid::new_v4(),
        };
        let plaintext = serde_json::to_vec(&device).map_err(|_| storage_failure())?;
        let encrypted = self.encrypt(&plaintext, DEVICE_AAD)?;
        match self
            .atomic_publish_if_absent(DEVICE_FILE, &encrypted)
            .await?
        {
            PublishOutcome::Published => Ok(device),
            PublishOutcome::AlreadyExists => {
                let plaintext = self
                    .read_encrypted(DEVICE_FILE, DEVICE_AAD)
                    .await?
                    .ok_or_else(storage_failure)?;
                serde_json::from_slice(&plaintext).map_err(|_| storage_failure())
            }
        }
    }

    async fn read_encrypted(
        &self,
        file_name: &str,
        aad: &[u8],
    ) -> Result<Option<Zeroizing<Vec<u8>>>, AppError> {
        let path = self.data_dir.join(file_name);
        if !repair_secret_permissions_if_exists(&path).await? {
            return Ok(None);
        }
        let encrypted = match fs::read(path).await {
            Ok(value) => value,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(storage_failure()),
        };
        self.decrypt(&encrypted, aad).map(Some)
    }

    fn encrypt(&self, plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>, AppError> {
        let cipher = Aes256Gcm::new_from_slice(self.key.as_ref()).map_err(|_| storage_failure())?;
        let mut nonce = [0_u8; NONCE_LEN];
        rand::rng().fill_bytes(&mut nonce);
        let nonce = Nonce::try_from(nonce.as_slice()).map_err(|_| storage_failure())?;
        let ciphertext = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|_| storage_failure())?;

        let mut encoded = Vec::with_capacity(MAGIC.len() + NONCE_LEN + ciphertext.len());
        encoded.extend_from_slice(MAGIC);
        encoded.extend_from_slice(nonce.as_slice());
        encoded.extend_from_slice(&ciphertext);
        Ok(encoded)
    }

    fn decrypt(&self, encrypted: &[u8], aad: &[u8]) -> Result<Zeroizing<Vec<u8>>, AppError> {
        if encrypted.len() < MAGIC.len() + NONCE_LEN || !encrypted.starts_with(MAGIC) {
            return Err(storage_failure());
        }

        let cipher = Aes256Gcm::new_from_slice(self.key.as_ref()).map_err(|_| storage_failure())?;
        let nonce = Nonce::try_from(&encrypted[MAGIC.len()..MAGIC.len() + NONCE_LEN])
            .map_err(|_| storage_failure())?;
        cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &encrypted[MAGIC.len() + NONCE_LEN..],
                    aad,
                },
            )
            .map(Zeroizing::new)
            .map_err(|_| storage_failure())
    }

    async fn atomic_replace(&self, file_name: &str, contents: &[u8]) -> Result<(), AppError> {
        atomic_replace_file(&self.data_dir, file_name, contents).await
    }

    async fn atomic_publish_if_absent(
        &self,
        file_name: &str,
        contents: &[u8],
    ) -> Result<PublishOutcome, AppError> {
        publish_file_if_absent(&self.data_dir, file_name, contents).await
    }
}

fn decode_external_key(encoded: &str) -> Result<Zeroizing<[u8; 32]>, AppError> {
    let mut key = Zeroizing::new([0_u8; 32]);
    let decoded_len = STANDARD
        .decode_slice(encoded.as_bytes(), key.as_mut())
        .map_err(|_| invalid_external_key())?;
    if decoded_len != key.len() {
        return Err(invalid_external_key());
    }
    Ok(key)
}

async fn open_owner_only_temporary_file(
    path: PathBuf,
) -> Result<(fs::File, TemporaryFile), AppError> {
    let (file, temporary) = tokio::task::spawn_blocking(move || {
        use std::os::unix::fs::OpenOptionsExt;

        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        Ok::<_, std::io::Error>((file, TemporaryFile::new(path)))
    })
    .await
    .map_err(|_| storage_failure())?
    .map_err(|_| storage_failure())?;
    Ok((fs::File::from_std(file), temporary))
}

async fn create_synced_temporary_file(
    data_dir: &Path,
    file_name: &str,
    contents: &[u8],
) -> Result<TemporaryFile, AppError> {
    let temporary = data_dir.join(format!(".{file_name}.{}.tmp", uuid::Uuid::new_v4()));
    let (mut file, temporary) = open_owner_only_temporary_file(temporary).await?;

    set_owner_only_permissions(temporary.path()).await?;
    file.write_all(contents)
        .await
        .map_err(|_| storage_failure())?;
    file.sync_all().await.map_err(|_| storage_failure())?;
    drop(file);
    Ok(temporary)
}

async fn atomic_replace_file(
    data_dir: &Path,
    file_name: &str,
    contents: &[u8],
) -> Result<(), AppError> {
    let destination = data_dir.join(file_name);
    let mut temporary = create_synced_temporary_file(data_dir, file_name, contents).await?;
    fs::rename(temporary.path(), destination)
        .await
        .map_err(|_| storage_failure())?;
    temporary.disarm();
    sync_parent(data_dir).await
}

async fn publish_file_if_absent(
    data_dir: &Path,
    file_name: &str,
    contents: &[u8],
) -> Result<PublishOutcome, AppError> {
    let destination = data_dir.join(file_name);
    let mut temporary = create_synced_temporary_file(data_dir, file_name, contents).await?;
    let outcome = match fs::hard_link(temporary.path(), destination).await {
        Ok(()) => PublishOutcome::Published,
        Err(error) if error.kind() == ErrorKind::AlreadyExists => PublishOutcome::AlreadyExists,
        Err(_) => return Err(storage_failure()),
    };
    temporary.cleanup().await?;
    sync_parent(data_dir).await?;
    Ok(outcome)
}

async fn load_or_create_master_key(data_dir: &Path) -> Result<Zeroizing<[u8; 32]>, AppError> {
    let _master_key_lock = acquire_master_key_lock(data_dir).await?;
    let path = data_dir.join(MASTER_KEY_FILE);
    loop {
        if repair_secret_permissions_if_exists(&path).await? {
            let bytes = match fs::read(&path).await {
                Ok(bytes) => Zeroizing::new(bytes),
                Err(error) if error.kind() == ErrorKind::NotFound => continue,
                Err(_) => return Err(storage_failure()),
            };
            if bytes.len() == 32 {
                let mut key = Zeroizing::new([0_u8; 32]);
                key.copy_from_slice(&bytes);
                return Ok(key);
            }

            recover_partial_master_key(data_dir, &path).await?;
            continue;
        }

        let mut candidate = Zeroizing::new([0_u8; 32]);
        rand::rng().fill_bytes(candidate.as_mut());
        match publish_file_if_absent(data_dir, MASTER_KEY_FILE, candidate.as_ref()).await? {
            PublishOutcome::Published => return Ok(candidate),
            PublishOutcome::AlreadyExists => continue,
        }
    }
}

async fn acquire_master_key_lock(data_dir: &Path) -> Result<MasterKeyLock, AppError> {
    let path = data_dir.join(".master.key.lock");
    let file = tokio::task::spawn_blocking(move || {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)?;
        FileExt::lock_exclusive(&file)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.sync_all()?;
        Ok::<_, std::io::Error>(file)
    })
    .await
    .map_err(|_| storage_failure())?
    .map_err(|_| storage_failure())?;
    sync_parent(data_dir).await?;
    Ok(MasterKeyLock(file))
}

async fn recover_partial_master_key(data_dir: &Path, path: &Path) -> Result<(), AppError> {
    if encrypted_data_exists(data_dir).await? {
        return Err(storage_failure());
    }

    match fs::remove_file(path).await {
        Ok(()) => sync_parent(data_dir).await,
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(_) => Err(storage_failure()),
    }
}

async fn encrypted_data_exists(data_dir: &Path) -> Result<bool, AppError> {
    for file_name in [SESSION_FILE, DEVICE_FILE, SELECTED_FLAT_FILE] {
        match fs::symlink_metadata(data_dir.join(file_name)).await {
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(_) => return Err(storage_failure()),
        }
    }
    Ok(false)
}

async fn repair_existing_secret_permissions(data_dir: &Path) -> Result<(), AppError> {
    for file_name in [
        MASTER_KEY_FILE,
        SESSION_FILE,
        DEVICE_FILE,
        SELECTED_FLAT_FILE,
    ] {
        repair_secret_permissions_if_exists(&data_dir.join(file_name)).await?;
    }
    Ok(())
}

async fn repair_secret_permissions_if_exists(path: &Path) -> Result<bool, AppError> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(storage_failure()),
    };
    if !metadata.file_type().is_file() {
        return Err(storage_failure());
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if metadata.permissions().mode() & 0o777 != 0o600 {
            match fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
                Err(_) => return Err(storage_failure()),
            }
            let file = match fs::File::open(path).await {
                Ok(file) => file,
                Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
                Err(_) => return Err(storage_failure()),
            };
            file.sync_all().await.map_err(|_| storage_failure())?;
        }
    }
    Ok(true)
}

#[cfg(unix)]
async fn set_owner_only_permissions(path: &Path) -> Result<(), AppError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .await
        .map_err(|_| storage_failure())
}

#[cfg(not(unix))]
async fn set_owner_only_permissions(_path: &Path) -> Result<(), AppError> {
    Err(storage_failure())
}

async fn sync_parent(path: &Path) -> Result<(), AppError> {
    fs::File::open(path)
        .await
        .map_err(|_| storage_failure())?
        .sync_all()
        .await
        .map_err(|_| storage_failure())
}

fn storage_failure() -> AppError {
    AppError::new(
        ErrorCode::StorageFailure,
        "Secure session storage is unavailable.",
        true,
    )
}

fn invalid_external_key() -> AppError {
    AppError::invalid_request("VDOME_SECRET_KEY must be Base64-encoded 32 bytes.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_written_before_the_flavor_field_infer_the_original_auth_path() {
        for (web_sso_access_token, expected) in [
            (serde_json::Value::Null, AuthFlavor::LegacySms),
            (
                serde_json::Value::String("old-websso-secret".into()),
                AuthFlavor::WebSso,
            ),
        ] {
            let stored: StoredSession = serde_json::from_value(serde_json::json!({
                "access_token": "old-access-secret",
                "refresh_token": "old-refresh-secret",
                "web_sso_access_token": web_sso_access_token,
                "device_token": null,
                "authenticated_at": "2026-08-06T12:00:00Z"
            }))
            .unwrap();

            let session = PersistedSession::from(stored);

            assert_eq!(session.auth_flavor, expected);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sensitive_temporary_file_is_owner_only_at_open_time() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".session.enc.synthetic.tmp");

        let (_file, temporary) = open_owner_only_temporary_file(path).await.unwrap();

        let mode = std::fs::metadata(temporary.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}
