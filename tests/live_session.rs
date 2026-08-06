use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use domru_mts::{
    error::AppError,
    service::live::{LiveProcess, LiveProcessRunner, LiveSessionManager},
};
use secrecy::{ExposeSecret, SecretString};
use tokio::sync::{Mutex, oneshot};

#[derive(Default)]
struct FakeRunner {
    sources: Mutex<Vec<String>>,
    outputs: Mutex<Vec<PathBuf>>,
    exits: Mutex<Vec<oneshot::Sender<()>>>,
    stop_calls: Arc<AtomicUsize>,
}

struct FakeProcess {
    exited: oneshot::Receiver<()>,
    stop_calls: Arc<AtomicUsize>,
}

#[async_trait]
impl LiveProcess for FakeProcess {
    async fn stop(&mut self) {
        self.stop_calls.fetch_add(1, Ordering::SeqCst);
    }

    async fn wait_for_exit(&mut self) {
        let _ = (&mut self.exited).await;
    }
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
        self.outputs.lock().await.push(output.to_owned());
        tokio::fs::write(
            output.join("index.m3u8"),
            b"#EXTM3U\n#EXTINF:1,\n/private/provider/segment-000001.ts\n",
        )
        .await
        .unwrap();
        tokio::fs::write(output.join("segment-000001.ts"), b"synthetic-ts")
            .await
            .unwrap();
        let (exit, exited) = oneshot::channel();
        self.exits.lock().await.push(exit);
        Ok(Box::new(FakeProcess {
            exited,
            stop_calls: Arc::clone(&self.stop_calls),
        }))
    }
}

impl FakeRunner {
    async fn end_next_process(&self) {
        self.exits.lock().await.remove(0).send(()).unwrap();
    }

    async fn output_for(&self, index: usize) -> PathBuf {
        self.outputs.lock().await[index].clone()
    }
}

#[tokio::test]
async fn session_serves_sanitized_bounded_assets_and_stops_idempotently() {
    let runner = Arc::new(FakeRunner::default());
    let manager = LiveSessionManager::new_for_test(runner.clone(), 2).unwrap();

    let view = manager
        .start(
            "camera-private".to_owned(),
            SecretString::from("rtsp://provider.invalid/private"),
        )
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
        .start(
            "camera-one".to_owned(),
            SecretString::from("rtsp://provider.invalid/one"),
        )
        .await
        .unwrap();
    let second = manager
        .start(
            "camera-two".to_owned(),
            SecretString::from("rtsp://provider.invalid/two"),
        )
        .await
        .unwrap();

    assert!(
        manager
            .start(
                "camera-three".to_owned(),
                SecretString::from("rtsp://provider.invalid/three"),
            )
            .await
            .is_err()
    );
    for invalid in ["../index.m3u8", "private.ts", "segment-x.ts"] {
        assert!(manager.read(first.session_id, invalid).await.is_err());
    }

    manager.stop(first.session_id).await.unwrap();
    manager.stop(second.session_id).await.unwrap();
}

#[tokio::test]
async fn oversized_segment_is_rejected_from_its_declared_metadata() {
    let runner = Arc::new(FakeRunner::default());
    let manager = LiveSessionManager::new_for_test(runner.clone(), 1).unwrap();
    let view = manager
        .start(
            "camera-one".to_owned(),
            SecretString::from("rtsp://provider.invalid/one"),
        )
        .await
        .unwrap();
    let segment = runner.output_for(0).await.join("segment-000001.ts");
    tokio::fs::write(&segment, vec![0_u8; 16 * 1024 * 1024 + 1])
        .await
        .unwrap();

    assert!(
        manager
            .read(view.session_id, "segment-000001.ts")
            .await
            .is_err()
    );
    manager.stop_all().await;
}

#[tokio::test]
async fn session_start_reuses_the_active_camera_session() {
    let runner = Arc::new(FakeRunner::default());
    let manager = LiveSessionManager::new_for_test(runner.clone(), 2).unwrap();

    let first = manager
        .start(
            "camera-one".to_owned(),
            SecretString::from("rtsp://provider.invalid/first"),
        )
        .await
        .unwrap();
    let repeated = manager
        .start(
            "camera-one".to_owned(),
            SecretString::from("rtsp://provider.invalid/rotated"),
        )
        .await
        .unwrap();

    assert_eq!(repeated.session_id, first.session_id);
    assert_eq!(runner.sources.lock().await.len(), 1);
    manager.stop_all().await;
}

#[tokio::test]
async fn exited_process_releases_its_camera_and_capacity() {
    let runner = Arc::new(FakeRunner::default());
    let manager = LiveSessionManager::new_for_test(runner.clone(), 1).unwrap();
    let first = manager
        .start(
            "camera-one".to_owned(),
            SecretString::from("rtsp://provider.invalid/one"),
        )
        .await
        .unwrap();

    runner.end_next_process().await;
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if manager
                .start(
                    "camera-two".to_owned(),
                    SecretString::from("rtsp://provider.invalid/two"),
                )
                .await
                .is_ok()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("dead process releases capacity");
    assert!(manager.read(first.session_id, "index.m3u8").await.is_err());
    manager.stop_all().await;
}

#[tokio::test]
async fn stop_all_stops_every_active_process_and_releases_capacity() {
    let runner = Arc::new(FakeRunner::default());
    let manager = LiveSessionManager::new_for_test(runner.clone(), 2).unwrap();
    let first = manager
        .start(
            "camera-one".to_owned(),
            SecretString::from("rtsp://provider.invalid/one"),
        )
        .await
        .unwrap();
    let second = manager
        .start(
            "camera-two".to_owned(),
            SecretString::from("rtsp://provider.invalid/two"),
        )
        .await
        .unwrap();

    manager.stop_all().await;

    assert_eq!(runner.stop_calls.load(Ordering::SeqCst), 2);
    assert!(manager.read(first.session_id, "index.m3u8").await.is_err());
    assert!(manager.read(second.session_id, "index.m3u8").await.is_err());
    assert!(
        manager
            .start(
                "camera-three".to_owned(),
                SecretString::from("rtsp://provider.invalid/three"),
            )
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn https_live_source_rejects_userinfo_before_starting_a_process() {
    let runner = Arc::new(FakeRunner::default());
    let manager = LiveSessionManager::new_for_test(runner.clone(), 1).unwrap();

    assert!(
        manager
            .start(
                "camera-one".to_owned(),
                SecretString::from("https://user:password@provider.invalid/live.m3u8"),
            )
            .await
            .is_err()
    );
    assert!(runner.sources.lock().await.is_empty());
}

#[tokio::test]
async fn https_live_source_starts_without_userinfo() {
    let runner = Arc::new(FakeRunner::default());
    let manager = LiveSessionManager::new_for_test(runner.clone(), 1).unwrap();

    let view = manager
        .start(
            "camera-one".to_owned(),
            SecretString::from("https://provider.invalid/live.m3u8"),
        )
        .await
        .expect("https source is supported");
    assert_eq!(runner.sources.lock().await.len(), 1);
    manager.stop(view.session_id).await.unwrap();
}
