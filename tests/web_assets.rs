use std::{fs, path::Path};

use axum::{
    body::Body,
    http::{Method, Request, Response, StatusCode, header},
};
use domru_mts::{app::build_router_with_config, config::AppConfig, state::AppState};
use http_body_util::BodyExt;
use tempfile::TempDir;
use tower::ServiceExt;

const IMMUTABLE_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

fn web_config(web_dist_dir: &Path) -> AppConfig {
    let mut config = AppConfig::from_iter(std::iter::empty::<(&str, &str)>()).unwrap();
    config.web_dist_dir = web_dist_dir.to_owned();
    config
}

fn synthetic_dist() -> TempDir {
    let dist = tempfile::tempdir().unwrap();
    fs::create_dir(dist.path().join("assets")).unwrap();
    fs::write(dist.path().join("index.html"), "<main>VDome</main>").unwrap();
    fs::write(
        dist.path().join("assets/app-a1b2c3d4.js"),
        "console.log('VDome');",
    )
    .unwrap();
    dist
}

#[tokio::test]
async fn serves_the_synthetic_index_without_caching() {
    let dist = synthetic_dist();
    let response = build_router_with_config(AppState::default(), &web_config(dist.path()))
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
    assert_eq!(
        response.into_body().collect().await.unwrap().to_bytes(),
        "<main>VDome</main>"
    );
}

#[tokio::test]
async fn serves_hashed_assets_with_their_mime_type_and_immutable_caching() {
    let dist = synthetic_dist();
    let response = build_router_with_config(AppState::default(), &web_config(dist.path()))
        .oneshot(
            Request::get("/assets/app-a1b2c3d4.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "text/javascript");
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        IMMUTABLE_CACHE_CONTROL
    );
}

#[tokio::test]
async fn encoded_index_alias_has_the_index_cache_policy() {
    let dist = synthetic_dist();
    let response = build_router_with_config(AppState::default(), &web_config(dist.path()))
        .oneshot(Request::get("/%69ndex.html").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
}

#[tokio::test]
async fn encoded_hashed_asset_alias_has_the_asset_cache_policy() {
    let dist = synthetic_dist();
    let response = build_router_with_config(AppState::default(), &web_config(dist.path()))
        .oneshot(
            Request::get("/%61ssets/app-a1b2c3d4.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        IMMUTABLE_CACHE_CONTROL
    );
}

#[tokio::test]
async fn non_api_get_routes_fall_back_to_the_spa_index() {
    let dist = synthetic_dist();
    let response = build_router_with_config(AppState::default(), &web_config(dist.path()))
        .oneshot(Request::get("/devices/42").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
    assert_eq!(
        response.into_body().collect().await.unwrap().to_bytes(),
        "<main>VDome</main>"
    );
}

#[tokio::test]
async fn api_404_is_json_instead_of_the_spa_index() {
    let dist = synthetic_dist();
    let response = build_router_with_config(AppState::default(), &web_config(dist.path()))
        .oneshot(Request::get("/api/missing").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(
        response.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("application/json")
    );
    assert!(
        String::from_utf8(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec()
        )
        .unwrap()
        .contains("not_found")
    );
}

#[tokio::test]
async fn api_namespace_is_an_exact_decoded_path_segment_for_all_methods() {
    let dist = synthetic_dist();
    let app = build_router_with_config(AppState::default(), &web_config(dist.path()));

    for method in [Method::GET, Method::POST, Method::HEAD] {
        for path in [
            "/api",
            "/api/",
            "/%61pi/missing",
            "/api%2fmissing",
            "/%61pi%2fmissing",
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method.clone())
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_json_not_found(response, &method, path).await;
        }
    }
}

#[tokio::test]
async fn apiary_is_a_web_path_only_for_get_requests() {
    let dist = synthetic_dist();
    let app = build_router_with_config(AppState::default(), &web_config(dist.path()));

    let get_response = app
        .clone()
        .oneshot(Request::get("/apiary").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(get_response.status(), StatusCode::OK);
    assert_eq!(get_response.headers()[header::CACHE_CONTROL], "no-cache");

    for method in [Method::POST, Method::HEAD] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method.clone())
                    .uri("/apiary")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} /apiary");
        assert_ne!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&"application/json".parse().unwrap()),
            "{method} /apiary"
        );
    }
}

#[tokio::test]
async fn traversal_requests_are_rejected() {
    let dist = synthetic_dist();
    let response = build_router_with_config(AppState::default(), &web_config(dist.path()))
        .oneshot(
            Request::get("/assets/%2e%2e%2findex.html")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_escaping_the_build_root_are_rejected() {
    use std::os::unix::fs::symlink;

    let dist = synthetic_dist();
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("secret.js");
    fs::write(&secret, "not a web asset").unwrap();
    symlink(&secret, dist.path().join("assets/escape-a1b2c3d4.js")).unwrap();

    let response = build_router_with_config(AppState::default(), &web_config(dist.path()))
        .oneshot(
            Request::get("/assets/escape-a1b2c3d4.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[cfg(unix)]
#[tokio::test]
async fn source_tree_symlink_swap_cannot_change_the_served_snapshot() {
    use std::os::unix::fs::symlink;

    let dist = synthetic_dist();
    let asset = dist.path().join("assets/app-a1b2c3d4.js");
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("secret.js");
    fs::write(&secret, "outside secret").unwrap();
    let app = build_router_with_config(AppState::default(), &web_config(dist.path()));

    fs::remove_file(&asset).unwrap();
    symlink(&secret, &asset).unwrap();

    let response = app
        .oneshot(
            Request::get("/assets/app-a1b2c3d4.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.into_body().collect().await.unwrap().to_bytes(),
        "console.log('VDome');"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn fifo_in_build_tree_is_rejected_without_blocking_startup() {
    use std::process::Command;

    let dist = synthetic_dist();
    let fifo = dist.path().join("assets/blocked.pipe");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );

    let response = build_router_with_config(AppState::default(), &web_config(dist.path()))
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

async fn assert_json_not_found(response: Response<Body>, method: &Method, path: &str) {
    assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {path}");
    assert!(
        response.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("application/json"),
        "{method} {path}"
    );
}
