use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use futures_util::{Stream, StreamExt, pin_mut};
use reqwest::{Method, Response, header::CONTENT_TYPE};

use crate::{
    domain::{ImageContentType, ProxiedMedia},
    error::{AppError, ErrorCode},
};

use super::{MtsClient, ProviderBase, ResponseAccept};

const SNAPSHOT_PREFIX: &str = "/vdome-smart-facade/devices/api/v1/cameras";
const MAX_CAMERA_ID_BYTES: usize = 512;
const DEFAULT_MAX_SNAPSHOT_BYTES: usize = 5 * 1024 * 1024;

#[async_trait]
pub trait SnapshotProvider: Send + Sync {
    async fn snapshot(&self, camera_id: &str) -> Result<ProxiedMedia, AppError>;
}

#[derive(Clone)]
pub struct MtsMediaProvider {
    client: MtsClient,
    max_snapshot_bytes: usize,
}

impl MtsMediaProvider {
    pub fn new(client: MtsClient) -> Self {
        Self {
            client,
            max_snapshot_bytes: DEFAULT_MAX_SNAPSHOT_BYTES,
        }
    }

    #[cfg(feature = "test-support")]
    pub fn new_for_test(client: MtsClient, max_snapshot_bytes: usize) -> Result<Self, AppError> {
        if max_snapshot_bytes == 0 || max_snapshot_bytes > DEFAULT_MAX_SNAPSHOT_BYTES {
            return Err(AppError::invalid_request("invalid snapshot byte limit"));
        }
        Ok(Self {
            client,
            max_snapshot_bytes,
        })
    }
}

#[async_trait]
impl SnapshotProvider for MtsMediaProvider {
    async fn snapshot(&self, camera_id: &str) -> Result<ProxiedMedia, AppError> {
        validate_camera_id(camera_id)?;
        let request = self
            .client
            .authorized_with_path_segment(
                Method::POST,
                ProviderBase::Gateway,
                SNAPSHOT_PREFIX,
                camera_id,
                Some("snapshots"),
            )?
            .explicit_empty_body()
            .response_accept(ResponseAccept::ImageWebp);
        let response = self.client.send_stream(request).await?;
        let content_type = image_content_type(&response)?;
        let bytes = read_snapshot(response, self.max_snapshot_bytes).await?;
        if bytes.is_empty() {
            return Err(snapshot_schema_error());
        }
        Ok(ProxiedMedia::new(content_type, bytes))
    }
}

fn validate_camera_id(camera_id: &str) -> Result<(), AppError> {
    if camera_id.is_empty()
        || camera_id.len() > MAX_CAMERA_ID_BYTES
        || camera_id.chars().any(char::is_control)
    {
        return Err(AppError::invalid_request("invalid camera ID"));
    }
    Ok(())
}

fn image_content_type(response: &Response) -> Result<ImageContentType, AppError> {
    let raw = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(snapshot_schema_error)?;
    if raw.eq_ignore_ascii_case("image/webp") {
        Ok(ImageContentType::Webp)
    } else if raw.eq_ignore_ascii_case("image/jpeg") {
        Ok(ImageContentType::Jpeg)
    } else if raw.eq_ignore_ascii_case("image/png") {
        Ok(ImageContentType::Png)
    } else {
        Err(snapshot_schema_error())
    }
}

async fn read_snapshot(response: Response, limit: usize) -> Result<Bytes, AppError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(snapshot_schema_error());
    }
    let stream = response
        .bytes_stream()
        .map(|chunk| chunk.map_err(|_| snapshot_transport_error()));
    read_snapshot_stream(stream, limit).await
}

async fn read_snapshot_stream<S>(stream: S, limit: usize) -> Result<Bytes, AppError>
where
    S: Stream<Item = Result<Bytes, AppError>>,
{
    pin_mut!(stream);
    let mut bytes = BytesMut::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        let next_len = bytes
            .len()
            .checked_add(chunk.len())
            .filter(|next_len| *next_len <= limit)
            .ok_or_else(snapshot_schema_error)?;
        bytes.reserve(next_len - bytes.len());
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes.freeze())
}

fn snapshot_schema_error() -> AppError {
    AppError::new(
        ErrorCode::UpstreamSchemaChanged,
        "Provider snapshot response was invalid.",
        false,
    )
}

fn snapshot_transport_error() -> AppError {
    AppError::new(
        ErrorCode::UpstreamUnavailable,
        "Provider snapshot response failed.",
        true,
    )
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        task::Poll,
    };

    use bytes::Bytes;
    use futures_util::stream;

    use super::read_snapshot_stream;

    #[tokio::test]
    async fn streaming_limit_stops_before_polling_an_unread_tail() {
        let polls = Arc::new(AtomicUsize::new(0));
        let observed_polls = Arc::clone(&polls);
        let mut chunks = VecDeque::from([
            Bytes::from_static(b"1234"),
            Bytes::from_static(b"5"),
            Bytes::from_static(b"unread-secret-tail"),
        ]);
        let stream = stream::poll_fn(move |_| {
            observed_polls.fetch_add(1, Ordering::SeqCst);
            Poll::Ready(chunks.pop_front().map(Ok))
        });

        let error = read_snapshot_stream(stream, 4).await.unwrap_err();

        assert_eq!(error.code, crate::error::ErrorCode::UpstreamSchemaChanged);
        assert_eq!(polls.load(Ordering::SeqCst), 2);
    }
}
