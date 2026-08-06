use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};

use async_trait::async_trait;
use bytes::Bytes;
use secrecy::{ExposeSecret, SecretString};
use serde::Serialize;
use tempfile::TempDir;
use tokio::{
    io::AsyncWriteExt,
    process::{Child, Command},
    sync::{Mutex, OwnedSemaphorePermit, Semaphore},
    time::{Instant, sleep},
};
use url::Url;
use uuid::Uuid;

use crate::error::{AppError, ErrorCode};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const SESSION_LIFETIME: Duration = Duration::from_secs(10 * 60);
const MAX_PLAYLIST_BYTES: usize = 256 * 1024;
const MAX_SEGMENT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSessionView {
    pub session_id: Uuid,
    pub playlist_url: String,
}

#[derive(Clone, Debug)]
pub struct LiveAsset {
    pub content_type: &'static str,
    pub bytes: Bytes,
}

#[async_trait]
pub trait LiveProcess: Send {
    async fn stop(&mut self);
}

#[async_trait]
pub trait LiveProcessRunner: Send + Sync {
    async fn start(
        &self,
        source: &SecretString,
        output: &Path,
    ) -> Result<Box<dyn LiveProcess>, AppError>;
}

struct LiveSession {
    directory: TempDir,
    process: Box<dyn LiveProcess>,
    expires_at: Instant,
    _permit: OwnedSemaphorePermit,
}

struct Inner {
    runner: Arc<dyn LiveProcessRunner>,
    capacity: Arc<Semaphore>,
    sessions: Mutex<HashMap<Uuid, LiveSession>>,
}

#[derive(Clone)]
pub struct LiveSessionManager {
    inner: Arc<Inner>,
}

impl LiveSessionManager {
    pub fn production() -> Self {
        Self::from_runner(Arc::new(FfmpegRunner), 2)
    }

    #[cfg(feature = "test-support")]
    pub fn new_for_test(
        runner: Arc<dyn LiveProcessRunner>,
        capacity: usize,
    ) -> Result<Self, AppError> {
        if capacity == 0 || capacity > 8 {
            return Err(AppError::invalid_request("invalid live session capacity"));
        }
        Ok(Self::from_runner(runner, capacity))
    }

    fn from_runner(runner: Arc<dyn LiveProcessRunner>, capacity: usize) -> Self {
        Self {
            inner: Arc::new(Inner {
                runner,
                capacity: Arc::new(Semaphore::new(capacity)),
                sessions: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub async fn start(&self, source: SecretString) -> Result<LiveSessionView, AppError> {
        validate_source(&source)?;
        let permit = Arc::clone(&self.inner.capacity)
            .try_acquire_owned()
            .map_err(|_| live_unavailable())?;
        let directory = tempfile::Builder::new()
            .prefix("domru-mts-live-")
            .tempdir()
            .map_err(|_| live_unavailable())?;
        let process = self.inner.runner.start(&source, directory.path()).await?;
        let session_id = Uuid::new_v4();
        self.inner.sessions.lock().await.insert(
            session_id,
            LiveSession {
                directory,
                process,
                expires_at: Instant::now() + SESSION_LIFETIME,
                _permit: permit,
            },
        );
        let manager = self.clone();
        tokio::spawn(async move {
            sleep(SESSION_LIFETIME).await;
            let _ = manager.stop(session_id).await;
        });
        Ok(LiveSessionView {
            session_id,
            playlist_url: format!("/api/v1/live/{session_id}/index.m3u8"),
        })
    }

    pub async fn read(&self, session_id: Uuid, asset: &str) -> Result<LiveAsset, AppError> {
        let (path, content_type, limit) = {
            let sessions = self.inner.sessions.lock().await;
            let session = sessions.get(&session_id).ok_or_else(live_not_found)?;
            if Instant::now() >= session.expires_at {
                return Err(live_not_found());
            }
            let (content_type, limit) = asset_contract(asset)?;
            (session.directory.path().join(asset), content_type, limit)
        };
        let bytes = tokio::fs::read(path).await.map_err(|_| live_not_found())?;
        if bytes.is_empty() || bytes.len() > limit {
            return Err(live_unavailable());
        }
        let bytes = if asset == "index.m3u8" {
            sanitize_playlist(&bytes)?
        } else {
            Bytes::from(bytes)
        };
        Ok(LiveAsset {
            content_type,
            bytes,
        })
    }

    pub async fn stop(&self, session_id: Uuid) -> Result<(), AppError> {
        let Some(mut session) = self.inner.sessions.lock().await.remove(&session_id) else {
            return Ok(());
        };
        session.process.stop().await;
        Ok(())
    }
}

fn validate_source(source: &SecretString) -> Result<(), AppError> {
    let value = source.expose_secret();
    if value.len() > 4_096 || value.contains(['\'', '\\']) || value.chars().any(char::is_control) {
        return Err(live_unavailable());
    }
    let url = Url::parse(value).map_err(|_| live_unavailable())?;
    if !matches!(url.scheme(), "rtsp" | "rtsps") || url.host().is_none() || url.fragment().is_some()
    {
        return Err(live_unavailable());
    }
    Ok(())
}

fn asset_contract(asset: &str) -> Result<(&'static str, usize), AppError> {
    if asset == "index.m3u8" {
        return Ok(("application/vnd.apple.mpegurl", MAX_PLAYLIST_BYTES));
    }
    let digits = asset
        .strip_prefix("segment-")
        .and_then(|value| value.strip_suffix(".ts"))
        .filter(|value| !value.is_empty() && value.len() <= 12)
        .filter(|value| value.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or_else(live_not_found)?;
    let _ = digits;
    Ok(("video/mp2t", MAX_SEGMENT_BYTES))
}

fn sanitize_playlist(bytes: &[u8]) -> Result<Bytes, AppError> {
    let source = std::str::from_utf8(bytes).map_err(|_| live_unavailable())?;
    let mut output = String::with_capacity(source.len());
    for line in source.lines() {
        if line.starts_with('#') || line.is_empty() {
            output.push_str(line);
        } else {
            let name = Path::new(line)
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(live_unavailable)?;
            asset_contract(name)?;
            output.push_str(name);
        }
        output.push('\n');
    }
    Ok(Bytes::from(output))
}

struct FfmpegRunner;

struct FfmpegProcess {
    child: Child,
}

#[async_trait]
impl LiveProcess for FfmpegProcess {
    async fn stop(&mut self) {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
    }
}

#[async_trait]
impl LiveProcessRunner for FfmpegRunner {
    async fn start(
        &self,
        source: &SecretString,
        output: &Path,
    ) -> Result<Box<dyn LiveProcess>, AppError> {
        let playlist = output.join("index.m3u8");
        let segments = output.join("segment-%06d.ts");
        let mut command = Command::new("ffmpeg");
        command
            .kill_on_drop(true)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .args([
                "-hide_banner",
                "-loglevel",
                "quiet",
                "-f",
                "concat",
                "-safe",
                "0",
                "-protocol_whitelist",
                "pipe,rtsp,rtsps,tcp,udp,tls,http,https,crypto",
                "-i",
                "pipe:0",
                "-map",
                "0:v:0",
                "-map",
                "0:a:0?",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-tune",
                "zerolatency",
                "-pix_fmt",
                "yuv420p",
                "-g",
                "25",
                "-sc_threshold",
                "0",
                "-c:a",
                "aac",
                "-b:a",
                "96k",
                "-ac",
                "1",
                "-f",
                "hls",
                "-hls_time",
                "1",
                "-hls_list_size",
                "6",
                "-hls_flags",
                "delete_segments+append_list+omit_endlist+independent_segments+temp_file",
                "-hls_segment_filename",
            ])
            .arg(&segments)
            .arg(&playlist);
        let mut child = command.spawn().map_err(|_| live_unavailable())?;
        let mut stdin = child.stdin.take().ok_or_else(live_unavailable)?;
        let description = format!(
            "ffconcat version 1.0\nfile '{}'\noption rtsp_transport tcp\n",
            source.expose_secret()
        );
        stdin
            .write_all(description.as_bytes())
            .await
            .map_err(|_| live_unavailable())?;
        stdin.shutdown().await.map_err(|_| live_unavailable())?;

        let deadline = Instant::now() + STARTUP_TIMEOUT;
        loop {
            if tokio::fs::metadata(&playlist)
                .await
                .is_ok_and(|metadata| metadata.len() > 0)
            {
                return Ok(Box::new(FfmpegProcess { child }));
            }
            if child.try_wait().map_err(|_| live_unavailable())?.is_some()
                || Instant::now() >= deadline
            {
                let _ = child.kill().await;
                return Err(live_unavailable());
            }
            sleep(Duration::from_millis(100)).await;
        }
    }
}

fn live_not_found() -> AppError {
    AppError::new(ErrorCode::NotFound, "Live session was not found.", false)
}

fn live_unavailable() -> AppError {
    AppError::new(
        ErrorCode::UpstreamUnavailable,
        "Live video is unavailable.",
        true,
    )
}
