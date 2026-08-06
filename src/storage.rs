use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use rand::RngCore;
use secrecy::ExposeSecret;
use tokio::{fs, io::AsyncWriteExt};
use zeroize::Zeroizing;

use crate::{
    domain::auth::{DeviceIdentity, PersistedSession, TokenBundle},
    error::{AppError, ErrorCode},
};

const MAGIC: &[u8; 4] = b"VDM1";
const NONCE_LEN: usize = 12;
const SESSION_FILE: &str = "session.enc";
const DEVICE_FILE: &str = "device.enc";
const MASTER_KEY_FILE: &str = "master.key";
const SESSION_AAD: &[u8] = b"domru-mts/session/v1";
const DEVICE_AAD: &[u8] = b"domru-mts/device/v1";

#[derive(Clone)]
pub struct SecretStore {
    data_dir: PathBuf,
    key: Zeroizing<[u8; 32]>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StoredSession {
    access_token: Zeroizing<String>,
    refresh_token: Zeroizing<String>,
    web_sso_access_token: Zeroizing<String>,
    device_token: Option<Zeroizing<String>>,
    authenticated_at: chrono::DateTime<chrono::Utc>,
}

impl From<&PersistedSession> for StoredSession {
    fn from(session: &PersistedSession) -> Self {
        Self {
            access_token: Zeroizing::new(session.tokens.access_token.expose_secret().to_owned()),
            refresh_token: Zeroizing::new(session.tokens.refresh_token.expose_secret().to_owned()),
            web_sso_access_token: Zeroizing::new(
                session
                    .tokens
                    .web_sso_access_token
                    .expose_secret()
                    .to_owned(),
            ),
            device_token: session
                .tokens
                .device_token
                .as_ref()
                .map(|token| Zeroizing::new(token.expose_secret().to_owned())),
            authenticated_at: session.authenticated_at,
        }
    }
}

impl From<StoredSession> for PersistedSession {
    fn from(session: StoredSession) -> Self {
        Self {
            tokens: TokenBundle {
                access_token: secrecy::SecretString::from(session.access_token.to_string()),
                refresh_token: secrecy::SecretString::from(session.refresh_token.to_string()),
                web_sso_access_token: secrecy::SecretString::from(
                    session.web_sso_access_token.to_string(),
                ),
                device_token: session
                    .device_token
                    .map(|token| secrecy::SecretString::from(token.to_string())),
            },
            authenticated_at: session.authenticated_at,
        }
    }
}

impl SecretStore {
    pub async fn open(
        data_dir: impl AsRef<Path>,
        external_key: Option<[u8; 32]>,
    ) -> Result<Self, AppError> {
        let data_dir = data_dir.as_ref().to_path_buf();
        fs::create_dir_all(&data_dir)
            .await
            .map_err(|_| storage_failure())?;
        let key = match external_key {
            Some(key) => key,
            None => load_or_create_master_key(&data_dir).await?,
        };

        Ok(Self {
            data_dir,
            key: Zeroizing::new(key),
        })
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
        self.atomic_write(SESSION_FILE, &encrypted).await
    }

    pub async fn clear_session(&self) -> Result<(), AppError> {
        let path = self.data_dir.join(SESSION_FILE);
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
        self.atomic_write(DEVICE_FILE, &encrypted).await?;
        Ok(device)
    }

    async fn read_encrypted(
        &self,
        file_name: &str,
        aad: &[u8],
    ) -> Result<Option<Zeroizing<Vec<u8>>>, AppError> {
        let path = self.data_dir.join(file_name);
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

    async fn atomic_write(&self, file_name: &str, contents: &[u8]) -> Result<(), AppError> {
        let destination = self.data_dir.join(file_name);
        let temporary = self
            .data_dir
            .join(format!(".{file_name}.{}.tmp", uuid::Uuid::new_v4()));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        let mut file = options
            .open(&temporary)
            .await
            .map_err(|_| storage_failure())?;

        set_owner_only_permissions(&temporary).await?;
        if file.write_all(contents).await.is_err() {
            let _ = fs::remove_file(&temporary).await;
            return Err(storage_failure());
        }
        if file.sync_all().await.is_err() {
            let _ = fs::remove_file(&temporary).await;
            return Err(storage_failure());
        }
        drop(file);

        if fs::rename(&temporary, destination).await.is_err() {
            let _ = fs::remove_file(&temporary).await;
            return Err(storage_failure());
        }
        sync_parent(&self.data_dir).await
    }
}

async fn load_or_create_master_key(data_dir: &Path) -> Result<[u8; 32], AppError> {
    let path = data_dir.join(MASTER_KEY_FILE);
    match fs::read(&path).await {
        Ok(key) => key.try_into().map_err(|_: Vec<u8>| storage_failure()),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let mut key = [0_u8; 32];
            rand::rng().fill_bytes(&mut key);
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            match options.open(&path).await {
                Ok(mut file) => {
                    set_owner_only_permissions(&path).await?;
                    file.write_all(&key).await.map_err(|_| storage_failure())?;
                    file.sync_all().await.map_err(|_| storage_failure())?;
                    drop(file);
                    sync_parent(data_dir).await?;
                    Ok(key)
                }
                Err(create_error) if create_error.kind() == ErrorKind::AlreadyExists => {
                    let key = fs::read(path).await.map_err(|_| storage_failure())?;
                    key.try_into().map_err(|_: Vec<u8>| storage_failure())
                }
                Err(_) => Err(storage_failure()),
            }
        }
        Err(_) => Err(storage_failure()),
    }
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
    Ok(())
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
