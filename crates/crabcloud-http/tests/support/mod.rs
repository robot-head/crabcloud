//! Shared test fixtures for the crabcloud-http integration tests.
//!
//! Consolidates the helpers that several test files previously carried
//! inline (`make_state`, `seed_user`, `seed_folder`, `apply_dir`,
//! `seed_file`, `seed_zip_tree`, `create_link`, `bearer`). Per-suite
//! helpers (e.g. the public-DAV `basic_auth` builder or
//! `create_link_expired`) stay inline next to their callers.

#![allow(dead_code)]

use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_core::{AppState, AppStateBuilder};
use crabcloud_filecache::DIRECTORY_MIMETYPE;
use crabcloud_storage::{
    ETag, FileKind, FileMetadata, Mimetype, NoopEventSink, Permissions, StorageEvent, StoragePath,
};
use crabcloud_users::{AuthTokenType, BcryptVerifier, PasswordVerifier, User, UserId};
use std::pin::Pin;
use std::time::{Duration, UNIX_EPOCH};
use tokio::io::AsyncRead;

/// Build an `AppState` backed by a fresh sqlite DB at `db` with `data`
/// as its `datadirectory`. The filecache scanner is disabled — under
/// `cargo test --workspace` on Linux CI the scanner's async event-apply
/// races our handler's follow-up `view.stat` calls. Same workaround
/// Batches C–F apply in their test helpers.
pub async fn make_state(db: std::path::PathBuf, data: std::path::PathBuf) -> AppState {
    let mut cfg = minimal_sqlite_config(db);
    cfg.datadirectory = data;
    cfg.filecache.enabled = false;
    AppStateBuilder::new(cfg).build().await.unwrap()
}

/// Create a user with a bcrypt-hashed `hunter2` password.
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
/// `Shares::create_link` and the handlers can locate it. Passing the
/// root (`""` or `"/"`) seeds only the home root. Idempotent.
pub async fn seed_folder(state: &AppState, uid: &str, path: &str) {
    let storage = state
        .storage_factory
        .home_storage(&UserId::new(uid).unwrap())
        .await
        .unwrap();
    let storage_id = storage.id().to_string();
    apply_dir(state, &storage_id, &StoragePath::root()).await;
    let stripped = path.trim_start_matches('/').trim_end_matches('/');
    if stripped.is_empty() {
        return;
    }
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

/// Idempotently apply a `DirCreated` event into the filecache so that
/// `View::stat` can find the seeded directory.
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

/// Write a small file under `uid`'s home on disk + filecache so a GET /
/// PROPFIND / download can serve it back.
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
    let ev = StorageEvent::Written {
        storage_id,
        path: sp,
        metadata: meta,
    };
    state.filecache.apply(&ev).await.unwrap();
}

/// Seed `<root>/cat.txt`, `<root>/dog.txt`, and
/// `<root>/vacation/beach.txt` under `uid`'s home with deterministic
/// byte contents. Used by both the authed and public-link zip tests so
/// the resulting archive carries multiple entries and at least one
/// nested folder.
pub async fn seed_zip_tree(state: &AppState, uid: &str, root: &str) {
    seed_folder(state, uid, root).await;
    seed_folder(state, uid, &format!("{root}/vacation")).await;
    seed_file(state, uid, &format!("{root}/cat.txt"), b"cat-text").await;
    seed_file(state, uid, &format!("{root}/dog.txt"), b"dog-text").await;
    seed_file(
        state,
        uid,
        &format!("{root}/vacation/beach.txt"),
        b"beach-text-bytes",
    )
    .await;
}

/// Create a public-link share via `Shares::create` directly. Sidesteps
/// the OCS handler so tests can exercise password and expiration paths
/// regardless of which OCS surface is wired up. Returns the 15-char
/// token.
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

/// Mint a session-typed app-password for `uid` and return the raw
/// token. Callers wrap it in `format!("Bearer {token}")` for the
/// `Authorization` header.
pub async fn bearer(state: &AppState, uid: &str) -> String {
    let ap = state.users.app_passwords().unwrap().clone();
    let (_row, raw) = ap
        .mint(
            &UserId::new(uid).unwrap(),
            uid,
            "TEST",
            AuthTokenType::Session,
            false,
        )
        .await
        .unwrap();
    raw.expose().to_string()
}
