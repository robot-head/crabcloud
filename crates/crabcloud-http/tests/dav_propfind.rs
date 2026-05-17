//! Integration tests for Batch D: PROPFIND.
//!
//! Body assertions use substring matching against the rendered XML. Quick
//! and robust enough for SP5; future hardening can swap to `quick_xml::reader`
//! once the response shape stabilises. Setup helpers mirror `dav_basic.rs`;
//! lifting into `tests/support/` is acknowledged in the plan as follow-up.

#![allow(unused_crate_dependencies)]

mod support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use crabcloud_core::AppState;
use support::{bearer, make_state, seed_user};
use tempfile::tempdir;
use tower::ServiceExt;

/// Build state + seed `alice`. Thin wrapper over the shared `make_state` +
/// `seed_user` so the existing call sites stay one-liners.
async fn make_state_with_user(db: std::path::PathBuf, data: std::path::PathBuf) -> AppState {
    let state = make_state(db, data).await;
    seed_user(&state, "alice").await;
    state
}

/// Mint a Bearer token for `alice` against the live token store.
async fn alice_bearer(state: &AppState) -> String {
    bearer(state, "alice").await
}

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[tokio::test]
async fn propfind_depth_0_returns_resource() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("p0.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    // Seed a file via PUT so the resource exists in the cache.
    let put = Request::builder()
        .method("PUT")
        .uri("/dav/files/alice/file.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from("hello"))
        .unwrap();
    let resp = app.clone().oneshot(put).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = Request::builder()
        .method("PROPFIND")
        .uri("/dav/files/alice/file.txt")
        .header("authorization", format!("Bearer {token}"))
        .header("depth", "0")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 207);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(ct.contains("xml"), "expected xml content-type, got {ct}");
    let body = body_string(resp).await;

    // Exactly one <d:response> block.
    assert_eq!(body.matches("<d:response>").count(), 1, "body: {body}");

    // The 10-prop set surfaces — substring assertion per prop name.
    for prop in [
        "d:getcontentlength",
        "d:getcontenttype",
        "d:getetag",
        "d:getlastmodified",
        "d:resourcetype",
        "d:displayname",
        "oc:id",
        "oc:permissions",
        "oc:size",
        "oc:favorite",
    ] {
        assert!(body.contains(prop), "missing {prop} in: {body}");
    }

    // The href reflects the legacy prefix the handler emits.
    assert!(
        body.contains("/remote.php/dav/files/alice/file.txt"),
        "href missing: {body}"
    );
    // Favorite defaults to "0" when no oc_properties row exists.
    assert!(
        body.contains("<oc:favorite>0</oc:favorite>"),
        "default favorite missing: {body}"
    );
}

#[tokio::test]
async fn propfind_depth_1_returns_children() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("p1.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    // Seed three children at the user's root.
    for name in ["a.txt", "b.txt", "c.txt"] {
        let req = Request::builder()
            .method("PUT")
            .uri(format!("/dav/files/alice/{name}"))
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from("x"))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED, "PUT {name} failed");
    }

    let req = Request::builder()
        .method("PROPFIND")
        .uri("/dav/files/alice")
        .header("authorization", format!("Bearer {token}"))
        .header("depth", "1")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 207);
    let body = body_string(resp).await;

    // The collection (alice's root) + 3 children = 4 response blocks.
    assert_eq!(body.matches("<d:response>").count(), 4, "body: {body}");
    for child in ["a.txt", "b.txt", "c.txt"] {
        assert!(body.contains(child), "missing child {child} in: {body}");
    }
    // The root entry must be marked as a collection.
    assert!(
        body.contains("<d:collection"),
        "missing d:collection in: {body}"
    );
}

#[tokio::test]
async fn propfind_depth_infinity_returns_403() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("pinf.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("PROPFIND")
        .uri("/dav/files/alice")
        .header("authorization", format!("Bearer {token}"))
        .header("depth", "infinity")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = body_string(resp).await;
    assert!(
        body.contains("propfind-finite-depth"),
        "missing propfind-finite-depth marker: {body}"
    );
}
