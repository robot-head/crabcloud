//! End-to-end tests for the authed folder-zip endpoint
//! (`GET /api/files/zip/{*path}`). Drives the full `build_router` so each
//! request runs through the real `AuthLayer` and our handler picks up the
//! `Extension<AuthContext>` via Bearer auth.

#![allow(unused_crate_dependencies)]

mod support;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_core::AppStateBuilder;
use std::io::Cursor;
use support::{bearer, make_state, seed_file, seed_folder, seed_user, seed_zip_tree};
use tempfile::tempdir;
use tower::ServiceExt;

const BODY_LIMIT: usize = 16 * 1024 * 1024;

#[tokio::test]
async fn authed_zip_returns_200_application_zip() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("ok.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_zip_tree(&state, "alice", "/Photos").await;
    let token = bearer(&state, "alice").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/files/zip/Photos")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .unwrap(),
        "application/zip"
    );
    let cd = resp
        .headers()
        .get(axum::http::header::CONTENT_DISPOSITION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(cd.contains("attachment"));
    assert!(cd.contains("filename=\"Photos.zip\""), "got: {cd}");
    let body = to_bytes(resp.into_body(), BODY_LIMIT).await.unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(body.to_vec())).unwrap();
    let mut names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();
    names.sort();
    assert!(
        names.iter().any(|n| n == "Photos/cat.txt"),
        "missing Photos/cat.txt in {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "Photos/dog.txt"),
        "missing Photos/dog.txt in {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "Photos/vacation/beach.txt"),
        "missing Photos/vacation/beach.txt in {names:?}"
    );
}

#[tokio::test]
async fn authed_zip_over_cap_returns_413_with_summary() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let mut cfg = minimal_sqlite_config(dir.path().join("cap.db"));
    cfg.datadirectory = data.path().to_path_buf();
    cfg.filecache.enabled = false;
    // Force the cap to 1 entry — the tree has 5, so the walk overflows.
    cfg.folder_zip_max_entries = 1;
    let state = AppStateBuilder::new(cfg).build().await.unwrap();
    seed_user(&state, "alice").await;
    seed_zip_tree(&state, "alice", "/Photos").await;
    let token = bearer(&state, "alice").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/files/zip/Photos")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .unwrap(),
        "application/json"
    );
    let body = to_bytes(resp.into_body(), BODY_LIMIT).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["error"], "folder too large");
    assert!(v["entries"].as_u64().unwrap() >= 2);
    assert_eq!(v["limits"]["max_entries"], 1);
    assert!(v["limits"]["max_bytes"].as_u64().is_some());
}

#[tokio::test]
async fn authed_zip_of_regular_file_returns_400() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("file.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_folder(&state, "alice", "/").await;
    seed_file(&state, "alice", "/note.txt", b"hello").await;
    let token = bearer(&state, "alice").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/files/zip/note.txt")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn authed_zip_unknown_path_returns_404() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("nx.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_folder(&state, "alice", "/").await;
    let token = bearer(&state, "alice").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/files/zip/does_not_exist")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn authed_zip_root_uses_uid_basename() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state(dir.path().join("root.db"), data.path().to_path_buf()).await;
    seed_user(&state, "alice").await;
    seed_zip_tree(&state, "alice", "/Photos").await;
    let token = bearer(&state, "alice").await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/files/zip/")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let cd = resp
        .headers()
        .get(axum::http::header::CONTENT_DISPOSITION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(cd.contains("filename=\"alice.zip\""), "got: {cd}");
}
