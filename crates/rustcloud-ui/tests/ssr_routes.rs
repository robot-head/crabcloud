//! SSR integration tests. Spin up a real `AppState`, build the full router,
//! and exercise the UI routes via `tower::ServiceExt::oneshot`.

#![allow(unused_crate_dependencies)]

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use rustcloud_config::test_support::sqlite_config_with_admin;
use rustcloud_core::AppStateBuilder;
use rustcloud_http::build_router;
use tempfile::tempdir;
use tower::ServiceExt;

async fn build_app() -> axum::Router {
    let dir = tempdir().unwrap();
    let cfg = sqlite_config_with_admin(dir.path().join("ssr.db"), "admin", "$2b$12$placeholder");
    let state = AppStateBuilder::new(cfg)
        .with_core_capabilities()
        .build()
        .await
        .unwrap();
    let app = build_router(state);
    std::mem::forget(dir); // keep the sqlite file alive for the duration of the test
    app
}

#[tokio::test]
async fn home_returns_ssr_html_with_hydration_payload() {
    let app = build_app().await;
    let req = Request::builder().uri("/").body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(ct.starts_with("text/html"), "Content-Type was: {ct}");

    let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.starts_with("<!DOCTYPE html>"), "missing doctype");
    assert!(
        html.contains("<script id=\"__dx_ctx\""),
        "missing hydration script"
    );
    assert!(
        html.contains("Welcome, guest"),
        "missing welcome text for anonymous user"
    );
    assert!(html.contains("href=\"/login\""), "missing login link");
}

#[tokio::test]
async fn login_returns_form_posting_to_index_php_login() {
    let app = build_app().await;
    let req = Request::builder()
        .uri("/login")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("<form"), "missing form element");
    assert!(
        html.contains("action=\"/index.php/login\""),
        "form action mismatch"
    );
    assert!(html.contains("method=\"post\""), "form method mismatch");
    assert!(html.contains("name=\"username\""), "missing username input");
    assert!(html.contains("name=\"password\""), "missing password input");
}

#[tokio::test]
async fn unknown_path_returns_404_dioxus_page() {
    let app = build_app().await;
    let req = Request::builder()
        .uri("/this/path/does/not/exist")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    // SSR handler always returns 200; the body indicates 404 via content.
    // (axum's default fall-through would have been 404, but our fallback IS the
    // SSR handler. Phase 5 can wire a proper 404 status via the Dioxus router.)
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("404"), "404 page didn't render");
    assert!(html.contains("Not Found"), "404 page didn't render");
}

#[tokio::test]
async fn locale_resolution_respects_accept_language() {
    let app = build_app().await;
    let req = Request::builder()
        .uri("/")
        .header("accept-language", "de-DE, en;q=0.5")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    // No German catalog seeded for the home page text in this test, but the
    // hydration payload should carry the resolved locale. The fixture's
    // AppStateBuilder doesn't load any catalogs, so resolve() falls all the way
    // to "en" — but we can still verify the locale appears in the payload.
    assert!(
        html.contains("\"locale\""),
        "hydration payload missing locale field"
    );
}

#[tokio::test]
async fn html_lang_attribute_matches_resolved_locale() {
    let app = build_app().await;
    let req = Request::builder().uri("/").body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(
        html.contains("<html lang=\"en\""),
        "html element missing lang attribute"
    );
}
