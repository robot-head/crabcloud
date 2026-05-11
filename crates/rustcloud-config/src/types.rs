use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;

/// Supported database backends. Mirrors Nextcloud's `dbtype` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DbType {
    Sqlite,
    Mysql,
    Pgsql,
}

impl DbType {
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
    pub instanceid: String,
    pub secret: SecretString,
    pub passwordsalt: SecretString,
    /// Installation flag; false means the installer must run first.
    #[serde(default)]
    pub installed: bool,
    /// Stored upstream-Nextcloud-compatible version string for clients.
    pub version: String,
    pub versionstring: String,

    // --- Database ---
    pub dbtype: DbType,
    pub dbhost: Option<String>,
    pub dbport: Option<u16>,
    pub dbname: String,
    pub dbuser: Option<String>,
    pub dbpassword: Option<SecretString>,
    #[serde(default = "default_db_prefix")]
    pub dbtableprefix: String,
    #[serde(default = "default_db_pool_max")]
    pub db_pool_max: u32,

    // --- Data ---
    pub datadirectory: PathBuf,

    // --- Web / proxy ---
    #[serde(default)]
    pub trusted_domains: Vec<String>,
    #[serde(default)]
    pub trusted_proxies: Vec<String>,
    #[serde(rename = "overwrite.cli.url")]
    pub overwrite_cli_url: Option<String>,
    #[serde(rename = "overwrite.protocol")]
    pub overwrite_protocol: Option<String>,
    #[serde(rename = "overwrite.host")]
    pub overwrite_host: Option<String>,

    // --- Logging ---
    #[serde(default = "default_loglevel")]
    pub loglevel: String,
    pub logfile: Option<PathBuf>,

    // --- i18n ---
    #[serde(default = "default_language")]
    pub default_language: String,

    // --- Rustcloud-specific ---
    #[serde(default = "default_bind_address")]
    pub bind_address: SocketAddr,
    #[serde(default)]
    pub cache: CacheConfig,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CacheConfig {
    #[serde(default = "default_cache_backend")]
    pub backend: String,
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

/// Errors raised while validating a parsed config.
#[derive(Debug, thiserror::Error)]
pub enum FileConfigError {
    #[error("missing required field: {0}")]
    MissingField(&'static str),
    #[error("invalid value for `{field}`: {message}")]
    InvalidValue {
        field: &'static str,
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
            dbname: "rustcloud".to_string(),
            dbuser: None,
            dbpassword: None,
            dbtableprefix: "oc_".to_string(),
            db_pool_max: 16,
            datadirectory: "/var/lib/rustcloud".into(),
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
dbname = "rustcloud"
datadirectory = "/var/lib/rustcloud"
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
}
