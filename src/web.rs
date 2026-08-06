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

#[cfg(not(unix))]
compile_error!("web asset snapshots currently support Unix targets only");

const INDEX_PATH: &str = "/index.html";
const IMMUTABLE_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";
const SNAPSHOT_LIMITS: SnapshotLimits = SnapshotLimits {
    max_entries: 4_096,
    max_file_bytes: 16 * 1024 * 1024,
    max_total_bytes: 64 * 1024 * 1024,
    max_depth: 64,
};

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

    pub(crate) fn is_removed_mts_relay_path(&self) -> bool {
        self.normalized == "/amserver"
            || self.normalized.starts_with("/amserver/")
            || self.normalized == "/wsso/authenticate"
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
        Self::load_with_limits(configured_root, SNAPSHOT_LIMITS)
    }

    fn load_with_limits(
        configured_root: &Path,
        limits: SnapshotLimits,
    ) -> io::Result<(PathBuf, Self)> {
        let root = configured_root.canonicalize()?;
        let mut entries = HashMap::new();

        #[cfg(unix)]
        snapshot_unix::load(&root, &mut entries, limits)?;

        Ok((
            root,
            Self {
                entries: Arc::new(entries),
            },
        ))
    }
}

#[derive(Clone, Copy, Debug)]
struct SnapshotLimits {
    max_entries: usize,
    max_file_bytes: u64,
    max_total_bytes: u64,
    max_depth: usize,
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

    use super::{SnapshotAsset, SnapshotEntry, SnapshotLimits};

    const OPEN_FLAGS: OFlags = OFlags::RDONLY
        .union(OFlags::CLOEXEC)
        .union(OFlags::NOFOLLOW)
        .union(OFlags::NONBLOCK);

    pub(super) fn load(
        root: &Path,
        entries: &mut HashMap<PathBuf, SnapshotEntry>,
        limits: SnapshotLimits,
    ) -> io::Result<()> {
        let root_fd =
            open(root, OPEN_FLAGS | OFlags::DIRECTORY, Mode::empty()).map_err(io::Error::from)?;
        let mut budget = SnapshotBudget::new(limits);
        load_directory(root_fd, root, entries, &mut budget, 0)
    }

    fn load_directory(
        directory_fd: OwnedFd,
        logical_path: &Path,
        entries: &mut HashMap<PathBuf, SnapshotEntry>,
        budget: &mut SnapshotBudget,
        depth: usize,
    ) -> io::Result<()> {
        entries.insert(logical_path.to_owned(), SnapshotEntry::Directory);
        let mut directory = Dir::read_from(&directory_fd).map_err(io::Error::from)?;

        while let Some(entry) = directory.read() {
            let entry = entry.map_err(io::Error::from)?;
            let name_bytes = entry.file_name().to_bytes();
            if matches!(name_bytes, b"." | b"..") {
                continue;
            }

            let child_depth = depth.checked_add(1).ok_or_else(depth_limit_exceeded)?;
            budget.claim_entry(child_depth)?;
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
                FileType::Directory => {
                    load_directory(child_fd, &child_path, entries, budget, child_depth)?
                }
                FileType::RegularFile => {
                    let mut file = File::from(child_fd);
                    let modified = file
                        .metadata()?
                        .modified()
                        .unwrap_or(SystemTime::UNIX_EPOCH);
                    let bytes = budget.read_file(&mut file)?;
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

    struct SnapshotBudget {
        limits: SnapshotLimits,
        entries: usize,
        total_bytes: u64,
    }

    impl SnapshotBudget {
        fn new(limits: SnapshotLimits) -> Self {
            Self {
                limits,
                entries: 0,
                total_bytes: 0,
            }
        }

        fn claim_entry(&mut self, depth: usize) -> io::Result<()> {
            if depth > self.limits.max_depth {
                return Err(depth_limit_exceeded());
            }
            self.entries = self
                .entries
                .checked_add(1)
                .filter(|entries| *entries <= self.limits.max_entries)
                .ok_or_else(entry_limit_exceeded)?;
            Ok(())
        }

        fn read_file(&mut self, file: &mut File) -> io::Result<Vec<u8>> {
            let remaining_total = self.limits.max_total_bytes.saturating_sub(self.total_bytes);
            let allowed = self.limits.max_file_bytes.min(remaining_total);
            let mut bytes = Vec::new();
            file.take(allowed.saturating_add(1))
                .read_to_end(&mut bytes)?;
            let actual = bytes.len() as u64;

            if actual > self.limits.max_file_bytes {
                return Err(file_byte_limit_exceeded());
            }
            if actual > remaining_total {
                return Err(total_byte_limit_exceeded());
            }
            self.total_bytes = self
                .total_bytes
                .checked_add(actual)
                .ok_or_else(total_byte_limit_exceeded)?;
            Ok(bytes)
        }
    }

    fn entry_limit_exceeded() -> io::Error {
        invalid_snapshot("web asset snapshot entry-count limit exceeded")
    }

    fn file_byte_limit_exceeded() -> io::Error {
        invalid_snapshot("web asset snapshot per-file byte limit exceeded")
    }

    fn total_byte_limit_exceeded() -> io::Error {
        invalid_snapshot("web asset snapshot aggregate byte limit exceeded")
    }

    fn depth_limit_exceeded() -> io::Error {
        invalid_snapshot("web asset snapshot directory-depth limit exceeded")
    }

    fn invalid_snapshot(message: &'static str) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, message)
    }
}

fn not_found() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "web asset not found")
}

#[cfg(test)]
mod tests {
    use std::{fs, io};

    use super::{SnapshotBackend, SnapshotLimits};

    #[test]
    fn snapshot_rejects_actual_file_bytes_over_the_per_file_limit() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("private-name.js"), b"12345").unwrap();

        let error = SnapshotBackend::load_with_limits(
            root.path(),
            SnapshotLimits {
                max_entries: 1,
                max_file_bytes: 4,
                max_total_bytes: 10,
                max_depth: 8,
            },
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(!error.to_string().contains("private-name"));
    }

    #[test]
    fn snapshot_rejects_actual_bytes_over_the_aggregate_limit() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("first.js"), b"abc").unwrap();
        fs::write(root.path().join("second.js"), b"def").unwrap();

        let error = SnapshotBackend::load_with_limits(
            root.path(),
            SnapshotLimits {
                max_entries: 2,
                max_file_bytes: 3,
                max_total_bytes: 5,
                max_depth: 8,
            },
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(!error.to_string().contains("first.js"));
        assert!(!error.to_string().contains("second.js"));
    }

    #[test]
    fn snapshot_rejects_more_than_the_entry_count_limit() {
        let root = tempfile::tempdir().unwrap();
        for name in ["one.js", "two.js", "three.js"] {
            fs::write(root.path().join(name), []).unwrap();
        }

        let error = SnapshotBackend::load_with_limits(
            root.path(),
            SnapshotLimits {
                max_entries: 2,
                max_file_bytes: 0,
                max_total_bytes: 0,
                max_depth: 8,
            },
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(!error.to_string().contains(".js"));
    }

    #[test]
    fn snapshot_rejects_directory_nesting_over_the_depth_limit() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("one/two/secret-name")).unwrap();

        let error = SnapshotBackend::load_with_limits(
            root.path(),
            SnapshotLimits {
                max_entries: 3,
                max_file_bytes: 0,
                max_total_bytes: 0,
                max_depth: 2,
            },
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(!error.to_string().contains("secret-name"));
    }
}
