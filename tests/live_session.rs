use std::{path::Path, sync::Arc};

use async_trait::async_trait;
use domru_mts::{
    error::AppError,
    service::live::{LiveProcess, LiveProcessRunner, LiveSessionManager},
};
use secrecy::{ExposeSecret, SecretString};
use tokio::sync::Mutex;

#[derive(Default)]
struct FakeRunner {
    sources: Mutex<Vec<String>>,
}

struct FakeProcess;

#[async_trait]
impl LiveProcess for FakeProcess {
    async fn stop(&mut self) {}
}

#[async_trait]
impl LiveProcessRunner for FakeRunner {
    async fn start(
        &self,
        source: &SecretString,
        output: &Path,
    ) -> Result<Box<dyn LiveProcess>, AppError> {
        self.sources
            .lock()
            .await
            .push(source.expose_secret().to_owned());
        tokio::fs::write(
            output.join("index.m3u8"),
            b"#EXTM3U\n#EXTINF:1,\n/private/provider/segment-000001.ts\n",
        )
        .await
        .unwrap();
        tokio::fs::write(output.join("segment-000001.ts"), b"synthetic-ts")
            .await
            .unwrap();
        Ok(Box::new(FakeProcess))
    }
}

#[tokio::test]
async fn session_serves_sanitized_bounded_assets_and_stops_idempotently() {
    let runner = Arc::new(FakeRunner::default());
    let manager = LiveSessionManager::new_for_test(runner.clone(), 2).unwrap();

    let view = manager
        .start(SecretString::from("rtsp://provider.invalid/private"))
        .await
        .unwrap();
    let playlist = manager.read(view.session_id, "index.m3u8").await.unwrap();
    let segment = manager
        .read(view.session_id, "segment-000001.ts")
        .await
        .unwrap();

    assert_eq!(playlist.content_type, "application/vnd.apple.mpegurl");
    assert_eq!(
        playlist.bytes.as_ref(),
        b"#EXTM3U\n#EXTINF:1,\nsegment-000001.ts\n"
    );
    assert_eq!(segment.content_type, "video/mp2t");
    assert_eq!(segment.bytes.as_ref(), b"synthetic-ts");
    assert!(view.playlist_url.starts_with("/api/v1/live/"));
    assert!(view.playlist_url.ends_with("/index.m3u8"));
    assert_eq!(runner.sources.lock().await.len(), 1);

    manager.stop(view.session_id).await.unwrap();
    manager.stop(view.session_id).await.unwrap();
    assert!(manager.read(view.session_id, "index.m3u8").await.is_err());
}

#[tokio::test]
async fn session_capacity_and_asset_names_fail_closed() {
    let manager = LiveSessionManager::new_for_test(Arc::new(FakeRunner::default()), 2).unwrap();
    let first = manager
        .start(SecretString::from("rtsp://provider.invalid/one"))
        .await
        .unwrap();
    let second = manager
        .start(SecretString::from("rtsp://provider.invalid/two"))
        .await
        .unwrap();

    assert!(
        manager
            .start(SecretString::from("rtsp://provider.invalid/three"))
            .await
            .is_err()
    );
    for invalid in ["../index.m3u8", "private.ts", "segment-x.ts"] {
        assert!(manager.read(first.session_id, invalid).await.is_err());
    }

    manager.stop(first.session_id).await.unwrap();
    manager.stop(second.session_id).await.unwrap();
}
