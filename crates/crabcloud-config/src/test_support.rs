//! Test-only helpers for building `FileConfig` instances. Compiled only when
//! the `test-support` feature is enabled.
//!
//! The helper produces a minimal, valid SQLite-backed config suitable for
//! integration and unit tests. Callers mutate specific fields to exercise
//! particular code paths (e.g., setting `bootstrap_admin`).

use crate::types::{
    BootstrapAdminConfig, CacheConfig, DbType, FileConfig, FilecacheConfig, MailConfig,
};
use secrecy::SecretString;
use std::net::SocketAddr;
use std::path::PathBuf;

/// Build a minimal SQLite-backed `FileConfig` for tests.
///
/// - `dbname` is set to `db_path.to_string_lossy()`.
/// - `bootstrap_admin` is `None`. Set it explicitly if a test exercises login.
/// - `bind_address` is `127.0.0.1:0` (ephemeral port — only relevant if the
///   test actually binds a TCP listener).
/// - All secrets are placeholder strings; do not use this helper outside tests.
pub fn minimal_sqlite_config(db_path: PathBuf) -> FileConfig {
    FileConfig {
        instanceid: "test".into(),
        secret: SecretString::new("a-32-byte-or-longer-secret-key!".into()),
        passwordsalt: SecretString::new("ps".into()),
        installed: true,
        version: "31.0.0.0".into(),
        versionstring: "31.0.0".into(),
        dbtype: DbType::Sqlite,
        dbhost: None,
        dbport: None,
        dbname: db_path.to_string_lossy().into(),
        dbuser: None,
        dbpassword: None,
        dbtableprefix: "oc_".into(),
        db_pool_max: 4,
        datadirectory: PathBuf::from("/tmp"),
        trusted_domains: vec!["localhost".into()],
        trusted_proxies: vec![],
        overwrite_cli_url: None,
        overwrite_protocol: None,
        overwrite_host: None,
        loglevel: "info".into(),
        logfile: None,
        default_language: "en".into(),
        bind_address: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        cache: CacheConfig::default(),
        filecache: FilecacheConfig::default(),
        folder_zip_max_entries: 500,
        folder_zip_max_bytes: 2 * 1024 * 1024 * 1024,
        preview_root: None,
        preview_max_pixels: 64 * 1024 * 1024,
        preview_retention_days: 60,
        mail: MailConfig::default(),
        mail_queue_retention_days: 30,
        trash_retention_days: 30,
        bootstrap_admin: None,
    }
}

/// Build a SQLite-backed `FileConfig` with `bootstrap_admin` populated.
/// The `password_hash` is the literal value passed in — generate via
/// `bcrypt::hash(...)` in the test if needed.
pub fn sqlite_config_with_admin(
    db_path: PathBuf,
    username: impl Into<String>,
    password_hash: impl Into<String>,
) -> FileConfig {
    let mut cfg = minimal_sqlite_config(db_path);
    cfg.bootstrap_admin = Some(BootstrapAdminConfig {
        username: username.into(),
        password_hash: password_hash.into(),
    });
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn minimal_config_validates() {
        let dir = tempdir().unwrap();
        let cfg = minimal_sqlite_config(dir.path().join("t.db"));
        cfg.validate().unwrap();
    }

    #[test]
    fn admin_config_carries_username_and_hash() {
        let dir = tempdir().unwrap();
        let cfg = sqlite_config_with_admin(dir.path().join("t.db"), "alice", "$2b$12$hash");
        let admin = cfg.bootstrap_admin.unwrap();
        assert_eq!(admin.username, "alice");
        assert_eq!(admin.password_hash, "$2b$12$hash");
    }
}
