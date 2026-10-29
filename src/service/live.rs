use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};

use async_trait::async_trait;
use bytes::Bytes;
use secrecy::{ExposeSecret, SecretString};
use serde::Serialize;
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    sync::{Mutex, OwnedSemaphorePermit, Semaphore, watch},
    task::JoinHandle,
    time::{Instant, sleep},
};
use url::Url;
use uuid::Uuid;

use crate::error::{AppError, ErrorCode};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(45);
const SESSION_LIFETIME: Duration = Duration::from_secs(10 * 60);
const MAX_PLAYLIST_BYTES: usize = 256 * 1024;
const MAX_SEGMENT_BYTES: usize = 16 * 1024 * 1024;
const MAX_CAMERA_KEY_BYTES: usize = 2_200;
// Players start on the oldest listed segment, so two 1 s segments leave a 1-2 s buffer. Chrome's
// built-in HLS player rejects live playlists shorter than three segments, so the web app prefers
// hls.js wherever Media Source Extensions exist.
const STARTUP_SEGMENT_COUNT: usize = 2;
const STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(50);

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

    async fn wait_for_exit(&mut self);
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
    camera_key: String,
    directory: TempDir,
    process: Arc<Mutex<Box<dyn LiveProcess>>>,
    _permit: OwnedSemaphorePermit,
    supervisor: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct Sessions {
    active: HashMap<Uuid, LiveSession>,
    by_camera: HashMap<String, Uuid>,
    starting: HashMap<String, StartingCamera>,
}

struct StartingCamera {
    token: Uuid,
    completion: watch::Sender<bool>,
}

struct Inner {
    runner: Arc<dyn LiveProcessRunner>,
    capacity: Arc<Semaphore>,
    sessions: Mutex<Sessions>,
}

struct StartingCameraGuard {
    inner: Arc<Inner>,
    camera_key: Option<String>,
    token: Uuid,
    completion: watch::Sender<bool>,
}

impl StartingCameraGuard {
    fn new(
        inner: Arc<Inner>,
        camera_key: String,
        token: Uuid,
        completion: watch::Sender<bool>,
    ) -> Self {
        Self {
            inner,
            camera_key: Some(camera_key),
            token,
            completion,
        }
    }

    fn disarm(&mut self) {
        self.camera_key = None;
        self.completion.send_replace(true);
    }
}

impl Drop for StartingCameraGuard {
    fn drop(&mut self) {
        let Some(camera_key) = self.camera_key.take() else {
            return;
        };
        if let Ok(mut sessions) = self.inner.sessions.try_lock() {
            remove_starting_if_owned(&mut sessions, &camera_key, self.token);
            drop(sessions);
            self.completion.send_replace(true);
            return;
        }
        let inner = Arc::clone(&self.inner);
        let token = self.token;
        let completion = self.completion.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            drop(runtime.spawn(async move {
                let mut sessions = inner.sessions.lock().await;
                remove_starting_if_owned(&mut sessions, &camera_key, token);
                drop(sessions);
                completion.send_replace(true);
            }));
        } else {
            self.completion.send_replace(true);
        }
    }
}

fn remove_starting_if_owned(sessions: &mut Sessions, camera_key: &str, token: Uuid) {
    if sessions
        .starting
        .get(camera_key)
        .is_some_and(|starting| starting.token == token)
    {
        sessions.starting.remove(camera_key);
    }
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
                sessions: Mutex::new(Sessions::default()),
            }),
        }
    }

    /// Returns the camera's running session, first waiting for an in-flight start to settle.
    ///
    /// Callers check this before resolving a provider source, so joining a running stream costs
    /// no provider round trips.
    pub async fn existing(&self, camera_key: &str) -> Option<LiveSessionView> {
        loop {
            let sessions = self.inner.sessions.lock().await;
            if let Some(session_id) = sessions.by_camera.get(camera_key) {
                return Some(session_view(*session_id));
            }
            let mut completion = sessions.starting.get(camera_key)?.completion.subscribe();
            drop(sessions);
            let _ = completion.changed().await;
        }
    }

    pub async fn start(
        &self,
        camera_key: String,
        source: SecretString,
    ) -> Result<LiveSessionView, AppError> {
        validate_source(&source)?;
        validate_camera_key(&camera_key)?;
        let (start_token, start_completion) = loop {
            let mut sessions = self.inner.sessions.lock().await;
            if let Some(session_id) = sessions.by_camera.get(&camera_key) {
                return Ok(session_view(*session_id));
            }
            if let Some(starting) = sessions.starting.get(&camera_key) {
                let mut completion = starting.completion.subscribe();
                drop(sessions);
                let _ = completion.changed().await;
                continue;
            }
            let token = Uuid::new_v4();
            let (completion, _) = watch::channel(false);
            sessions.starting.insert(
                camera_key.clone(),
                StartingCamera {
                    token,
                    completion: completion.clone(),
                },
            );
            break (token, completion);
        };
        let mut starting_camera = StartingCameraGuard::new(
            Arc::clone(&self.inner),
            camera_key.clone(),
            start_token,
            start_completion,
        );
        let permit = Arc::clone(&self.inner.capacity)
            .try_acquire_owned()
            .map_err(|_| live_unavailable());
        let permit = match permit {
            Ok(permit) => permit,
            Err(error) => return Err(error),
        };
        let directory = tempfile::Builder::new()
            .prefix("domru-mts-live-")
            .tempdir()
            .map_err(|_| live_unavailable());
        let directory = match directory {
            Ok(directory) => directory,
            Err(error) => return Err(error),
        };
        let process = self.inner.runner.start(&source, directory.path()).await;
        let process = match process {
            Ok(process) => Arc::new(Mutex::new(process)),
            Err(error) => return Err(error),
        };
        let session_id = Uuid::new_v4();
        let inserted = {
            let mut sessions = self.inner.sessions.lock().await;
            if sessions
                .starting
                .get(&camera_key)
                .is_none_or(|starting| starting.token != start_token)
            {
                false
            } else {
                sessions.starting.remove(&camera_key);
                sessions.by_camera.insert(camera_key.clone(), session_id);
                sessions.active.insert(
                    session_id,
                    LiveSession {
                        camera_key,
                        directory,
                        process: Arc::clone(&process),
                        _permit: permit,
                        supervisor: None,
                    },
                );
                true
            }
        };
        if !inserted {
            process.lock().await.stop().await;
            return Err(live_unavailable());
        }
        starting_camera.disarm();
        let manager = self.clone();
        let supervisor = tokio::spawn(async move {
            manager.supervise(session_id, process).await;
        });
        let mut sessions = self.inner.sessions.lock().await;
        if let Some(session) = sessions.active.get_mut(&session_id) {
            session.supervisor = Some(supervisor);
        }
        Ok(session_view(session_id))
    }

    pub async fn read(&self, session_id: Uuid, asset: &str) -> Result<LiveAsset, AppError> {
        let (path, content_type, limit) = {
            let sessions = self.inner.sessions.lock().await;
            let session = sessions
                .active
                .get(&session_id)
                .ok_or_else(live_not_found)?;
            let (content_type, limit) = asset_contract(asset)?;
            (session.directory.path().join(asset), content_type, limit)
        };
        let file = tokio::fs::File::open(path)
            .await
            .map_err(|_| live_not_found())?;
        let metadata = file.metadata().await.map_err(|_| live_not_found())?;
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() > limit as u64 {
            return Err(live_unavailable());
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| live_not_found())?;
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
        let Some(session) = self.take_session(session_id).await else {
            return Ok(());
        };
        Self::stop_session(session, true).await;
        Ok(())
    }

    pub async fn stop_all(&self) {
        let sessions = {
            let mut sessions = self.inner.sessions.lock().await;
            for starting in sessions.starting.drain().map(|(_, starting)| starting) {
                starting.completion.send_replace(true);
            }
            sessions.by_camera.clear();
            std::mem::take(&mut sessions.active)
        };
        for session in sessions.into_values() {
            Self::stop_session(session, true).await;
        }
    }

    async fn take_session(&self, session_id: Uuid) -> Option<LiveSession> {
        let mut sessions = self.inner.sessions.lock().await;
        let session = sessions.active.remove(&session_id)?;
        if sessions.by_camera.get(&session.camera_key) == Some(&session_id) {
            sessions.by_camera.remove(&session.camera_key);
        }
        Some(session)
    }

    async fn stop_session(mut session: LiveSession, abort_supervisor: bool) {
        if abort_supervisor && let Some(supervisor) = session.supervisor.take() {
            supervisor.abort();
            let _ = supervisor.await;
        }
        session.process.lock().await.stop().await;
    }

    async fn supervise(&self, session_id: Uuid, process: Arc<Mutex<Box<dyn LiveProcess>>>) {
        tokio::select! {
            _ = sleep(SESSION_LIFETIME) => {
                if let Some(session) = self.take_session(session_id).await {
                    Self::stop_session(session, false).await;
                }
            }
            _ = async {
                process.lock().await.wait_for_exit().await;
            } => {
                let _ = self.take_session(session_id).await;
            }
        }
    }
}

fn session_view(session_id: Uuid) -> LiveSessionView {
    LiveSessionView {
        session_id,
        playlist_url: format!("/api/v1/live/{session_id}/index.m3u8"),
    }
}

fn validate_source(source: &SecretString) -> Result<(), AppError> {
    let value = source.expose_secret();
    if value.len() > 4_096 || value.contains(['\'', '\\']) || value.chars().any(char::is_control) {
        return Err(live_unavailable());
    }
    let url = Url::parse(value).map_err(|_| live_unavailable())?;
    if !matches!(url.scheme(), "https" | "rtsp" | "rtsps")
        || url.host().is_none()
        || url.fragment().is_some()
        || (url.scheme() == "https" && (!url.username().is_empty() || url.password().is_some()))
    {
        return Err(live_unavailable());
    }
    Ok(())
}

fn validate_camera_key(camera_key: &str) -> Result<(), AppError> {
    if camera_key.is_empty()
        || camera_key.len() > MAX_CAMERA_KEY_BYTES
        || camera_key.chars().any(char::is_control)
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

fn playlist_has_startup_window(bytes: &[u8]) -> bool {
    let Ok(source) = std::str::from_utf8(bytes) else {
        return false;
    };
    source
        .lines()
        .filter(|line| {
            Path::new(line)
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    asset_contract(name).is_ok_and(|(content_type, _)| content_type == "video/mp2t")
                })
        })
        .take(STARTUP_SEGMENT_COUNT)
        .count()
        == STARTUP_SEGMENT_COUNT
}

struct FfmpegRunner;

struct FfmpegProcess {
    child: Child,
    stderr_task: JoinHandle<Vec<u8>>,
}

#[async_trait]
impl LiveProcess for FfmpegProcess {
    async fn stop(&mut self) {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
        let _ = (&mut self.stderr_task).await;
    }

    async fn wait_for_exit(&mut self) {
        let _ = self.child.wait().await;
        let _ = (&mut self.stderr_task).await;
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
            .stderr(std::process::Stdio::piped())
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "concat",
                "-safe",
                "0",
                "-protocol_whitelist",
                "pipe,rtsp,rtsps,tcp,udp,tls,http,https,crypto",
                // Frame-threaded decoding delays output by one frame per extra thread, and the
                // default starts one thread per core.
                "-threads",
                "2",
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
                // Time-based keyframes keep segments at 1 s whatever the camera frame rate.
                "-force_key_frames",
                "expr:gte(t,n_forced*1)",
                "-sc_threshold",
                "0",
                "-c:a",
                "aac",
                "-b:a",
                "64k",
                "-ac",
                "1",
                // 48 kHz AAC frames last 21 ms instead of 128 ms at 8 kHz, so segments close sooner.
                "-ar",
                "48000",
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
        let stderr = child.stderr.take().ok_or_else(live_unavailable)?;
        let stderr_task = tokio::spawn(drain_ffmpeg_stderr(stderr));
        let mut stdin = child.stdin.take().ok_or_else(live_unavailable)?;
        let source_url = Url::parse(source.expose_secret()).map_err(|_| live_unavailable())?;
        let rtsp_transport = if matches!(source_url.scheme(), "rtsp" | "rtsps") {
            "option rtsp_transport tcp\n"
        } else {
            ""
        };
        // Rate probing waits for 20 frames by default, which delays the first segment by about
        // 0.4 s. Ten frames still give an exact rate; probing less, or capping analyzeduration or
        // probesize, breaks cameras whose H.264 headers carry no frame rate.
        let description = format!(
            "ffconcat version 1.0\nfile '{}'\n{rtsp_transport}option fpsprobesize 10\n",
            source.expose_secret()
        );
        stdin
            .write_all(description.as_bytes())
            .await
            .map_err(|_| live_unavailable())?;
        stdin.shutdown().await.map_err(|_| live_unavailable())?;
        drop(stdin);

        let deadline = Instant::now() + STARTUP_TIMEOUT;
        loop {
            if tokio::fs::read(&playlist)
                .await
                .is_ok_and(|bytes| playlist_has_startup_window(&bytes))
            {
                return Ok(Box::new(FfmpegProcess { child, stderr_task }));
            }
            if child.try_wait().map_err(|_| live_unavailable())?.is_some()
                || Instant::now() >= deadline
            {
                let _ = child.kill().await;
                let _ = child.wait().await;
                let category = stderr_task
                    .await
                    .ok()
                    .map(|stderr| classify_ffmpeg_failure(&stderr))
                    .unwrap_or("task");
                tracing::warn!(category, "live ffmpeg startup failed");
                return Err(live_unavailable());
            }
            sleep(STARTUP_POLL_INTERVAL).await;
        }
    }
}

async fn drain_ffmpeg_stderr(mut stderr: tokio::process::ChildStderr) -> Vec<u8> {
    const RETAINED_BYTES: usize = 16 * 1024;
    let mut retained = Vec::with_capacity(RETAINED_BYTES);
    let mut chunk = [0_u8; 4 * 1024];
    while let Ok(read) = stderr.read(&mut chunk).await {
        if read == 0 {
            break;
        }
        let remaining = RETAINED_BYTES.saturating_sub(retained.len());
        retained.extend_from_slice(&chunk[..read.min(remaining)]);
    }
    retained
}

fn classify_ffmpeg_failure(stderr: &[u8]) -> &'static str {
    let message = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    if message.contains("401") || message.contains("unauthorized") {
        "authentication"
    } else if message.contains("403") || message.contains("forbidden") {
        "forbidden"
    } else if message.contains("404") || message.contains("not found") {
        "source_not_found"
    } else if message.contains("method describe failed") {
        "rtsp_describe"
    } else if message.contains("server returned 4") {
        "upstream_client_error"
    } else if message.contains("server returned 5") {
        "upstream_server_error"
    } else if message.contains("timed out") || message.contains("timeout") {
        "timeout"
    } else if message.contains("connection refused") || message.contains("no route to host") {
        "connection"
    } else if message.contains("network is unreachable")
        || message.contains("host is unreachable")
        || message.contains("connection reset")
        || message.contains("broken pipe")
    {
        "network"
    } else if message.contains("protocol not found") || message.contains("unknown protocol") {
        "protocol"
    } else if message.contains("invalid data") {
        "format"
    } else if message.contains("invalid argument") {
        "invalid_argument"
    } else if message.contains("permission denied") || message.contains("operation not permitted") {
        "permission"
    } else if message.contains("end of file") {
        "eof"
    } else if message.contains("does not contain any stream")
        || message.contains("matches no streams")
    {
        "no_streams"
    } else if message.contains("could not find codec parameters") {
        "codec_parameters"
    } else if message.contains("error opening input") {
        "open_input"
    } else if message.contains("conversion failed") {
        "conversion"
    } else if message.contains("input/output error") {
        "io"
    } else if message.contains("error number -") {
        "averror"
    } else if message.contains("immediate exit requested") {
        "cancelled"
    } else if message.contains("no such process") {
        "no_process"
    } else {
        "process"
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

#[cfg(test)]
mod tests {
    use super::playlist_has_startup_window;

    #[test]
    fn playlist_is_not_ready_until_two_segments_are_published() {
        let one_segment = b"#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:1.0,\nsegment-000000.ts\n";
        let two_segments = b"#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:1.0,\nsegment-000000.ts\n#EXTINF:1.0,\nsegment-000001.ts\n";

        assert!(!playlist_has_startup_window(one_segment));
        assert!(playlist_has_startup_window(two_segments));
    }

    #[test]
    fn master_playlist_uris_do_not_count_as_ready_segments() {
        let master_playlist = b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=800000\nlow/index.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=1400000\nmedium/index.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=2800000\nhigh/index.m3u8\n";

        assert!(!playlist_has_startup_window(master_playlist));
    }
}
