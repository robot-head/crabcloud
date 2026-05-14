//! End-to-end HTTP flow for the OCS surface and the shared middleware stack.
//!
//! `/status.php` and `/index.php/login` migrated to Dioxus `#[server]`
//! functions in `crabcloud-app` and are now exercised by the Playwright suite;
//! this test focuses on what `build_router` still serves directly (the OCS
//! REST routes) plus the shared headers/security stack.

#![allow(unused_crate_dependencies)]

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::Router;
use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_core::AppStateBuilder;
use crabcloud_http::build_router;
use tempfile::tempdir;
use tower::ServiceExt;

#[tokio::test]
async fn ocs_capabilities_and_security_headers() {
    let dir = tempdir().unwrap();
    let state = AppStateBuilder::new(minimal_sqlite_config(dir.path().join("e2e.db")))
        .with_core_capabilities()
        .build()
        .await
        .unwrap();
    // `build_router` now expects a Dioxus fullstack router to merge with;
    // for OCS-only tests an empty router stands in.
    let app = build_router(state, Router::new());

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/ocs/v2.php/cloud/capabilities?format=json")
                .header("ocs-apirequest", "true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 32 * 1024).await.unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed["ocs"]["meta"]["statuscode"], 200);
    assert_eq!(
        parsed["ocs"]["data"]["capabilities"]["core"]["pollinterval"],
        60
    );
    assert_eq!(parsed["ocs"]["data"]["version"]["major"], 31);

    // Security headers attached by the shared middleware stack.
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/ocs/v2.php/cloud/capabilities?format=json")
                .header("ocs-apirequest", "true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let h = resp.headers();
    assert!(h.get("strict-transport-security").is_some(), "HSTS missing");
    assert!(h.get("x-content-type-options").is_some(), "XCTO missing");
    assert!(h.get("content-security-policy").is_some(), "CSP missing");
}
