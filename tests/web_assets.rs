use std::{fs, path::Path};

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use domru_mts::{app::build_router_with_config, config::AppConfig, state::AppState};
use http_body_util::BodyExt;
use tempfile::TempDir;
use tower::ServiceExt;

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
        "public, max-age=31536000, immutable"
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
