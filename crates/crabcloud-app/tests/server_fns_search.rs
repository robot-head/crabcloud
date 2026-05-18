//! HTTP-level integration tests for the search server fn
//! (`/api/files/search`). Mirrors the `server_fns_activity.rs` scaffold
//! — drives the full `build_router` stack so requests travel through
//! the production auth middleware and the dx fullstack server-fn
//! handler.
//!
//! Search rows are seeded by calling `Search::upsert_for_file` directly
//! (same pattern as the OCS `ocs_search.rs` suite). The wider indexer
//! + share-fan-out semantics are exercised by the `crabcloud-search`
//! unit / e2e suites — these tests stay focused on the server-fn wire
//! surface (auth gating, DTO shape, empty-query short-circuit, cursor
//! plumbing).

#![allow(unused_crate_dependencies)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_core::{AppState, AppStateBuilder};
use crabcloud_users::{AuthTokenType, BcryptVerifier, PasswordVerifier, User, UserId};
use dioxus::server::{DioxusRouterExt, FullstackState};
use tempfile::tempdir;
use tower::ServiceExt;

async fn make_state_with_user(db: std::path::PathBuf, data: std::path::PathBuf) -> AppState {
    let mut cfg = minimal_sqlite_config(db);
    cfg.datadirectory = data;
    cfg.filecache.enabled = false;
    // `minimal_sqlite_config` defaults `search_indexer_enabled = false`
    // so the direct `upsert_for_file` seeds below are the sole source
    // of truth for these wire-surface tests.
    let state = AppStateBuilder::new(cfg).build().await.unwrap();
    let hash = BcryptVerifier::new().hash("hunter2").unwrap();
    state
        .users
        .user_store()
        .create(
            &User {
                uid: UserId::new("alice").unwrap(),
                display_name: "Alice".into(),
                email: None,
                enabled: true,
                last_seen: 0,
            },
            Some(&hash),
        )
        .await
        .unwrap();
    state
}

async fn bearer_for(state: &AppState, uid: &str) -> String {
    let ap = state.users.app_passwords().unwrap().clone();
    let (_row, raw) = ap
        .mint(
            &UserId::new(uid).unwrap(),
            uid,
            "UI",
            AuthTokenType::Session,
            false,
        )
        .await
        .unwrap();
    raw.expose().to_string()
}

fn build_app(state: AppState) -> axum::Router {
    let dioxus_router = axum::Router::new()
        .register_server_functions()
        .with_state(FullstackState::headless());
    crabcloud_http::build_router(state, dioxus_router)
}

/// Seed one search row for `viewer` against `(fileid, basename, path,
/// mime, size)`. Bypasses the indexer + filecache plumbing — these
/// tests only care about the server-fn wire surface; cross-crate
/// indexer integration is covered by `crabcloud-search`'s own e2e
/// suite.
async fn seed_search_row(
    state: &AppState,
    viewer: &str,
    fileid: i64,
    basename: &str,
    path: &str,
    mime: &str,
    size: i64,
) {
    state
        .search
        .upsert_for_file(
            viewer,
            fileid,
            "local::test",
            basename,
            path,
            mime,
            1_700_000_000,
            size,
        )
        .await
        .expect("upsert");
}

async fn post_json(
    app: &axum::Router,
    token: &str,
    uri: &str,
    body: serde_json::Value,
) -> axum::response::Response {
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("ocs-apirequest", "true")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap()
}

async fn decode_bytes(resp: axum::response::Response) -> (StatusCode, Vec<u8>) {
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap()
        .to_vec();
    (status, bytes)
}

#[tokio::test]
async fn search_files_empty_query_returns_empty_hits() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("e.db"), data.path().to_path_buf()).await;
    let token = bearer_for(&state, "alice").await;
    // Seed a row so we can prove the empty-query path doesn't surface
    // it — the short-circuit returns before touching the index.
    seed_search_row(
        &state,
        "alice",
        1,
        "report.docx",
        "/docs/report.docx",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        12345,
    )
    .await;
    let app = build_app(state);

    let resp = post_json(
        &app,
        &token,
        "/api/files/search",
        serde_json::json!({ "query": "" }),
    )
    .await;
    let (status, body) = decode_bytes(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "body={:?}",
        String::from_utf8_lossy(&body)
    );
    let resp: crabcloud_app::SearchResponseDto =
        serde_json::from_slice(&body).unwrap_or_else(|e| {
            panic!(
                "decode SearchResponseDto: {e} body={:?}",
                String::from_utf8_lossy(&body)
            )
        });
    assert!(resp.hits.is_empty(), "{:?}", resp.hits);
    assert!(resp.cursor.is_none(), "{:?}", resp.cursor);
}

#[tokio::test]
async fn search_files_whitespace_only_query_returns_empty_hits() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("ws.db"), data.path().to_path_buf()).await;
    let token = bearer_for(&state, "alice").await;
    seed_search_row(
        &state,
        "alice",
        1,
        "report.docx",
        "/docs/report.docx",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        12345,
    )
    .await;
    let app = build_app(state);

    let resp = post_json(
        &app,
        &token,
        "/api/files/search",
        serde_json::json!({ "query": "   " }),
    )
    .await;
    let (status, body) = decode_bytes(resp).await;
    assert_eq!(status, StatusCode::OK);
    let resp: crabcloud_app::SearchResponseDto = serde_json::from_slice(&body).unwrap();
    assert!(resp.hits.is_empty(), "{:?}", resp.hits);
}

#[tokio::test]
async fn search_files_returns_seeded_hit_with_dto_shape() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("h.db"), data.path().to_path_buf()).await;
    let token = bearer_for(&state, "alice").await;
    seed_search_row(
        &state,
        "alice",
        42,
        "vacation.jpg",
        "/photos/vacation.jpg",
        "image/jpeg",
        2_000_000,
    )
    .await;
    let app = build_app(state);

    let resp = post_json(
        &app,
        &token,
        "/api/files/search",
        serde_json::json!({ "query": "vacation" }),
    )
    .await;
    let (status, body) = decode_bytes(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "body={:?}",
        String::from_utf8_lossy(&body)
    );
    let resp: crabcloud_app::SearchResponseDto = serde_json::from_slice(&body).unwrap();
    assert_eq!(resp.hits.len(), 1, "{:?}", resp.hits);
    let hit = &resp.hits[0];
    assert_eq!(hit.fileid, 42);
    assert_eq!(hit.basename, "vacation.jpg");
    assert_eq!(hit.path, "/photos/vacation.jpg");
    assert_eq!(hit.mime, "image/jpeg");
    assert_eq!(hit.size, 2_000_000);
    assert_eq!(hit.mtime, 1_700_000_000);
    // Single hit below the 10-row page limit — no next-cursor.
    assert!(resp.cursor.is_none(), "{:?}", resp.cursor);
}

#[tokio::test]
async fn search_files_isolates_results_per_viewer() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("iso.db"), data.path().to_path_buf()).await;
    let token = bearer_for(&state, "alice").await;
    // Bob's row should not leak into Alice's query — the search index
    // is per-viewer materialized.
    seed_search_row(
        &state,
        "bob",
        99,
        "secret.txt",
        "/private/secret.txt",
        "text/plain",
        100,
    )
    .await;
    let app = build_app(state);

    let resp = post_json(
        &app,
        &token,
        "/api/files/search",
        serde_json::json!({ "query": "secret" }),
    )
    .await;
    let (status, body) = decode_bytes(resp).await;
    assert_eq!(status, StatusCode::OK);
    let resp: crabcloud_app::SearchResponseDto = serde_json::from_slice(&body).unwrap();
    assert!(
        resp.hits.is_empty(),
        "alice should not see bob's row: {:?}",
        resp.hits
    );
}

#[tokio::test]
async fn search_files_unauthenticated_returns_non_ok() {
    // Same contract as the activity / versions suites: AuthLayer only
    // 401s when an auth header is present-but-invalid. With no auth at
    // all the request falls through anonymous; the server fn body
    // returns `unauthorized` (mapped to 500). Either way it's not 200.
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("u.db"), data.path().to_path_buf()).await;
    let app = build_app(state);

    let req = Request::builder()
        .method("POST")
        .uri("/api/files/search")
        .header("ocs-apirequest", "true")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"query":"anything"}"#))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_ne!(resp.status(), StatusCode::OK);
}
