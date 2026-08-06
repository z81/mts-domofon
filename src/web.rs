use std::path::{Component, Path, PathBuf};

use axum::{
    body::Body,
    extract::Request,
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use percent_encoding::percent_decode_str;
use tower::ServiceExt;
use tower_http::services::ServeDir;

const INDEX_PATH: &str = "/index.html";
const IMMUTABLE_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

#[derive(Clone)]
pub(crate) struct WebAssets {
    root: Option<PathBuf>,
    service: ServeDir,
}

impl WebAssets {
    pub(crate) fn new(web_dist_dir: PathBuf) -> Self {
        let root = web_dist_dir.canonicalize().ok();
        let service = ServeDir::new(root.as_ref().unwrap_or(&web_dist_dir))
            .append_index_html_on_directories(true);
        Self { root, service }
    }

    pub(crate) async fn serve(&self, request: Request) -> Response {
        if request.method() != Method::GET || !self.path_is_safe(request.uri().path()) {
            return StatusCode::NOT_FOUND.into_response();
        }

        if !self.path_stays_within_root(request.uri().path()) {
            return StatusCode::NOT_FOUND.into_response();
        }

        let requested_path = request.uri().path().to_owned();
        let response = self.serve_dir(request).await;
        if response.status() == StatusCode::NOT_FOUND {
            if !self.path_stays_within_root(INDEX_PATH) {
                return StatusCode::NOT_FOUND.into_response();
            }
            let index_request = Request::get(INDEX_PATH)
                .body(Body::empty())
                .expect("static index URI is valid");
            return self.with_cache_control(self.serve_dir(index_request).await, true, INDEX_PATH);
        }

        self.with_cache_control(
            response,
            requested_path == "/" || requested_path == INDEX_PATH,
            &requested_path,
        )
    }

    async fn serve_dir(&self, request: Request) -> Response {
        self.service
            .clone()
            .oneshot(request)
            .await
            .unwrap_or_else(|never| match never {})
            .map(Body::new)
    }

    fn with_cache_control(&self, mut response: Response, is_index: bool, path: &str) -> Response {
        if response.status().is_success() {
            let value = if is_index {
                "no-cache"
            } else if is_hashed_asset(path) {
                IMMUTABLE_CACHE_CONTROL
            } else {
                return response;
            };
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                value.parse().expect("valid cache header"),
            );
        }
        response
    }

    fn path_is_safe(&self, path: &str) -> bool {
        let Ok(decoded) = percent_decode_str(path).decode_utf8() else {
            return false;
        };
        Path::new(decoded.as_ref())
            .components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
    }

    fn path_stays_within_root(&self, path: &str) -> bool {
        let Some(root) = &self.root else {
            return false;
        };
        let Ok(decoded) = percent_decode_str(path).decode_utf8() else {
            return false;
        };
        let relative = decoded.trim_start_matches('/');
        let candidate = root.join(relative);
        canonical_existing_ancestor(candidate).is_some_and(|ancestor| ancestor.starts_with(root))
    }
}

fn canonical_existing_ancestor(mut path: PathBuf) -> Option<PathBuf> {
    loop {
        match path.canonicalize() {
            Ok(canonical) => return Some(canonical),
            Err(_) if path.pop() => {}
            Err(_) => return None,
        }
    }
}

fn is_hashed_asset(path: &str) -> bool {
    let Some(file_name) = path.strip_prefix("/assets/") else {
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
