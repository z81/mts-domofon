use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use futures_util::{Stream, StreamExt, pin_mut};
use reqwest::{
    Method, Response,
    header::{CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE},
};

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
            .json_content_type()
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
    let mut values = response.headers().get_all(CONTENT_TYPE).iter();
    let raw = values
        .next()
        .and_then(|value| value.to_str().ok())
        .ok_or_else(snapshot_schema_error)?;
    if values.next().is_some() {
        return Err(snapshot_schema_error());
    }
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
    if response.headers().contains_key(CONTENT_ENCODING) {
        return Err(snapshot_schema_error());
    }
    if raw_content_length(&response)?.is_some_and(|length| length > limit as u64) {
        return Err(snapshot_schema_error());
    }
    let stream = response
        .bytes_stream()
        .map(|chunk| chunk.map_err(|_| snapshot_transport_error()));
    read_snapshot_stream(stream, limit).await
}

fn raw_content_length(response: &Response) -> Result<Option<u64>, AppError> {
    let mut values = response.headers().get_all(CONTENT_LENGTH).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some()
        || value.as_bytes().is_empty()
        || !value.as_bytes().iter().all(u8::is_ascii_digit)
    {
        return Err(snapshot_schema_error());
    }
    value
        .to_str()
        .ok()
        .and_then(|value| value.parse().ok())
        .map(Some)
        .ok_or_else(snapshot_schema_error)
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
    use reqwest::{
        Response,
        header::{CONTENT_LENGTH, CONTENT_TYPE, HeaderValue},
    };

    use super::{image_content_type, read_snapshot, read_snapshot_stream};

    fn synthetic_response(body: &'static [u8]) -> Response {
        http::Response::new(Bytes::from_static(body)).into()
    }

    #[test]
    fn multiple_content_type_values_are_rejected_even_when_the_first_is_supported() {
        let mut response = synthetic_response(b"safe-image");
        response
            .headers_mut()
            .append(CONTENT_TYPE, HeaderValue::from_static("image/webp"));
        response
            .headers_mut()
            .append(CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));

        let error = image_content_type(&response).unwrap_err();

        assert_eq!(error.code, crate::error::ErrorCode::UpstreamSchemaChanged);
    }

    #[tokio::test]
    async fn ambiguous_and_invalid_raw_content_lengths_are_rejected() {
        for values in [&["10", "10"][..], &["10, 10"][..], &["not-a-number"][..]] {
            let mut response = synthetic_response(b"safe-image");
            for value in values {
                response.headers_mut().append(
                    CONTENT_LENGTH,
                    HeaderValue::from_str(value).expect("valid test header"),
                );
            }

            let error = read_snapshot(response, 32).await.unwrap_err();

            assert_eq!(error.code, crate::error::ErrorCode::UpstreamSchemaChanged);
        }
    }

    #[tokio::test]
    async fn raw_content_length_is_checked_before_the_body_stream_is_polled() {
        let polls = Arc::new(AtomicUsize::new(0));
        let observed_polls = Arc::clone(&polls);
        let body = reqwest::Body::wrap_stream(stream::poll_fn(move |_| {
            observed_polls.fetch_add(1, Ordering::SeqCst);
            Poll::Ready(Some(Ok::<_, std::io::Error>(Bytes::from_static(b"x"))))
        }));
        let mut response: Response = http::Response::new(body).into();
        response
            .headers_mut()
            .insert(CONTENT_LENGTH, HeaderValue::from_static("5"));

        let error = read_snapshot(response, 4).await.unwrap_err();

        assert_eq!(error.code, crate::error::ErrorCode::UpstreamSchemaChanged);
        assert_eq!(polls.load(Ordering::SeqCst), 0);
    }

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
