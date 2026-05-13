//! Shared test fixture for the sharing integration tests. Builds a fresh
//! migrated pool per dialect (sqlite via tempdir; mysql + postgres via the
//! `cargo xtask up` docker containers), plus a `UsersService` and a
//! `FileCache` wired against it. Seed helpers create users, groups, and
//! filecache rows so tests can call the `Shares` service directly.

#![allow(dead_code)]

use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_config::{DbType, FileConfig};
use crabcloud_db::{core_set, DbError, DbPool, MigrationRunner};
use crabcloud_filecache::{FileCache, DIRECTORY_MIMETYPE};
use crabcloud_sharing::{CreateShareRequest, ShareType, Shares};
use crabcloud_storage::{
    ETag, FileKind, FileMetadata, Mimetype, Permissions, StorageEvent, StoragePath,
};
use crabcloud_users::{
    BcryptVerifier, Group, GroupId, SqlGroupStore, SqlPreferenceStore, SqlUserStore, User, UserId,
    UsersService,
};
use secrecy::SecretString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};
use tempfile::TempDir;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum FixtureKind {
    Sqlite,
    MySql,
    Postgres,
}

pub struct Fixture {
    pub pool: Arc<DbPool>,
    pub users: Arc<UsersService>,
    pub filecache: Arc<FileCache>,
    pub shares: Shares,
    // Hold the tempdir alive for sqlite-backed fixtures so the file lasts.
    _tempdir: Option<TempDir>,
}

impl Fixture {
    pub async fn new(kind: FixtureKind) -> Self {
        let (pool, tempdir) = match kind {
            FixtureKind::Sqlite => {
                let dir = tempfile::tempdir().unwrap();
                let cfg = minimal_sqlite_config(dir.path().join("sharing.db"));
                let pool = DbPool::connect(&cfg).await.unwrap();
                run_migrations(&pool, &cfg).await;
                (pool, Some(dir))
            }
            FixtureKind::MySql => {
                let url = std::env::var("CRABCLOUD_TEST_MYSQL_URL").unwrap_or_else(|_| {
                    "mysql://crabcloud:crabcloud@127.0.0.1:3307/crabcloud".into()
                });
                let cfg = mysql_config_from_url(&url);
                let pool = DbPool::connect(&cfg).await.unwrap();
                reset_external(&pool).await;
                run_migrations(&pool, &cfg).await;
                (pool, None)
            }
            FixtureKind::Postgres => {
                let url = std::env::var("CRABCLOUD_TEST_POSTGRES_URL").unwrap_or_else(|_| {
                    "postgres://crabcloud:crabcloud@127.0.0.1:5433/crabcloud".into()
                });
                let cfg = postgres_config_from_url(&url);
                let pool = DbPool::connect(&cfg).await.unwrap();
                reset_external(&pool).await;
                run_migrations(&pool, &cfg).await;
                (pool, None)
            }
        };
        let pool_arc = Arc::new(pool);
        let users: Arc<UsersService> = Arc::new(UsersService::new(
            Arc::new(SqlUserStore::new((*pool_arc).clone())),
            Arc::new(SqlGroupStore::new((*pool_arc).clone())),
            Arc::new(SqlPreferenceStore::new((*pool_arc).clone())),
            Arc::new(BcryptVerifier::new()),
        ));
        let filecache = Arc::new(FileCache::new((*pool_arc).clone()));
        let shares = Shares::new(pool_arc.clone(), users.clone(), filecache.clone());
        Self {
            pool: pool_arc,
            users,
            filecache,
            shares,
            _tempdir: tempdir,
        }
    }

    pub fn home_storage_id(&self, uid: &str) -> String {
        format!("home::{uid}")
    }
}

async fn run_migrations(pool: &DbPool, cfg: &FileConfig) {
    let mut runner = MigrationRunner::new(pool, &cfg.dbtableprefix);
    runner.register(core_set());
    runner.run().await.unwrap();
}

async fn reset_external(pool: &DbPool) {
    let tables = [
        "oc_share",
        "oc_properties",
        "oc_filelocks",
        "oc_filecache",
        "oc_mimetypes",
        "oc_storages",
        "oc_authtoken",
        "oc_preferences",
        "oc_group_user",
        "oc_groups",
        "oc_users",
        "oc_appconfig",
        "oc_migrations",
    ];
    match pool {
        DbPool::MySql(p) => {
            for t in tables {
                let _ = sqlx::query(&format!("DROP TABLE IF EXISTS {t}"))
                    .execute(p)
                    .await;
            }
        }
        DbPool::Postgres(p) => {
            for t in tables {
                let _ = sqlx::query(&format!("DROP TABLE IF EXISTS {t}"))
                    .execute(p)
                    .await;
            }
        }
        DbPool::Sqlite(_) => {}
    }
}

pub async fn seed_user(fx: &Fixture, uid: &str) {
    let user = User {
        uid: UserId::new(uid).unwrap(),
        display_name: uid.to_string(),
        email: None,
        enabled: true,
        last_seen: 0,
    };
    fx.users.user_store().create(&user, None).await.unwrap();
}

pub async fn seed_group(fx: &Fixture, gid: &str) {
    let g = Group {
        gid: GroupId::new(gid).unwrap(),
        display_name: gid.to_string(),
    };
    fx.users.group_store().create(&g).await.unwrap();
}

pub async fn add_user_to_group(fx: &Fixture, uid: &str, gid: &str) {
    fx.users
        .group_store()
        .add_to_group(&UserId::new(uid).unwrap(), &GroupId::new(gid).unwrap())
        .await
        .unwrap();
}

/// Seed `path` in `uid`'s home filecache. Creates the root row first if it
/// is not already present (idempotent), then any intermediate directories,
/// then the leaf. Returns the leaf row's `fileid`.
pub async fn seed_file(fx: &Fixture, uid: &str, path: &str, is_dir: bool) -> i64 {
    let storage_id = fx.home_storage_id(uid);
    seed_dir_if_missing(fx, &storage_id, &StoragePath::root()).await;

    // Build each intermediate directory in order so the leaf has an ancestor.
    let stripped = path.trim_start_matches('/').trim_end_matches('/');
    let segments: Vec<&str> = stripped.split('/').collect();
    let mut cur = String::new();
    for (i, seg) in segments.iter().enumerate() {
        if !cur.is_empty() {
            cur.push('/');
        }
        cur.push_str(seg);
        let is_leaf = i == segments.len() - 1;
        let sp = StoragePath::new(cur.clone()).unwrap();
        if is_leaf {
            apply_event(
                &fx.filecache,
                &storage_id,
                &sp,
                if is_dir {
                    make_dir_meta(&sp)
                } else {
                    make_file_meta(&sp, 7, "text/plain")
                },
                is_dir,
            )
            .await;
        } else {
            apply_event(&fx.filecache, &storage_id, &sp, make_dir_meta(&sp), true).await;
        }
    }
    fx.filecache
        .lookup(&storage_id, &StoragePath::new(stripped).unwrap())
        .await
        .unwrap()
        .expect("seeded row")
        .fileid
}

async fn seed_dir_if_missing(fx: &Fixture, storage_id: &str, path: &StoragePath) {
    if fx
        .filecache
        .lookup(storage_id, path)
        .await
        .unwrap()
        .is_some()
    {
        return;
    }
    apply_event(&fx.filecache, storage_id, path, make_dir_meta(path), true).await;
}

async fn apply_event(
    cache: &FileCache,
    storage_id: &str,
    path: &StoragePath,
    md: FileMetadata,
    is_dir: bool,
) {
    let event = if is_dir {
        StorageEvent::DirCreated {
            storage_id: storage_id.into(),
            path: path.clone(),
            metadata: md,
        }
    } else {
        StorageEvent::Written {
            storage_id: storage_id.into(),
            path: path.clone(),
            metadata: md,
        }
    };
    cache.apply(&event).await.unwrap();
}

fn make_dir_meta(path: &StoragePath) -> FileMetadata {
    FileMetadata {
        path: path.clone(),
        kind: FileKind::Directory,
        size: 0,
        mtime: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        etag: ETag::new(),
        mimetype: Mimetype::parse(DIRECTORY_MIMETYPE).unwrap(),
        permissions: Permissions::full(),
    }
}

fn make_file_meta(path: &StoragePath, size: u64, mime: &str) -> FileMetadata {
    FileMetadata {
        path: path.clone(),
        kind: FileKind::File,
        size,
        mtime: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        etag: ETag::new(),
        mimetype: Mimetype::parse(mime).unwrap(),
        permissions: Permissions::full(),
    }
}

pub fn share_request(
    requester: &str,
    home_storage_id: &str,
    path: &str,
    share_type: ShareType,
    with: &str,
    perms: u32,
) -> CreateShareRequest {
    CreateShareRequest {
        requester: requester.to_string(),
        path: path.to_string(),
        share_type,
        share_with: with.to_string(),
        permissions: perms,
        home_storage_id: home_storage_id.to_string(),
    }
}

// --- URL → config helpers (parsing a URL is the simplest way to populate
//     FileConfig fields from env without reinventing the wheel) ---

fn mysql_config_from_url(url: &str) -> FileConfig {
    let parsed = parse_url(url);
    let mut cfg = minimal_sqlite_config(PathBuf::from("ignored.db"));
    cfg.dbtype = DbType::Mysql;
    cfg.dbhost = Some(parsed.host);
    cfg.dbport = Some(parsed.port);
    cfg.dbuser = Some(parsed.user);
    cfg.dbpassword = parsed.password.map(|p| SecretString::new(p.into()));
    cfg.dbname = parsed.database;
    cfg
}

fn postgres_config_from_url(url: &str) -> FileConfig {
    let parsed = parse_url(url);
    let mut cfg = minimal_sqlite_config(PathBuf::from("ignored.db"));
    cfg.dbtype = DbType::Pgsql;
    cfg.dbhost = Some(parsed.host);
    cfg.dbport = Some(parsed.port);
    cfg.dbuser = Some(parsed.user);
    cfg.dbpassword = parsed.password.map(|p| SecretString::new(p.into()));
    cfg.dbname = parsed.database;
    cfg
}

struct ParsedUrl {
    user: String,
    password: Option<String>,
    host: String,
    port: u16,
    database: String,
}

fn parse_url(url: &str) -> ParsedUrl {
    let after_scheme = url.split_once("://").expect("scheme").1;
    let (auth, host_db) = after_scheme.split_once('@').expect("auth");
    let (user, password) = match auth.split_once(':') {
        Some((u, p)) => (u.to_string(), Some(p.to_string())),
        None => (auth.to_string(), None),
    };
    let (host_port, database) = host_db.split_once('/').expect("path");
    let (host, port) = match host_port.split_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap()),
        None => (host_port.to_string(), 0),
    };
    ParsedUrl {
        user,
        password,
        host,
        port,
        database: database.to_string(),
    }
}

// Anchor crates referenced only by tests/sharing_e2e.rs so clippy --all-targets
// doesn't flag them when running the common module in isolation.
use anyhow as _;
use async_trait as _;
use chrono as _;
use crabcloud_storage as _;
use serde as _;
use sqlx as _;
use thiserror as _;
use tokio as _;
use tracing as _;

// Suppress unused-import false positives for items only referenced from
// downstream test modules.
const _: fn() = || {
    let _ = DbError::Migration {
        namespace: String::new(),
        version: 0,
        message: String::new(),
    };
};
