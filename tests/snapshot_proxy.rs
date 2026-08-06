use std::sync::Arc;

use async_trait::async_trait;
use domru_mts::{
    domain::{Camera, CameraCapabilities, Capability, FlatId, ImageContentType},
    error::{AppError, ErrorCode},
    provider::mts::{
        MtsClient, TokenSource,
        intercom::DeviceProvider,
        media::{MtsMediaProvider, SnapshotProvider},
    },
    service::{devices::DeviceService, media::MediaService},
};
use secrecy::SecretString;
use url::Url;
use uuid::Uuid;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

const SNAPSHOT_PREFIX: &str = "/vdome-smart-facade/devices/api/v1/cameras";

#[derive(Clone, Copy)]
struct TestTokens;

#[async_trait]
impl TokenSource for TestTokens {
    async fn access_token(&self) -> Option<SecretString> {
        Some(SecretString::from("snapshot-access-secret"))
    }

    async fn device_token(&self) -> Option<SecretString> {
        Some(SecretString::from("snapshot-device-secret"))
    }

    async fn refresh_after_unauthorized(&self) -> Result<(), AppError> {
        Err(AppError::new(
            ErrorCode::SessionExpired,
            "test session expired",
            false,
        ))
    }
}

fn client(server: &MockServer) -> MtsClient {
    let base = Url::parse(&format!("{}/", server.uri())).unwrap();
    MtsClient::new_for_test(base.clone(), base, Arc::new(TestTokens)).unwrap()
}

async fn response(server: &MockServer, status: u16, content_type: Option<&str>, body: &[u8]) {
    let mut template = ResponseTemplate::new(status).set_body_bytes(body.to_vec());
    if let Some(content_type) = content_type {
        template = template.insert_header("content-type", content_type);
    }
    Mock::given(method("POST"))
        .respond_with(template)
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn snapshot_uses_exact_opaque_path_headers_empty_post_and_bytes_response() {
    let server = MockServer::start().await;
    let marker = b"synthetic-webp-fixture-marker";
    response(&server, 200, Some("image/webp"), marker).await;
    let provider = MtsMediaProvider::new_for_test(client(&server), 1024).unwrap();
    let camera_id = "camera /%?# юг";

    let media = provider.snapshot(camera_id).await.unwrap();

    assert_eq!(media.content_type, ImageContentType::Webp);
    assert_eq!(media.bytes.as_ref(), marker);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method.as_str(), "POST");
    assert_eq!(
        request.url.path(),
        format!("{SNAPSHOT_PREFIX}/camera%20%2F%25%3F%23%20%D1%8E%D0%B3/snapshots")
    );
    assert_eq!(request.headers.get("accept").unwrap(), "image/webp");
    assert_eq!(
        request.headers.get("authorization").unwrap(),
        "Bearer snapshot-access-secret"
    );
    assert_eq!(
        request.headers.get("x-device-token").unwrap(),
        "snapshot-device-secret"
    );
    assert_eq!(request.headers.get("x-device-type").unwrap(), "ANDROID");
    assert_eq!(request.headers.get("content-length").unwrap(), "0");
    assert!(Uuid::parse_str(request.headers.get("request-id").unwrap().to_str().unwrap()).is_ok());
    assert!(request.body.is_empty());
    assert!(request.headers.get("x-auth-token").is_none());
}

#[tokio::test]
async fn snapshot_accepts_only_exact_bounded_image_media_types() {
    for (content_type, expected) in [
        ("image/webp", ImageContentType::Webp),
        ("IMAGE/JPEG", ImageContentType::Jpeg),
        ("image/png", ImageContentType::Png),
    ] {
        let server = MockServer::start().await;
        response(&server, 200, Some(content_type), b"safe-image").await;
        let media = MtsMediaProvider::new_for_test(client(&server), 32)
            .unwrap()
            .snapshot("camera-safe")
            .await
            .unwrap();
        assert_eq!(media.content_type, expected);
    }

    for content_type in [
        None,
        Some("application/json"),
        Some("image/webp; charset=binary"),
    ] {
        let server = MockServer::start().await;
        response(&server, 200, content_type, b"provider-secret-body").await;
        let error = MtsMediaProvider::new_for_test(client(&server), 32)
            .unwrap()
            .snapshot("camera-safe")
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::UpstreamSchemaChanged);
        assert!(!format!("{error:?}").contains("provider-secret-body"));
    }
}

#[tokio::test]
async fn snapshot_rejects_declared_and_streamed_oversize_and_empty_images() {
    for body in [b"12345".as_slice(), b"".as_slice()] {
        let server = MockServer::start().await;
        response(&server, 200, Some("image/webp"), body).await;
        let error = MtsMediaProvider::new_for_test(client(&server), 4)
            .unwrap()
            .snapshot("camera-safe")
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::UpstreamSchemaChanged);
    }
}

#[tokio::test]
async fn snapshot_mutation_is_dispatched_once_on_redirect_unauthorized_and_server_failure() {
    for status in [302, 401, 503] {
        let server = MockServer::start().await;
        response(
            &server,
            status,
            Some("text/plain"),
            b"upstream-secret-marker",
        )
        .await;
        let error = MtsMediaProvider::new_for_test(client(&server), 32)
            .unwrap()
            .snapshot("camera-safe")
            .await
            .unwrap_err();
        assert!(!format!("{error:?}").contains("upstream-secret-marker"));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn invalid_camera_ids_fail_before_dispatch() {
    for camera_id in ["", ".", "..", "line\nbreak", &"x".repeat(513)] {
        let server = MockServer::start().await;
        let error = MtsMediaProvider::new_for_test(client(&server), 32)
            .unwrap()
            .snapshot(camera_id)
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}

struct OwnedCameraProvider {
    flat: FlatId,
    camera_id: String,
}

#[async_trait]
impl DeviceProvider for OwnedCameraProvider {
    async fn access_points(
        &self,
        _flat: &FlatId,
        _category: Option<&str>,
    ) -> Result<Vec<domru_mts::AccessPoint>, AppError> {
        Ok(Vec::new())
    }

    async fn cameras(&self, flat: &FlatId) -> Result<Vec<Camera>, AppError> {
        if flat != &self.flat {
            return Ok(Vec::new());
        }
        Ok(vec![Camera {
            id: self.camera_id.clone(),
            flat_id: self.flat.clone(),
            name: "Owned camera".to_owned(),
            access_point_id: None,
            capabilities: CameraCapabilities {
                snapshot: Capability::Available,
                live: Capability::Unavailable { reason: None },
                archive: Capability::Unavailable { reason: None },
            },
        }])
    }
}

#[tokio::test]
async fn media_service_proves_camera_ownership_before_snapshot_dispatch() {
    let server = MockServer::start().await;
    response(&server, 200, Some("image/png"), b"owned-image").await;
    let owner = FlatId("flat-owner".to_owned());
    let devices = Arc::new(DeviceService::new(Arc::new(OwnedCameraProvider {
        flat: owner.clone(),
        camera_id: "camera-owned".to_owned(),
    })));
    let snapshots = Arc::new(MtsMediaProvider::new_for_test(client(&server), 32).unwrap());
    let service = MediaService::new(devices, snapshots);

    let media = service.snapshot(&owner, "camera-owned").await.unwrap();
    assert_eq!(media.bytes.as_ref(), b"owned-image");
    assert_eq!(server.received_requests().await.unwrap().len(), 1);

    let error = service
        .snapshot(&FlatId("flat-other".to_owned()), "camera-owned")
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::NotFound);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[test]
fn media_debug_exposes_only_type_and_byte_length() {
    let media = domru_mts::domain::ProxiedMedia::new(
        ImageContentType::Webp,
        bytes::Bytes::from_static(b"image-debug-secret"),
    );
    let debug = format!("{media:?}");
    assert!(debug.contains("Webp"));
    assert!(debug.contains("18"));
    assert!(!debug.contains("image-debug-secret"));
}
