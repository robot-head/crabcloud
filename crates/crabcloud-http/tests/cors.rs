#![allow(unused_crate_dependencies)]

use axum::body::Body;
use axum::http::{Method, Request};
use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_config::FileConfig;
use crabcloud_core::AppStateBuilder;
use crabcloud_http::build_router;
use std::path::PathBuf;
use tempfile::tempdir;
use tower::ServiceExt;

fn cfg(path: PathBuf) -> FileConfig {
    let mut cfg = minimal_sqlite_config(path);
    cfg.trusted_domains = vec!["cloud.example.com".into()];
    cfg
}

#[tokio::test]
async fn cors_allows_both_http_and_https_origins_for_trusted_domain() {
    let dir = tempdir().unwrap();
    let state = AppStateBuilder::new(cfg(dir.path().join("cors.db")))
        .build()
        .await
        .unwrap();
    let app = build_router(state);

    for scheme in ["http", "https"] {
        let origin = format!("{scheme}://cloud.example.com");
        let req = Request::builder()
            .method(Method::OPTIONS)
            .uri("/status.php")
            .header("origin", &origin)
            .header("access-control-request-method", "GET")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let acao = resp.headers().get("access-control-allow-origin");
        assert!(
            acao.is_some(),
            "missing access-control-allow-origin for Origin: {origin}"
        );
        assert_eq!(acao.unwrap().to_str().unwrap(), origin);
    }
}

#[tokio::test]
async fn cors_preflight_response_has_security_headers() {
    let dir = tempdir().unwrap();
    let state = AppStateBuilder::new(cfg(dir.path().join("cors2.db")))
        .build()
        .await
        .unwrap();
    let app = build_router(state);

    let req = Request::builder()
        .method(Method::OPTIONS)
        .uri("/status.php")
        .header("origin", "http://cloud.example.com")
        .header("access-control-request-method", "GET")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    // Security headers must be present even on CORS short-circuit responses.
    let h = resp.headers();
    assert!(
        h.get("strict-transport-security").is_some(),
        "HSTS missing on preflight"
    );
    assert!(
        h.get("x-content-type-options").is_some(),
        "XCTO missing on preflight"
    );
    assert!(
        h.get("content-security-policy").is_some(),
        "CSP missing on preflight"
    );
}
