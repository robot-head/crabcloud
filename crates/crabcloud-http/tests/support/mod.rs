//! Shared test fixtures for the public-link integration tests
//! (`public_link_e2e.rs` and `public_dav_e2e.rs`).
//!
//! Keeps the surface intentionally minimal: only the six helpers needed by
//! both suites — `make_state`, `seed_user`, `seed_folder`, `apply_dir`,
//! `seed_file`, and `create_link`. Per-suite helpers (e.g. the DAV
//! `basic_auth` builder) stay inline next to their callers.

#![allow(dead_code)]

use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_core::{AppState, AppStateBuilder};
use crabcloud_filecache::DIRECTORY_MIMETYPE;
use crabcloud_storage::{
    ETag, FileKind, FileMetadata, Mimetype, NoopEventSink, Permissions, StorageEvent, StoragePath,
};
use crabcloud_users::{BcryptVerifier, PasswordVerifier, User, UserId};
use std::pin::Pin;
use std::time::{Duration, UNIX_EPOCH};
use tokio::io::AsyncRead;

pub async fn make_state(db: std::path::PathBuf, data: std::path::PathBuf) -> AppState {
    let mut cfg = minimal_sqlite_config(db);
    cfg.datadirectory = data;
    // Disable the filecache scanner: under `cargo test --workspace` on
    // Linux CI the scanner's async event-apply races our handler's
    // follow-up `view.stat` calls. Matches the workaround in
    // `dav_basic.rs`.
    cfg.filecache.enabled = false;
    AppStateBuilder::new(cfg).build().await.unwrap()
}

pub async fn seed_user(state: &AppState, uid: &str) {
    let hash = BcryptVerifier::new().hash("hunter2").unwrap();
    state
        .users
        .user_store()
        .create(
            &User {
                uid: UserId::new(uid).unwrap(),
                display_name: format!("{uid} display"),
                email: None,
                enabled: true,
                last_seen: 0,
            },
            Some(&hash),
        )
        .await
        .unwrap();
}

/// Materialise a folder under `uid`'s home on disk and ensure the
/// filecache has the chain `/`, `/seg1`, `/seg1/seg2`, … so
/// `Shares::create_link` can locate it. Idempotent.
pub async fn seed_folder(state: &AppState, uid: &str, path: &str) {
    let storage = state
        .storage_factory
        .home_storage(&UserId::new(uid).unwrap())
        .await
        .unwrap();
    let storage_id = storage.id().to_string();
    apply_dir(state, &storage_id, &StoragePath::root()).await;
    let stripped = path.trim_start_matches('/').trim_end_matches('/');
    let segments: Vec<&str> = stripped.split('/').collect();
    let mut cur = String::new();
    for seg in segments {
        if !cur.is_empty() {
            cur.push('/');
        }
        cur.push_str(seg);
        let sp = StoragePath::new(cur.clone()).unwrap();
        if !storage.exists(&sp).await.unwrap() {
            storage.mkdir(&sp, &NoopEventSink).await.unwrap();
        }
        apply_dir(state, &storage_id, &sp).await;
    }
}

pub async fn apply_dir(state: &AppState, storage_id: &str, path: &StoragePath) {
    if state
        .filecache
        .lookup(storage_id, path)
        .await
        .unwrap()
        .is_some()
    {
        return;
    }
    let md = FileMetadata {
        path: path.clone(),
        kind: FileKind::Directory,
        size: 0,
        mtime: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        etag: ETag::new(),
        mimetype: Mimetype::parse(DIRECTORY_MIMETYPE).unwrap(),
        permissions: Permissions::full(),
    };
    let ev = StorageEvent::DirCreated {
        storage_id: storage_id.into(),
        path: path.clone(),
        metadata: md,
    };
    state.filecache.apply(&ev).await.unwrap();
}

/// Write a small file under `uid`'s home on disk + filecache so the
/// public link's download / GET / PROPFIND can serve it back.
pub async fn seed_file(state: &AppState, uid: &str, path: &str, body: &[u8]) {
    let storage = state
        .storage_factory
        .home_storage(&UserId::new(uid).unwrap())
        .await
        .unwrap();
    let sp = StoragePath::new(path.trim_start_matches('/').to_string()).unwrap();
    let bytes = body.to_vec();
    let reader: Pin<Box<dyn AsyncRead + Send>> = Box::pin(std::io::Cursor::new(bytes));
    let meta = storage.put_file(&sp, reader, &NoopEventSink).await.unwrap();
    let storage_id = storage.id().to_string();
    // Mirror the storage `put_file` into the filecache so `View::stat`
    // finds it.
    let ev = StorageEvent::Written {
        storage_id: storage_id.clone(),
        path: sp.clone(),
        metadata: meta,
    };
    state.filecache.apply(&ev).await.unwrap();
}

/// Create a public-link share via `Shares::create` directly. This sidesteps
/// the OCS handler (which doesn't accept password/expireDate on Batch E's
/// `sp8/e-public-surface` branch — the OCS wiring lives on the sibling
/// `sp8/e-ocs-link-shape` branch and merges separately). Returns the
/// 15-char token.
pub async fn create_link(
    state: &AppState,
    requester: &str,
    path: &str,
    permissions: u32,
    password: Option<&str>,
    expire_date: Option<chrono::NaiveDate>,
) -> String {
    use crabcloud_sharing::{CreateShareRequest, ShareType};
    let home_sid = state
        .storage_factory
        .home_storage(&UserId::new(requester).unwrap())
        .await
        .unwrap()
        .id()
        .to_string();
    let req = CreateShareRequest {
        requester: requester.to_string(),
        home_storage_id: home_sid,
        path: path.to_string(),
        share_type: ShareType::Link,
        share_with: String::new(),
        permissions,
        password: password.map(|s| s.to_string()),
        expire_date,
    };
    let row = state.shares.create(req).await.expect("create_link");
    row.token.expect("link rows carry a token")
}
