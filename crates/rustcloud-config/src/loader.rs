use crate::types::{FileConfig, FileConfigError};
use figment::{
    providers::{Env, Format, Toml},
    Figment,
};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("config file `{path}` not found")]
    NotFound { path: String },
    #[error("config parse error: {0}")]
    Parse(#[from] figment::Error),
    #[error(transparent)]
    Validate(#[from] FileConfigError),
}

pub fn load(base: &Path, cli_overrides: &[(&str, &str)]) -> Result<FileConfig, LoadError> {
    if !base.exists() {
        return Err(LoadError::NotFound {
            path: base.display().to_string(),
        });
    }

    let local_overlay = base
        .parent()
        .map(|dir| dir.join("config.local.toml"))
        .filter(|p| p.exists());

    let mut fig = Figment::new().merge(Toml::file(base));

    if let Some(local) = local_overlay {
        fig = fig.merge(Toml::file(local));
    }

    // RUSTCLOUD_* env vars override file values. Dotted keys (overwrite.cli.url) are
    // supported via Env::raw().split("__") — i.e., RUSTCLOUD_OVERWRITE__CLI__URL.
    fig = fig.merge(Env::prefixed("RUSTCLOUD_").split("__"));

    // CLI overrides win last.
    for (key, value) in cli_overrides {
        fig = fig.merge(figment::providers::Serialized::default(
            key,
            value.to_string(),
        ));
    }

    let cfg: FileConfig = fig.extract()?;
    cfg.validate()?;
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    const MINIMAL_TOML: &str = r#"
instanceid = "abc123"
secret = "s"
passwordsalt = "ps"
installed = true
version = "31.0.0.0"
versionstring = "31.0.0"
dbtype = "sqlite"
dbname = "rustcloud"
datadirectory = "/var/lib/rustcloud"
trusted_domains = ["localhost"]
"#;

    #[test]
    fn loads_minimal_toml() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, MINIMAL_TOML).unwrap();
        let cfg = load(&path, &[]).unwrap();
        assert_eq!(cfg.instanceid, "abc123");
        assert_eq!(cfg.dbtype, crate::DbType::Sqlite);
        assert_eq!(cfg.db_pool_max, 16); // default applied
    }

    #[test]
    fn local_overlay_overrides_base() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), MINIMAL_TOML).unwrap();
        fs::write(
            dir.path().join("config.local.toml"),
            "instanceid = \"overridden\"\n",
        )
        .unwrap();
        let cfg = load(&dir.path().join("config.toml"), &[]).unwrap();
        assert_eq!(cfg.instanceid, "overridden");
    }

    #[test]
    fn missing_file_errors_clearly() {
        let dir = tempdir().unwrap();
        let err = load(&dir.path().join("does-not-exist.toml"), &[]).unwrap_err();
        assert!(matches!(err, LoadError::NotFound { .. }));
    }

    #[test]
    fn cli_override_wins_over_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, MINIMAL_TOML).unwrap();
        let cfg = load(&path, &[("instanceid", "cli-win")]).unwrap();
        assert_eq!(cfg.instanceid, "cli-win");
    }

    #[test]
    fn validation_runs_after_merge() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut bad = String::from(MINIMAL_TOML);
        // Override to invalid: dbtype mysql but no dbhost.
        bad.push_str("\n[dummy_unused]\n"); // make sure the file still parses
        let _ = bad;
        fs::write(&path, MINIMAL_TOML).unwrap();
        // Now force dbtype mysql via CLI without dbhost.
        let err = load(&path, &[("dbtype", "mysql")]).unwrap_err();
        assert!(matches!(
            err,
            LoadError::Validate(FileConfigError::MissingField("dbhost"))
        ));
    }
}
