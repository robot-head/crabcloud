use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;

/// Supported database backends. Mirrors Nextcloud's `dbtype` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DbType {
    /// In-process SQLite database; default and easiest for dev/test.
    Sqlite,
    /// MySQL / MariaDB over a TCP connection.
    Mysql,
    /// PostgreSQL over a TCP connection.
    Pgsql,
}

impl DbType {
    /// String form used by config files and the legacy `dbtype` field.
    pub fn as_str(self) -> &'static str {
        match self {
            DbType::Sqlite => "sqlite",
            DbType::Mysql => "mysql",
            DbType::Pgsql => "pgsql",
        }
    }
}

/// The complete file-loaded configuration. Validated into this struct on boot;
/// invalid configs fail fast.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    // --- Identity / instance ---
    /// Stable per-installation identifier (Nextcloud `instanceid`). Used as a
    /// suffix on cookies, cache keys, and the OCS payload.
    pub instanceid: String,
    /// Server-wide secret for signing session cookies / CSRF tokens.
    pub secret: SecretString,
    /// Per-user password salt used for legacy Nextcloud hashing schemes.
    pub passwordsalt: SecretString,
    /// Installation flag; false means the installer must run first.
    #[serde(default)]
    pub installed: bool,
    /// Stored upstream-Nextcloud-compatible version string for clients.
    pub version: String,
    /// Human-readable version (e.g. `31.0.0`) shipped on `/status.php`.
    pub versionstring: String,

    // --- Database ---
    /// Selected database backend.
    pub dbtype: DbType,
    /// Host for MySQL/Postgres; ignored for SQLite.
    pub dbhost: Option<String>,
    /// TCP port for the database host; defaults are backend-specific.
    pub dbport: Option<u16>,
    /// Database name (or filesystem path for SQLite).
    pub dbname: String,
    /// Username for authenticated DB backends.
    pub dbuser: Option<String>,
    /// Password for authenticated DB backends.
    pub dbpassword: Option<SecretString>,
    /// Table-name prefix applied to all schema objects.
    #[serde(default = "default_db_prefix")]
    pub dbtableprefix: String,
    /// Maximum size of the connection pool.
    #[serde(default = "default_db_pool_max")]
    pub db_pool_max: u32,

    // --- Data ---
    /// Filesystem path where user data lives.
    pub datadirectory: PathBuf,

    // --- Web / proxy ---
    /// Hostnames the server will answer requests for (Nextcloud trusted_domains).
    #[serde(default)]
    pub trusted_domains: Vec<String>,
    /// IP CIDRs allowed to set forwarding headers (`X-Forwarded-*`).
    #[serde(default)]
    pub trusted_proxies: Vec<String>,
    /// Forced base URL for CLI-issued links (e.g. `occ` output, share emails).
    #[serde(rename = "overwrite.cli.url")]
    pub overwrite_cli_url: Option<String>,
    /// Forced protocol (`http`/`https`) when behind a TLS-terminating proxy.
    #[serde(rename = "overwrite.protocol")]
    pub overwrite_protocol: Option<String>,
    /// Forced `Host` value when behind a reverse proxy.
    #[serde(rename = "overwrite.host")]
    pub overwrite_host: Option<String>,

    // --- Logging ---
    /// `tracing` env-filter directive (e.g. `info`, `crabcloud_http=debug,info`).
    #[serde(default = "default_loglevel")]
    pub loglevel: String,
    /// Optional path to a log file; if unset, logs go to stderr.
    pub logfile: Option<PathBuf>,

    // --- i18n ---
    /// Fallback locale when no `Accept-Language` preference matches.
    #[serde(default = "default_language")]
    pub default_language: String,

    // --- Crabcloud-specific ---
    /// Address the HTTP server binds to.
    #[serde(default = "default_bind_address")]
    pub bind_address: SocketAddr,
    /// Cache backend selection (currently `memory` only).
    #[serde(default)]
    pub cache: CacheConfig,
    /// Filecache + scanner tuning. Defaults: enabled, 1024-event channel.
    #[serde(default)]
    pub filecache: FilecacheConfig,

    /// Optional bootstrap admin (Phase 3 deferred-users stand-in).
    pub bootstrap_admin: Option<BootstrapAdminConfig>,
}

/// Cache subsystem configuration. Currently selects the backend.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CacheConfig {
    /// Backend identifier (`memory` is the only implemented value).
    #[serde(default = "default_cache_backend")]
    pub backend: String,
}

/// Filecache + scanner tuning. Drives `AppStateBuilder::build` (whether the
/// scanner consumer is spawned, and the broadcast channel capacity used by
/// `ChannelEventSink`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilecacheConfig {
    /// When `true`, `AppStateBuilder::build` spawns the scanner's continuous
    /// consumer loop. When `false`, `register_storage` + `full_scan` still
    /// work — only the background apply loop is suppressed.
    #[serde(default = "default_filecache_enabled")]
    pub enabled: bool,
    /// `tokio::sync::broadcast` capacity wired into `ChannelEventSink`.
    /// Slow consumers past this bound get `RecvError::Lagged`, which the
    /// scanner recovers from via a full-scan of every registered storage.
    #[serde(default = "default_event_channel_capacity")]
    pub event_channel_capacity: usize,
}

impl Default for FilecacheConfig {
    fn default() -> Self {
        Self {
            enabled: default_filecache_enabled(),
            event_channel_capacity: default_event_channel_capacity(),
        }
    }
}

/// Phase 3 stub for the deferred users sub-project. A single admin account
/// whose credentials live in `config.toml`. Real user store lands later.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct BootstrapAdminConfig {
    /// Username of the bootstrap administrator. Compared verbatim against form input.
    pub username: String,
    /// bcrypt hash of the password. Generate with `htpasswd -nBC 12` or
    /// `bcrypt::hash`.
    pub password_hash: String,
}

fn default_db_prefix() -> String {
    "oc_".to_string()
}
fn default_db_pool_max() -> u32 {
    16
}
fn default_loglevel() -> String {
    "info".to_string()
}
fn default_language() -> String {
    "en".to_string()
}
fn default_bind_address() -> SocketAddr {
    "127.0.0.1:8080".parse().unwrap()
}
fn default_cache_backend() -> String {
    "memory".to_string()
}
fn default_filecache_enabled() -> bool {
    true
}
fn default_event_channel_capacity() -> usize {
    1024
}

/// Errors raised while validating a parsed config.
#[derive(Debug, thiserror::Error)]
pub enum FileConfigError {
    /// A required field was missing or empty.
    #[error("missing required field: {0}")]
    MissingField(&'static str),
    /// A field had a value that's syntactically valid but semantically wrong.
    #[error("invalid value for `{field}`: {message}")]
    InvalidValue {
        /// Name of the offending field.
        field: &'static str,
        /// Human-readable reason the value was rejected.
        message: String,
    },
}

impl FileConfig {
    /// Post-deserialization validation. Called by the loader after merging layers.
    pub fn validate(&self) -> Result<(), FileConfigError> {
        if self.instanceid.is_empty() {
            return Err(FileConfigError::MissingField("instanceid"));
        }
        if self.dbname.is_empty() {
            return Err(FileConfigError::MissingField("dbname"));
        }
        if matches!(self.dbtype, DbType::Mysql | DbType::Pgsql) && self.dbhost.is_none() {
            return Err(FileConfigError::MissingField("dbhost"));
        }
        if self.db_pool_max == 0 {
            return Err(FileConfigError::InvalidValue {
                field: "db_pool_max",
                message: "must be at least 1".into(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    fn minimal_sqlite_config() -> FileConfig {
        FileConfig {
            instanceid: "abc123".to_string(),
            secret: SecretString::new("a-secret".into()),
            passwordsalt: SecretString::new("a-salt".into()),
            installed: true,
            version: "31.0.0.0".to_string(),
            versionstring: "31.0.0".to_string(),
            dbtype: DbType::Sqlite,
            dbhost: None,
            dbport: None,
            dbname: "crabcloud".to_string(),
            dbuser: None,
            dbpassword: None,
            dbtableprefix: "oc_".to_string(),
            db_pool_max: 16,
            datadirectory: "/var/lib/crabcloud".into(),
            trusted_domains: vec!["localhost".to_string()],
            trusted_proxies: vec![],
            overwrite_cli_url: None,
            overwrite_protocol: None,
            overwrite_host: None,
            loglevel: "info".to_string(),
            logfile: None,
            default_language: "en".to_string(),
            bind_address: "127.0.0.1:8080".parse().unwrap(),
            cache: CacheConfig {
                backend: "memory".to_string(),
            },
            filecache: FilecacheConfig::default(),
            bootstrap_admin: None,
        }
    }

    #[test]
    fn dbtype_serializes_as_lowercase_string() {
        let s = serde_json::to_string(&DbType::Pgsql).unwrap();
        assert_eq!(s, "\"pgsql\"");
    }

    #[test]
    fn minimal_config_validates() {
        minimal_sqlite_config().validate().unwrap();
    }

    #[test]
    fn missing_instanceid_fails() {
        let mut c = minimal_sqlite_config();
        c.instanceid.clear();
        let err = c.validate().unwrap_err();
        assert!(matches!(err, FileConfigError::MissingField("instanceid")));
    }

    #[test]
    fn mysql_without_dbhost_fails() {
        let mut c = minimal_sqlite_config();
        c.dbtype = DbType::Mysql;
        let err = c.validate().unwrap_err();
        assert!(matches!(err, FileConfigError::MissingField("dbhost")));
    }

    #[test]
    fn zero_pool_max_fails() {
        let mut c = minimal_sqlite_config();
        c.db_pool_max = 0;
        let err = c.validate().unwrap_err();
        assert!(matches!(
            err,
            FileConfigError::InvalidValue {
                field: "db_pool_max",
                ..
            }
        ));
    }

    #[test]
    fn dbpassword_is_not_in_debug_output() {
        let mut c = minimal_sqlite_config();
        c.dbpassword = Some(SecretString::new("super-secret-value".into()));
        let dbg = format!("{:?}", c);
        assert!(!dbg.contains("super-secret-value"));
        assert_eq!(
            c.dbpassword.as_ref().unwrap().expose_secret(),
            "super-secret-value"
        );
    }

    #[test]
    fn toml_deserialize_with_dotted_overwrite_keys() {
        // Confirm overwrite.cli.url etc. round-trip via TOML.
        let input = r#"
instanceid = "i1"
secret = "s"
passwordsalt = "ps"
installed = true
version = "31.0.0.0"
versionstring = "31.0.0"
dbtype = "sqlite"
dbname = "crabcloud"
datadirectory = "/var/lib/crabcloud"
trusted_domains = ["localhost"]
"overwrite.cli.url" = "https://cloud.example.com"
"overwrite.protocol" = "https"
"#;
        let cfg: FileConfig = toml::from_str(input).unwrap();
        cfg.validate().unwrap();
        assert_eq!(
            cfg.overwrite_cli_url.as_deref(),
            Some("https://cloud.example.com")
        );
        assert_eq!(cfg.overwrite_protocol.as_deref(), Some("https"));
    }

    #[test]
    fn bootstrap_admin_round_trips_via_toml() {
        let input = r#"
instanceid = "i1"
secret = "s"
passwordsalt = "ps"
installed = true
version = "31.0.0.0"
versionstring = "31.0.0"
dbtype = "sqlite"
dbname = "crabcloud"
datadirectory = "/var/lib/crabcloud"
trusted_domains = ["localhost"]

[bootstrap_admin]
username = "admin"
password_hash = "$2b$12$abcdefghijklmnopqrstuv"
"#;
        let cfg: FileConfig = toml::from_str(input).unwrap();
        cfg.validate().unwrap();
        let ba = cfg.bootstrap_admin.unwrap();
        assert_eq!(ba.username, "admin");
        assert!(ba.password_hash.starts_with("$2b$"));
    }
}
