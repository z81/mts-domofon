use std::{
    collections::HashMap,
    future::{Ready, ready},
    io::{self, Cursor, SeekFrom},
    path::{Component, Path, PathBuf},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::SystemTime,
};

use axum::{
    body::Body,
    extract::Request,
    http::{Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use tokio::io::{AsyncRead, AsyncSeek, ReadBuf};
use tower::ServiceExt;
use tower_http::services::fs::{Backend, DefaultServeDirFallback, File, Metadata, ServeDir};

const INDEX_PATH: &str = "/index.html";
const IMMUTABLE_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

type SnapshotServeDir = ServeDir<DefaultServeDirFallback, SnapshotBackend>;

#[derive(Clone, Debug)]
pub(crate) struct ValidatedPath {
    normalized: String,
    lookup_uri: Uri,
}

impl ValidatedPath {
    pub(crate) fn parse(raw_path: &str) -> Option<Self> {
        let decoded = percent_decode_str(raw_path).decode_utf8().ok()?;
        if !decoded.starts_with('/') || decoded.contains(['\0', '\\']) {
            return None;
        }

        let trailing_slash = decoded.len() > 1 && decoded.ends_with('/');
        let mut segments = Vec::new();
        for component in Path::new(decoded.as_ref()).components() {
            match component {
                Component::RootDir => {}
                Component::Normal(segment) => segments.push(segment.to_str()?.to_owned()),
                Component::CurDir | Component::ParentDir | Component::Prefix(_) => return None,
            }
        }

        let mut normalized = String::from("/");
        normalized.push_str(&segments.join("/"));
        if trailing_slash && normalized != "/" {
            normalized.push('/');
        }

        let mut encoded = String::from("/");
        for (index, segment) in segments.iter().enumerate() {
            if index > 0 {
                encoded.push('/');
            }
            encoded.push_str(&utf8_percent_encode(segment, NON_ALPHANUMERIC).to_string());
        }
        if trailing_slash && encoded != "/" {
            encoded.push('/');
        }

        Some(Self {
            normalized,
            lookup_uri: encoded.parse().ok()?,
        })
    }

    pub(crate) fn is_api(&self) -> bool {
        self.normalized == "/api" || self.normalized.starts_with("/api/")
    }

    fn is_index(&self) -> bool {
        self.normalized == "/" || self.normalized == INDEX_PATH
    }

    fn is_hashed_asset(&self) -> bool {
        let Some(file_name) = self.normalized.strip_prefix("/assets/") else {
            return false;
        };
        let Some((stem, _extension)) = file_name.rsplit_once('.') else {
            return false;
        };
        let Some((_, hash)) = stem.rsplit_once('-') else {
            return false;
        };
        hash.len() >= 8
            && hash
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    }
}

#[derive(Clone, Debug)]
pub(crate) struct WebAssets {
    service: SnapshotServeDir,
}

impl WebAssets {
    pub(crate) fn new(web_dist_dir: PathBuf) -> Self {
        let (root, backend) = match SnapshotBackend::load(&web_dist_dir) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                tracing::warn!(%error, "web asset snapshot unavailable");
                (web_dist_dir, SnapshotBackend::default())
            }
        };

        // ServeDir retains HTTP file semantics and MIME detection, while the custom backend
        // makes the validated startup snapshot the only request-time asset source. No request
        // can race a later source-tree symlink or content change because the filesystem is never
        // reopened after this constructor returns.
        let service = ServeDir::with_backend(root, backend).append_index_html_on_directories(true);
        Self { service }
    }

    pub(crate) async fn serve(&self, mut request: Request, path: &ValidatedPath) -> Response {
        if request.method() != Method::GET {
            return StatusCode::NOT_FOUND.into_response();
        }

        *request.uri_mut() = path.lookup_uri.clone();
        let response = self.serve_dir(request).await;
        if response.status() == StatusCode::NOT_FOUND {
            let index_request = Request::get(INDEX_PATH)
                .body(Body::empty())
                .expect("static index URI is valid");
            return self
                .with_cache_control(self.serve_dir(index_request).await, CachePolicy::Index);
        }

        let cache_policy = if path.is_index() {
            CachePolicy::Index
        } else if path.is_hashed_asset() {
            CachePolicy::Immutable
        } else {
            CachePolicy::Default
        };
        self.with_cache_control(response, cache_policy)
    }

    async fn serve_dir(&self, request: Request) -> Response {
        self.service
            .clone()
            .oneshot(request)
            .await
            .unwrap_or_else(|never| match never {})
            .map(Body::new)
    }

    fn with_cache_control(&self, mut response: Response, policy: CachePolicy) -> Response {
        if response.status().is_success() {
            let value = match policy {
                CachePolicy::Index => "no-cache",
                CachePolicy::Immutable => IMMUTABLE_CACHE_CONTROL,
                CachePolicy::Default => return response,
            };
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                value.parse().expect("valid cache header"),
            );
        }
        response
    }
}

#[derive(Clone, Copy)]
enum CachePolicy {
    Default,
    Index,
    Immutable,
}

#[derive(Clone, Debug, Default)]
struct SnapshotBackend {
    entries: Arc<HashMap<PathBuf, SnapshotEntry>>,
}

impl SnapshotBackend {
    fn load(configured_root: &Path) -> io::Result<(PathBuf, Self)> {
        let root = configured_root.canonicalize()?;
        let mut entries = HashMap::new();

        #[cfg(unix)]
        snapshot_unix::load(&root, &mut entries)?;
        #[cfg(not(unix))]
        load_portable(&root, &mut entries)?;

        Ok((
            root,
            Self {
                entries: Arc::new(entries),
            },
        ))
    }
}

impl Backend for SnapshotBackend {
    type File = SnapshotFile;
    type Metadata = SnapshotMetadata;
    type OpenFuture = Ready<io::Result<Self::File>>;
    type MetadataFuture = Ready<io::Result<Self::Metadata>>;

    fn open(&self, path: PathBuf) -> Self::OpenFuture {
        ready(match self.entries.get(&path) {
            Some(SnapshotEntry::File(asset)) => Ok(SnapshotFile {
                cursor: Cursor::new(Arc::clone(&asset.bytes)),
                modified: asset.modified,
            }),
            _ => Err(not_found()),
        })
    }

    fn metadata(&self, path: PathBuf) -> Self::MetadataFuture {
        ready(match self.entries.get(&path) {
            Some(SnapshotEntry::Directory) => Ok(SnapshotMetadata {
                is_dir: true,
                len: 0,
                modified: SystemTime::UNIX_EPOCH,
            }),
            Some(SnapshotEntry::File(asset)) => Ok(SnapshotMetadata {
                is_dir: false,
                len: asset.bytes.len() as u64,
                modified: asset.modified,
            }),
            None => Err(not_found()),
        })
    }
}

#[derive(Clone, Debug)]
enum SnapshotEntry {
    Directory,
    File(SnapshotAsset),
}

#[derive(Clone, Debug)]
struct SnapshotAsset {
    bytes: Arc<[u8]>,
    modified: SystemTime,
}

#[derive(Clone, Debug)]
struct SnapshotMetadata {
    is_dir: bool,
    len: u64,
    modified: SystemTime,
}

impl Metadata for SnapshotMetadata {
    fn is_dir(&self) -> bool {
        self.is_dir
    }

    fn modified(&self) -> io::Result<SystemTime> {
        Ok(self.modified)
    }

    fn len(&self) -> u64 {
        self.len
    }
}

#[derive(Debug)]
struct SnapshotFile {
    cursor: Cursor<Arc<[u8]>>,
    modified: SystemTime,
}

impl AsyncRead for SnapshotFile {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.cursor).poll_read(context, buffer)
    }
}

impl AsyncSeek for SnapshotFile {
    fn start_seek(mut self: Pin<&mut Self>, position: SeekFrom) -> io::Result<()> {
        Pin::new(&mut self.cursor).start_seek(position)
    }

    fn poll_complete(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<u64>> {
        Pin::new(&mut self.cursor).poll_complete(context)
    }
}

impl File for SnapshotFile {
    type Metadata = SnapshotMetadata;
    type MetadataFuture<'a> = Ready<io::Result<Self::Metadata>>;

    fn metadata(&self) -> Self::MetadataFuture<'_> {
        ready(Ok(SnapshotMetadata {
            is_dir: false,
            len: self.cursor.get_ref().len() as u64,
            modified: self.modified,
        }))
    }
}

#[cfg(unix)]
mod snapshot_unix {
    use std::{
        collections::HashMap,
        ffi::OsStr,
        fs::File,
        io::{self, Read},
        os::{fd::OwnedFd, unix::ffi::OsStrExt},
        path::{Path, PathBuf},
        sync::Arc,
        time::SystemTime,
    };

    use rustix::fs::{Dir, FileType, Mode, OFlags, fstat, open, openat};

    use super::{SnapshotAsset, SnapshotEntry};

    const OPEN_FLAGS: OFlags = OFlags::RDONLY
        .union(OFlags::CLOEXEC)
        .union(OFlags::NOFOLLOW);

    pub(super) fn load(
        root: &Path,
        entries: &mut HashMap<PathBuf, SnapshotEntry>,
    ) -> io::Result<()> {
        let root_fd =
            open(root, OPEN_FLAGS | OFlags::DIRECTORY, Mode::empty()).map_err(io::Error::from)?;
        load_directory(root_fd, root, entries)
    }

    fn load_directory(
        directory_fd: OwnedFd,
        logical_path: &Path,
        entries: &mut HashMap<PathBuf, SnapshotEntry>,
    ) -> io::Result<()> {
        entries.insert(logical_path.to_owned(), SnapshotEntry::Directory);
        let mut directory = Dir::read_from(&directory_fd).map_err(io::Error::from)?;

        while let Some(entry) = directory.read() {
            let entry = entry.map_err(io::Error::from)?;
            let name_bytes = entry.file_name().to_bytes();
            if matches!(name_bytes, b"." | b"..") {
                continue;
            }

            let name = OsStr::from_bytes(name_bytes);
            let child_path = logical_path.join(name);
            let child_fd = openat(&directory_fd, entry.file_name(), OPEN_FLAGS, Mode::empty())
                .map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("web build entry could not be opened without symlinks: {error}"),
                    )
                })?;
            let stat = fstat(&child_fd).map_err(io::Error::from)?;

            match FileType::from_raw_mode(stat.st_mode) {
                FileType::Directory => load_directory(child_fd, &child_path, entries)?,
                FileType::RegularFile => {
                    let mut bytes = Vec::new();
                    let mut file = File::from(child_fd);
                    let modified = file
                        .metadata()?
                        .modified()
                        .unwrap_or(SystemTime::UNIX_EPOCH);
                    file.read_to_end(&mut bytes)?;
                    entries.insert(
                        child_path,
                        SnapshotEntry::File(SnapshotAsset {
                            bytes: Arc::from(bytes),
                            modified,
                        }),
                    );
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "web build contains a non-file entry",
                    ));
                }
            }
        }

        Ok(())
    }
}

#[cfg(not(unix))]
fn load_portable(
    directory: &Path,
    entries: &mut HashMap<PathBuf, SnapshotEntry>,
) -> io::Result<()> {
    entries.insert(directory.to_owned(), SnapshotEntry::Directory);
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let path = entry.path();
        if file_type.is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "web build contains a symlink",
            ));
        }
        if file_type.is_dir() {
            load_portable(&path, entries)?;
        } else if file_type.is_file() {
            let modified = entry
                .metadata()?
                .modified()
                .unwrap_or(SystemTime::UNIX_EPOCH);
            entries.insert(
                path.clone(),
                SnapshotEntry::File(SnapshotAsset {
                    bytes: Arc::from(std::fs::read(path)?),
                    modified,
                }),
            );
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "web build contains a non-file entry",
            ));
        }
    }
    Ok(())
}

fn not_found() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "web asset not found")
}
