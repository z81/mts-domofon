use std::fmt;

use bytes::Bytes;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageContentType {
    Webp,
    Jpeg,
    Png,
}

impl ImageContentType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Webp => "image/webp",
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct ProxiedMedia {
    pub content_type: ImageContentType,
    pub bytes: Bytes,
}

impl ProxiedMedia {
    pub fn new(content_type: ImageContentType, bytes: Bytes) -> Self {
        Self {
            content_type,
            bytes,
        }
    }
}

impl fmt::Debug for ProxiedMedia {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProxiedMedia")
            .field("content_type", &self.content_type)
            .field("byte_len", &self.bytes.len())
            .finish()
    }
}
