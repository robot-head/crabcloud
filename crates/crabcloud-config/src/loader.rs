use crate::types::{FileConfig, FileConfigError};
use figment::{
    providers::{Env, Format, Toml},
    Figment,
};
use std::path::Path;

/// Errors produced by [`load`] when assembling the layered configuration.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// The base config file does not exist on disk.
    #[error("config file `{path}` not found")]
    NotFound {
        /// Path that was probed (display string of the requested file).
        path: String,
    },
    /// The TOML or environment overlay failed to parse / deserialize into `FileConfig`.
    ///
    /// `figment::Error` is large (~200 bytes), so it's boxed to keep
    /// `LoadError` small per clippy::result_large_err.
    #[error("config parse error: {0}")]
    Parse(#[from] Box<figment::Error>),
    /// The merged configuration failed `FileConfig::validate`.
    #[error(transparent)]
    Validate(#[from] FileConfigError),
}

/// Load and validate the layered configuration.
///
/// Merge order (later layers win): the TOML file at `base` → an optional
/// sibling `config.local.toml` → `CRABCLOUD_*` env vars (with `__` as nested
/// separator) → `cli_overrides` (pairs of dotted key + string value).
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

    // CRABCLOUD_* env vars override file values. Dotted keys (overwrite.cli.url) are
    // supported via Env::raw().split("__") — i.e., CRABCLOUD_OVERWRITE__CLI__URL.
    // Some CRABCLOUD_* vars are NOT config fields and must be ignored, otherwise
    // figment fails under deny_unknown_fields:
    //   CONFIG    — reserved for the clap config-path flag
    //   GIT_SHA   — emitted by crabcloud-app's build.rs via cargo:rustc-env;
    //               propagates into the runtime env when launched via `cargo run`
    //               (e.g. our CI's `cargo run --release -p crabcloud-app -- migrate`).
    //   TEST_MYSQL_URL, TEST_POSTGRES_URL, E2E_URL — reserved for test/CI tooling.
    fig = fig.merge(Env::prefixed("CRABCLOUD_").split("__").ignore(&[
        "CONFIG",
        "GIT_SHA",
        "TEST_MYSQL_URL",
        "TEST_POSTGRES_URL",
        "E2E_URL",
    ]));

    // CLI overrides win last.
    for (key, value) in cli_overrides {
        fig = fig.merge(figment::providers::Serialized::default(
            key,
            value.to_string(),
        ));
    }

    let cfg: FileConfig = fig.extract().map_err(Box::new)?;
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
dbname = "crabcloud"
datadirectory = "/var/lib/crabcloud"
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
    fn validation_runs_after_cli_merge() {
        // Base TOML has dbtype=sqlite (valid, no dbhost needed).
        // CLI override flips dbtype to mysql; merged value is mysql WITHOUT dbhost
        // (the base never set it), which validate() must reject.
        // This proves CLI overrides are merged BEFORE validation runs.
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, MINIMAL_TOML).unwrap();
        let err = load(&path, &[("dbtype", "mysql")]).unwrap_err();
        assert!(matches!(
            err,
            LoadError::Validate(FileConfigError::MissingField("dbhost"))
        ));
    }

    #[test]
    fn crabcloud_config_env_var_is_ignored_by_loader() {
        // SAFETY: tests in the same module run on the same thread, but other tests
        // in the binary may set env vars concurrently. To keep this test reliable,
        // we set and unset the env var around the load.
        std::env::set_var("CRABCLOUD_CONFIG", "/this/should/be/ignored.toml");
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, MINIMAL_TOML).unwrap();
        let result = load(&path, &[]);
        std::env::remove_var("CRABCLOUD_CONFIG");
        let cfg = result.unwrap();
        assert_eq!(cfg.instanceid, "abc123");
    }

    #[test]
    fn build_script_env_vars_are_ignored_by_loader() {
        // crabcloud-app's build.rs emits cargo:rustc-env=CRABCLOUD_GIT_SHA=...
        // which propagates into the runtime env under `cargo run` (CI's e2e path).
        // Figment would reject it under deny_unknown_fields without the ignore list.
        for (k, v) in [
            ("CRABCLOUD_GIT_SHA", "deadbeef"),
            ("CRABCLOUD_TEST_MYSQL_URL", "mysql://x"),
            ("CRABCLOUD_TEST_POSTGRES_URL", "postgres://x"),
            ("CRABCLOUD_E2E_URL", "http://localhost:8080"),
        ] {
            std::env::set_var(k, v);
        }
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, MINIMAL_TOML).unwrap();
        let result = load(&path, &[]);
        for k in [
            "CRABCLOUD_GIT_SHA",
            "CRABCLOUD_TEST_MYSQL_URL",
            "CRABCLOUD_TEST_POSTGRES_URL",
            "CRABCLOUD_E2E_URL",
        ] {
            std::env::remove_var(k);
        }
        let cfg = result.expect("loader must ignore build-script + CI env vars");
        assert_eq!(cfg.instanceid, "abc123");
    }
}
