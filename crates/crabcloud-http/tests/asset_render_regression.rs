//! Regression guard for the dx 0.7.9 placeholder-leak bug. The SSR'd HTML's
//! `<link rel="stylesheet">` href must be a hashed `/assets/<hash>.css` URL
//! produced by dx's link-time substitution — never the manganis placeholder
//! ("This should be replaced by dx as part of the build process. …") or an
//! absolute source path.
//!
//! This test is `#[ignore]`'d in the default test suite because it only
//! holds when the binary went through dx's linker. CI runs it explicitly
//! against the dx-built binary via `cargo test … -- --ignored`.

#![allow(unused_crate_dependencies)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_core::AppStateBuilder;
use dioxus::server::{DioxusRouterExt, FullstackState};
use tempfile::tempdir;
use tower::ServiceExt;

#[tokio::test]
#[ignore = "asserts dx-link-section substitution; only valid against dx-built binary"]
async fn ssr_stylesheet_href_is_a_hashed_assets_path() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let mut cfg = minimal_sqlite_config(dir.path().join("href.db"));
    cfg.datadirectory = data.path().to_path_buf();
    cfg.filecache.enabled = false;
    let state = AppStateBuilder::new(cfg).build().await.unwrap();

    let dioxus_router = axum::Router::new()
        .register_server_functions()
        .with_state(FullstackState::headless());
    let app = crabcloud_http::build_router(state, dioxus_router);

    let req = Request::builder()
        .method("GET")
        .uri("/login")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 256 * 1024)
        .await
        .unwrap();
    let html = std::str::from_utf8(&body).unwrap();

    let placeholder_marker = "This should be replaced by dx";
    assert!(
        !html.contains(placeholder_marker),
        "SSR HTML still contains the manganis placeholder; dx link substitution didn't run. Excerpt: {}",
        &html[..html.len().min(2000)]
    );

    let link_pattern =
        regex::Regex::new(r#"<link rel="stylesheet" href="/assets/[A-Za-z0-9_.-]+\.css""#).unwrap();
    assert!(
        link_pattern.is_match(html),
        "no hashed stylesheet href in SSR HTML. Excerpt: {}",
        &html[..html.len().min(2000)]
    );
}
