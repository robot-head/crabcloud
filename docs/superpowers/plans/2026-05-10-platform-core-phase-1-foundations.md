# Platform Core — Phase 1: Foundations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the Rustcloud workspace and produce a binary that loads layered config, connects to SQLite/MySQL/Postgres, runs core migrations, and exits cleanly — verified by a multi-dialect integration test suite that runs green in CI.

**Architecture:** Cargo workspace with focused crates per responsibility. `rustcloud-server` is the binary; it consumes `rustcloud-config` (layered TOML + env + CLI overrides via `figment`) and `rustcloud-db` (a `DbPool` enum over three concrete sqlx pool types — `SqlitePool`, `MySqlPool`, `PgPool` — with a hand-rolled `MigrationRunner` that supports per-namespace migration tracking for the future app framework). No HTTP, no UI yet; this phase produces the substrate everything else builds on.

**Tech Stack:** Rust 1.83+ stable, `tokio` (async), `clap` (CLI), `figment` (config), `secrecy` (sensitive fields), `sqlx` 0.8 with `sqlite`/`mysql`/`postgres` features and `runtime-tokio-rustls`, `tracing` + `tracing-subscriber`, `anyhow` (errors at boundaries) + `thiserror` (typed errors in libraries), `cargo-xtask` pattern for project commands, GitHub Actions for CI.

**Parent spec:** `docs/superpowers/specs/2026-05-10-platform-core-design.md`. Defers HTTP/UI/cache/i18n/OCS to later phases.

---

## File Structure

Phase 1 creates the following files. Each task lists exactly what it creates or modifies.

```
rustcloud/
├── Cargo.toml                              # workspace manifest (Task 1)
├── rust-toolchain.toml                     # pin Rust version (Task 1)
├── .gitignore                              # (Task 1)
├── README.md                               # minimal stub (Task 1 → expanded Task 14)
├── .cargo/config.toml                      # cargo xtask alias (Task 2)
├── .github/workflows/ci.yml                # CI workflow (Task 12)
├── crates/
│   ├── rustcloud-config/
│   │   ├── Cargo.toml                      # (Task 4)
│   │   └── src/
│   │       ├── lib.rs                      # re-exports (Task 4)
│   │       ├── types.rs                    # FileConfig, DbType (Task 4)
│   │       └── loader.rs                   # figment-based loading (Task 5)
│   ├── rustcloud-db/
│   │   ├── Cargo.toml                      # (Task 7)
│   │   └── src/
│   │       ├── lib.rs                      # re-exports (Task 7)
│   │       ├── pool.rs                     # DbPool enum + connect (Task 7)
│   │       ├── migrate.rs                  # MigrationRunner (Task 8)
│   │       └── error.rs                    # DbError (Task 7)
│   └── rustcloud-server/
│       ├── Cargo.toml                      # (Task 3)
│       └── src/
│           ├── main.rs                     # entry point (Task 3 → expanded each task)
│           ├── cli.rs                      # clap definitions (Task 3)
│           └── tracing.rs                  # tracing-subscriber init (Task 3)
├── xtask/
│   ├── Cargo.toml                          # (Task 2)
│   └── src/
│       └── main.rs                         # xtask commands (Task 2 → expanded Task 11)
├── migrations/
│   └── core/
│       └── 0001_initial/
│           ├── sqlite.sql                  # (Task 9)
│           ├── mysql.sql                   # (Task 9)
│           └── postgres.sql                # (Task 9)
├── dev/
│   └── docker-compose.yml                  # MySQL + Postgres for local dev (Task 1)
├── config/
│   └── config.toml.example                 # sample config (Task 5)
└── tests/
    └── migrate_end_to_end.rs               # full migrate flow per dialect (Task 13)
```

---

## Conventions

- **Commits:** every task ends with at least one commit. Commit messages use Conventional Commits (`feat:`, `chore:`, `test:`, `docs:`). Co-Authored-By line is included.
- **Testing:** TDD — write the failing test first, watch it fail, implement, watch it pass, commit. Where a task creates a brand-new crate, the first test may be a smoke test (`assert_eq!(1, 1)`) just to verify the crate compiles; the meaningful test follows immediately.
- **No mocks for the DB.** Tests hit a real SQLite (in-process); MySQL and Postgres tests use `testcontainers-rs` in CI and a docker-compose stack locally.
- **Errors:** Library crates expose typed errors via `thiserror`. The `rustcloud-server` binary converts to `anyhow::Result` at the `main` boundary.
- **Shell:** commands shown use cross-platform Cargo / git / docker commands. Where shell-specific syntax matters (env var setting), both PowerShell and bash forms are shown.

---

## Task 1: Workspace bootstrap

**Files:**
- Create: `Cargo.toml`
- Create: `rust-toolchain.toml`
- Create: `.gitignore`
- Create: `README.md`
- Create: `dev/docker-compose.yml`

- [ ] **Step 1: Verify clean repo state**

Run:
```
git status
```
Expected: branch `master`, working tree clean, no untracked files except the existing `docs/superpowers/` tree.

- [ ] **Step 2: Create the workspace `Cargo.toml`**

Write `Cargo.toml`:
```toml
[workspace]
# Members are added by subsequent tasks as the corresponding crates are created.
members = []
resolver = "2"

[workspace.package]
version = "0.1.0"
edition = "2021"
rust-version = "1.83"
license = "AGPL-3.0-or-later"
authors = ["Rustcloud Contributors"]
repository = "https://github.com/mdstone/rustcloud"

[workspace.dependencies]
anyhow = "1.0"
async-trait = "0.1"
clap = { version = "4.5", features = ["derive", "env"] }
figment = { version = "0.10", features = ["toml", "env"] }
secrecy = { version = "0.10", features = ["serde"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sqlx = { version = "0.8", default-features = false, features = [
    "runtime-tokio-rustls",
    "sqlite",
    "mysql",
    "postgres",
    "chrono",
    "macros",
    "migrate",
] }
thiserror = "2"
tempfile = "3"
testcontainers = "0.23"
testcontainers-modules = { version = "0.11", features = ["mysql", "postgres"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread", "signal", "fs", "time"] }
toml = "0.8"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }

# Internal workspace deps. Entries are added by subsequent tasks when their crate exists.
# rustcloud-config = { path = "crates/rustcloud-config" }   # Task 4
# rustcloud-db     = { path = "crates/rustcloud-db" }       # Task 7

[profile.release]
lto = "thin"
codegen-units = 1
strip = "debuginfo"
```

- [ ] **Step 3: Pin the Rust toolchain**

Write `rust-toolchain.toml`:
```toml
[toolchain]
channel = "1.83.0"
components = ["rustfmt", "clippy"]
profile = "minimal"
```

- [ ] **Step 4: Write `.gitignore`**

Write `.gitignore`:
```
/target
**/*.rs.bk
Cargo.lock.bak

# Local dev secrets / overrides
config/config.local.toml
.env
.env.local

# sqlx offline cache is committed; nothing to ignore.

# OS / editor
.DS_Store
Thumbs.db
.idea/
.vscode/*
!.vscode/settings.json
!.vscode/extensions.json
```

- [ ] **Step 5: Write a minimal `README.md`**

Write `README.md`:
```markdown
# Rustcloud

A Rust port of [Nextcloud server](https://github.com/nextcloud/server), with a Dioxus frontend.

**Status:** very early. See `docs/superpowers/specs/` for design specs and `docs/superpowers/plans/` for implementation plans.

## Quick start (development)

```bash
# Start dev databases
docker compose -f dev/docker-compose.yml up -d

# Build the workspace
cargo build

# Run lint + tests
cargo xtask check-all
```

## License

AGPL-3.0-or-later — see `LICENSE`.
```

- [ ] **Step 6: Write `dev/docker-compose.yml`**

Write `dev/docker-compose.yml`:
```yaml
services:
  mysql:
    image: mysql:8.4
    environment:
      MYSQL_ROOT_PASSWORD: rustcloud
      MYSQL_DATABASE: rustcloud
      MYSQL_USER: rustcloud
      MYSQL_PASSWORD: rustcloud
    ports:
      - "3307:3306"
    healthcheck:
      test: ["CMD", "mysqladmin", "ping", "-h", "localhost", "-prustcloud"]
      interval: 5s
      timeout: 5s
      retries: 12

  postgres:
    image: postgres:16
    environment:
      POSTGRES_DB: rustcloud
      POSTGRES_USER: rustcloud
      POSTGRES_PASSWORD: rustcloud
    ports:
      - "5433:5432"
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U rustcloud"]
      interval: 5s
      timeout: 5s
      retries: 12
```

Ports are non-default (3307, 5433) so they don't conflict with locally-installed DBs.

- [ ] **Step 7: Verify the manifest parses (skip cargo invocation — no members yet)**

Manually inspect `Cargo.toml`: confirm the TOML is syntactically valid (no unclosed brackets, no duplicate keys). A `cargo verify-project` call would fail here because `members = []` plus the `workspace.dependencies` block triggers warnings about an empty workspace — that's fine; we add members in subsequent tasks.

- [ ] **Step 8: Commit**

```
git add Cargo.toml rust-toolchain.toml .gitignore README.md dev/docker-compose.yml
git commit -m "chore: bootstrap cargo workspace and dev docker-compose

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 2: xtask scaffold + cargo alias

**Files:**
- Modify: `Cargo.toml` (add `xtask` to workspace members)
- Create: `.cargo/config.toml`
- Create: `xtask/Cargo.toml`
- Create: `xtask/src/main.rs`

The `cargo-xtask` pattern lets us add project commands (`cargo xtask check-all`, etc.) without external scripts. The `.cargo/config.toml` alias maps `xtask` to `run --package xtask --`.

- [ ] **Step 0: Add `xtask` to the workspace `members`**

Modify `Cargo.toml`:
```toml
[workspace]
members = [
    "xtask",
]
resolver = "2"
```

- [ ] **Step 1: Write `.cargo/config.toml`**

Write `.cargo/config.toml`:
```toml
[alias]
xtask = "run --package xtask --quiet --"
```

- [ ] **Step 2: Write `xtask/Cargo.toml`**

Write `xtask/Cargo.toml`:
```toml
[package]
name = "xtask"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
publish = false

[dependencies]
anyhow.workspace = true
clap.workspace = true
```

- [ ] **Step 3: Write `xtask/src/main.rs` with stub subcommands**

Write `xtask/src/main.rs`:
```rust
use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use std::process::Command;

#[derive(Parser)]
#[command(name = "xtask", about = "Project automation commands")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run `cargo fmt --check && cargo clippy && cargo test`.
    CheckAll,
    /// Stub — implemented in a later task.
    Build,
    /// Stub — implemented in a later task.
    Dev,
    /// Stub — implemented in a later phase (no query! macros yet).
    Prepare,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Cmd::CheckAll => check_all(),
        Cmd::Build => bail!("`build` is implemented in a later phase"),
        Cmd::Dev => bail!("`dev` is implemented in a later phase"),
        Cmd::Prepare => bail!("`prepare` is implemented in a later phase"),
    }
}

fn check_all() -> Result<()> {
    run("cargo", &["fmt", "--all", "--", "--check"])?;
    run("cargo", &["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"])?;
    run("cargo", &["test", "--workspace"])?;
    Ok(())
}

fn run(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program).args(args).status()?;
    if !status.success() {
        bail!("`{} {}` exited with status {}", program, args.join(" "), status);
    }
    Ok(())
}
```

- [ ] **Step 4: Verify the xtask compiles**

Run:
```
cargo build -p xtask
```
Expected: compiles cleanly (warnings allowed).

- [ ] **Step 5: Verify the alias works**

Run:
```
cargo xtask --help
```
Expected: prints usage text listing `check-all`, `build`, `dev`, `prepare` subcommands.

- [ ] **Step 6: Verify `check-all` runs (will fail at the test step because no tests exist yet — that's expected)**

Run:
```
cargo xtask check-all
```
Expected: passes fmt + clippy; `cargo test` runs with zero tests and exits 0. End-to-end PASS.

- [ ] **Step 7: Commit**

```
git add .cargo xtask
git commit -m "chore: add xtask crate with check-all subcommand and cargo alias

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 3: rustcloud-server binary skeleton + tracing + clap

**Files:**
- Create: `crates/rustcloud-server/Cargo.toml`
- Create: `crates/rustcloud-server/src/main.rs`
- Create: `crates/rustcloud-server/src/cli.rs`
- Create: `crates/rustcloud-server/src/tracing.rs`

This task gets the binary buildable with `--version`, `serve`, `migrate` subcommands stubbed. Real wiring happens in later tasks.

- [ ] **Step 1: Write the failing test (smoke: `cargo build -p rustcloud-server` succeeds)**

There's no source code yet — the "test" is just a build. Skip to creating the files.

- [ ] **Step 2: Write `crates/rustcloud-server/Cargo.toml`**

Write `crates/rustcloud-server/Cargo.toml`:
```toml
[package]
name = "rustcloud-server"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[[bin]]
name = "rustcloud-server"
path = "src/main.rs"

[dependencies]
anyhow.workspace = true
clap.workspace = true
tokio.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
```

- [ ] **Step 3: Write `crates/rustcloud-server/src/cli.rs`**

Write `crates/rustcloud-server/src/cli.rs`:
```rust
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "rustcloud-server", version, about = "Rustcloud server")]
pub struct Cli {
    /// Path to the main config file.
    #[arg(long, env = "RUSTCLOUD_CONFIG", default_value = "config/config.toml", global = true)]
    pub config: PathBuf,

    #[command(subcommand)]
    pub command: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Start the HTTP server (implemented in a later phase).
    Serve,
    /// Run pending migrations and exit (implemented in Task 10).
    Migrate,
    /// Print version information.
    Version,
}

impl Cli {
    pub fn command(&self) -> Cmd {
        // Default subcommand is `serve` when none specified.
        match &self.command {
            Some(c) => match c {
                Cmd::Serve => Cmd::Serve,
                Cmd::Migrate => Cmd::Migrate,
                Cmd::Version => Cmd::Version,
            },
            None => Cmd::Serve,
        }
    }
}
```

- [ ] **Step 4: Write `crates/rustcloud-server/src/tracing.rs`**

Write `crates/rustcloud-server/src/tracing.rs`:
```rust
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Initialize the global tracing subscriber.
///
/// - `RUST_LOG` (or otherwise `info`) selects the filter.
/// - Output is JSON when stdout is not a TTY, plain otherwise.
pub fn init() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let is_tty = std::io::IsTerminal::is_terminal(&std::io::stdout());

    let registry = tracing_subscriber::registry().with(filter);

    if is_tty {
        registry.with(fmt::layer().with_target(false)).init();
    } else {
        registry.with(fmt::layer().json().with_target(true)).init();
    }
}
```

- [ ] **Step 5: Write `crates/rustcloud-server/src/main.rs`**

Write `crates/rustcloud-server/src/main.rs`:
```rust
mod cli;
mod tracing;

use anyhow::Result;
use clap::Parser;
use tracing::info;

use crate::cli::{Cli, Cmd};

#[tokio::main]
async fn main() -> Result<()> {
    crate::tracing::init();
    let cli = Cli::parse();

    match cli.command() {
        Cmd::Version => {
            println!("rustcloud-server {} (build target subproject: platform-core)", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Cmd::Serve => {
            info!(config = %cli.config.display(), "serve subcommand not yet implemented");
            anyhow::bail!("`serve` is implemented in a later phase");
        }
        Cmd::Migrate => {
            info!(config = %cli.config.display(), "migrate subcommand not yet implemented");
            anyhow::bail!("`migrate` is implemented in Task 10");
        }
    }
}
```

Note: we import `tracing::info` for the macro and `crate::tracing` for our module. Renaming our module would avoid the clash, but `tracing` (the module name) maps cleanly to the crate it wraps; the `use tracing::info` in the body unambiguously refers to the external crate.

- [ ] **Step 6: Add `rustcloud-server` to the workspace and build**

Modify `Cargo.toml` — extend the `members` array:
```toml
[workspace]
members = [
    "crates/rustcloud-server",
    "xtask",
]
resolver = "2"
```

Run:
```
cargo build
```
Expected: clean build of `xtask` + `rustcloud-server`.

- [ ] **Step 7: Run `version` subcommand**

Run:
```
cargo run -p rustcloud-server -- version
```
Expected output:
```
rustcloud-server 0.1.0 (build target subproject: platform-core)
```

- [ ] **Step 8: Run with no subcommand to confirm `serve` is default and errors clearly**

Run:
```
cargo run -p rustcloud-server
```
Expected: structured log line "serve subcommand not yet implemented", then error `serve is implemented in a later phase`. Exit code non-zero.

- [ ] **Step 9: Write a smoke unit test for CLI parsing**

Modify `crates/rustcloud-server/src/cli.rs` — append:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_command_factory_is_valid() {
        // `debug_assert` panics on invalid clap configuration.
        Cli::command().debug_assert();
    }

    #[test]
    fn default_subcommand_is_serve() {
        let cli = Cli::parse_from(["rustcloud-server"]);
        assert!(matches!(cli.command(), Cmd::Serve));
    }

    #[test]
    fn version_subcommand_parses() {
        let cli = Cli::parse_from(["rustcloud-server", "version"]);
        assert!(matches!(cli.command(), Cmd::Version));
    }

    #[test]
    fn config_flag_overrides_default() {
        let cli = Cli::parse_from(["rustcloud-server", "--config", "/tmp/custom.toml", "version"]);
        assert_eq!(cli.config, std::path::PathBuf::from("/tmp/custom.toml"));
    }
}
```

- [ ] **Step 10: Run the tests**

Run:
```
cargo test -p rustcloud-server
```
Expected: 4 tests passed.

- [ ] **Step 11: Commit**

```
git add Cargo.toml crates/rustcloud-server
git commit -m "feat(server): add binary skeleton with clap CLI and tracing init

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 4: rustcloud-config types

**Files:**
- Create: `crates/rustcloud-config/Cargo.toml`
- Create: `crates/rustcloud-config/src/lib.rs`
- Create: `crates/rustcloud-config/src/types.rs`

Define the typed shape of the config file. No loading logic yet — just types + serde.

- [ ] **Step 1: Write `crates/rustcloud-config/Cargo.toml`**

Write `crates/rustcloud-config/Cargo.toml`:
```toml
[package]
name = "rustcloud-config"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
secrecy.workspace = true
serde.workspace = true
thiserror.workspace = true
toml.workspace = true

[dev-dependencies]
tempfile.workspace = true
```

- [ ] **Step 2: Write `crates/rustcloud-config/src/lib.rs`**

Write `crates/rustcloud-config/src/lib.rs`:
```rust
//! Layered configuration for Rustcloud.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §5.

mod types;

pub use types::{DbType, FileConfig, FileConfigError};
```

- [ ] **Step 3: Write `crates/rustcloud-config/src/types.rs`**

Write `crates/rustcloud-config/src/types.rs`:
```rust
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
#[derive(Debug, Clone, Deserialize, Serialize)]
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

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CacheConfig {
    #[serde(default = "default_cache_backend")]
    pub backend: String,
}

fn default_db_prefix() -> String { "oc_".to_string() }
fn default_db_pool_max() -> u32 { 16 }
fn default_loglevel() -> String { "info".to_string() }
fn default_language() -> String { "en".to_string() }
fn default_bind_address() -> SocketAddr { "127.0.0.1:8080".parse().unwrap() }
fn default_cache_backend() -> String { "memory".to_string() }

/// Errors raised while validating a parsed config.
#[derive(Debug, thiserror::Error)]
pub enum FileConfigError {
    #[error("missing required field: {0}")]
    MissingField(&'static str),
    #[error("invalid value for `{field}`: {message}")]
    InvalidValue { field: &'static str, message: String },
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
```

- [ ] **Step 4: Write unit tests for `types.rs`**

Append to `crates/rustcloud-config/src/types.rs`:
```rust
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
            cache: CacheConfig { backend: "memory".to_string() },
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
        assert!(matches!(err, FileConfigError::InvalidValue { field: "db_pool_max", .. }));
    }

    #[test]
    fn dbpassword_is_not_in_debug_output() {
        let mut c = minimal_sqlite_config();
        c.dbpassword = Some(SecretString::new("super-secret-value".into()));
        let dbg = format!("{:?}", c);
        assert!(!dbg.contains("super-secret-value"));
        assert_eq!(c.dbpassword.as_ref().unwrap().expose_secret(), "super-secret-value");
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
        assert_eq!(cfg.overwrite_cli_url.as_deref(), Some("https://cloud.example.com"));
        assert_eq!(cfg.overwrite_protocol.as_deref(), Some("https"));
    }
}
```

- [ ] **Step 5: Add `rustcloud-config` to the workspace**

Modify `Cargo.toml` — append `crates/rustcloud-config` to `members` and add the workspace dependency entry:
```toml
[workspace]
members = [
    "crates/rustcloud-config",
    "crates/rustcloud-server",
    "xtask",
]
```

Under `[workspace.dependencies]`, add (un-comment if you commented it out):
```toml
rustcloud-config = { path = "crates/rustcloud-config" }
```

- [ ] **Step 6: Run the tests**

Run:
```
cargo test -p rustcloud-config
```
Expected: 6 tests passed.

- [ ] **Step 7: Commit**

```
git add Cargo.toml crates/rustcloud-config
git commit -m "feat(config): add typed FileConfig and DbType with validation

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 5: rustcloud-config layered loader (figment)

**Files:**
- Create: `crates/rustcloud-config/src/loader.rs`
- Modify: `crates/rustcloud-config/Cargo.toml`
- Modify: `crates/rustcloud-config/src/lib.rs`
- Create: `config/config.toml.example`

Layered loading order (each overrides the previous):
1. The base TOML at the path passed in.
2. An optional `config.local.toml` overlay in the same directory.
3. Env vars prefixed `RUSTCLOUD_`.
4. CLI key=value overrides (a slice passed at load time).

- [ ] **Step 1: Add `figment` to the crate's deps**

Modify `crates/rustcloud-config/Cargo.toml` — replace the `[dependencies]` block:
```toml
[dependencies]
figment = { workspace = true, features = ["toml", "env"] }
secrecy.workspace = true
serde.workspace = true
thiserror.workspace = true
toml.workspace = true
```

- [ ] **Step 2: Write the failing test for layered loading**

Create `crates/rustcloud-config/src/loader.rs` with just the test for now:
```rust
use crate::types::{FileConfig, FileConfigError};
use figment::{providers::{Env, Format, Toml}, Figment};
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
    todo!("implemented in step 4")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::fs;

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
        ).unwrap();
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
        assert!(matches!(err, LoadError::Validate(FileConfigError::MissingField("dbhost"))));
    }
}
```

Note: `figment::providers::Env` and `figment::providers::Toml` come from the `env` and `toml` features we already enabled.

- [ ] **Step 3: Re-export from `lib.rs`**

Modify `crates/rustcloud-config/src/lib.rs`:
```rust
//! Layered configuration for Rustcloud.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §5.

mod loader;
mod types;

pub use loader::{load, LoadError};
pub use types::{CacheConfig, DbType, FileConfig, FileConfigError};
```

- [ ] **Step 4: Run the failing tests to confirm the test infrastructure works**

Run:
```
cargo test -p rustcloud-config
```
Expected: tests in `loader::tests` FAIL with `todo!()` panics. `types::tests` still pass.

- [ ] **Step 5: Implement `load`**

Replace the body of `load` in `crates/rustcloud-config/src/loader.rs`:
```rust
pub fn load(base: &Path, cli_overrides: &[(&str, &str)]) -> Result<FileConfig, LoadError> {
    if !base.exists() {
        return Err(LoadError::NotFound { path: base.display().to_string() });
    }

    let local_overlay = base.parent()
        .map(|dir| dir.join("config.local.toml"))
        .filter(|p| p.exists());

    let mut fig = Figment::new()
        .merge(Toml::file(base));

    if let Some(local) = local_overlay {
        fig = fig.merge(Toml::file(local));
    }

    // RUSTCLOUD_* env vars override file values. Dotted keys (overwrite.cli.url) are
    // supported via Env::raw().split("__") — i.e., RUSTCLOUD_OVERWRITE__CLI__URL.
    fig = fig.merge(Env::prefixed("RUSTCLOUD_").split("__"));

    // CLI overrides win last.
    for (key, value) in cli_overrides {
        fig = fig.merge(figment::providers::Serialized::default(key, value));
    }

    let cfg: FileConfig = fig.extract()?;
    cfg.validate()?;
    Ok(cfg)
}
```

The `Serialized` provider needs the `serde_json::Value` form for non-string types; for our test cases (all string overrides) `&str` is accepted.

- [ ] **Step 6: Run the tests**

Run:
```
cargo test -p rustcloud-config
```
Expected: all tests pass.

If `cli_override_wins_over_file` fails because `figment::providers::Serialized` doesn't accept `&str` directly, change the implementation to:
```rust
    for (key, value) in cli_overrides {
        fig = fig.merge(figment::providers::Serialized::default(key, value.to_string()));
    }
```

- [ ] **Step 7: Create `config/config.toml.example`**

Write `config/config.toml.example`:
```toml
# Rustcloud configuration. Copy to config/config.toml and edit.
# Most keys match Nextcloud's config.php semantically; the file format is TOML.

instanceid     = "REPLACE_WITH_RANDOM_STRING"
secret         = "REPLACE_WITH_RANDOM_32_BYTES_HEX"
passwordsalt   = "REPLACE_WITH_RANDOM_32_BYTES_HEX"

installed      = false
version        = "31.0.0.0"
versionstring  = "31.0.0"

dbtype         = "sqlite"
dbname         = "rustcloud.db"
dbtableprefix  = "oc_"
# For mysql/pgsql, set these and remove the comment characters:
# dbhost       = "127.0.0.1"
# dbport       = 3307
# dbuser       = "rustcloud"
# dbpassword   = "rustcloud"

datadirectory  = "./data"

trusted_domains = ["localhost", "127.0.0.1"]
# trusted_proxies = ["10.0.0.0/8"]

# Logging
loglevel = "info"

# Rustcloud-specific
bind_address = "127.0.0.1:8080"
db_pool_max  = 16

# overwrite.cli.url, overwrite.protocol, overwrite.host are supported via dotted keys:
# "overwrite.cli.url" = "https://cloud.example.com"
# "overwrite.protocol" = "https"

[cache]
backend = "memory"
```

- [ ] **Step 8: Commit**

```
git add crates/rustcloud-config config/config.toml.example
git commit -m "feat(config): add layered figment loader with env+CLI overrides

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 6: Wire config loading into rustcloud-server

**Files:**
- Modify: `crates/rustcloud-server/Cargo.toml`
- Modify: `crates/rustcloud-server/src/main.rs`

- [ ] **Step 1: Add `rustcloud-config` dependency**

Modify `crates/rustcloud-server/Cargo.toml` — replace `[dependencies]`:
```toml
[dependencies]
anyhow.workspace = true
clap.workspace = true
rustcloud-config.workspace = true
tokio.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
```

- [ ] **Step 2: Load config in `main`**

Modify `crates/rustcloud-server/src/main.rs` — replace the `match` block in `main`:
```rust
    match cli.command() {
        Cmd::Version => {
            println!("rustcloud-server {} (build target subproject: platform-core)", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Cmd::Serve => {
            let config = rustcloud_config::load(&cli.config, &[])?;
            info!(
                instanceid = %config.instanceid,
                dbtype = %config.dbtype.as_str(),
                "loaded config; serve subcommand not yet implemented"
            );
            anyhow::bail!("`serve` is implemented in a later phase");
        }
        Cmd::Migrate => {
            let config = rustcloud_config::load(&cli.config, &[])?;
            info!(
                instanceid = %config.instanceid,
                dbtype = %config.dbtype.as_str(),
                "loaded config; migrate subcommand not yet implemented"
            );
            anyhow::bail!("`migrate` is implemented in Task 10");
        }
    }
```

- [ ] **Step 3: Build the workspace**

Run:
```
cargo build
```
Expected: clean.

- [ ] **Step 4: Smoke-test against a fixture config**

Create a temp config file:

PowerShell:
```powershell
@'
instanceid = "smoketest"
secret = "s"
passwordsalt = "ps"
installed = true
version = "31.0.0.0"
versionstring = "31.0.0"
dbtype = "sqlite"
dbname = "rustcloud.db"
datadirectory = "./data"
trusted_domains = ["localhost"]
'@ | Out-File -Encoding utf8 fixture.toml
```

bash:
```bash
cat > fixture.toml <<'EOF'
instanceid = "smoketest"
secret = "s"
passwordsalt = "ps"
installed = true
version = "31.0.0.0"
versionstring = "31.0.0"
dbtype = "sqlite"
dbname = "rustcloud.db"
datadirectory = "./data"
trusted_domains = ["localhost"]
EOF
```

Then:
```
cargo run -p rustcloud-server -- --config fixture.toml serve
```

Expected: log line "loaded config; serve subcommand not yet implemented" with `instanceid=smoketest dbtype=sqlite`, then error exit.

Clean up: `rm fixture.toml`.

- [ ] **Step 5: Commit**

```
git add crates/rustcloud-server
git commit -m "feat(server): load layered config before dispatching subcommands

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 7: rustcloud-db crate — DbPool enum + connect

**Files:**
- Create: `crates/rustcloud-db/Cargo.toml`
- Create: `crates/rustcloud-db/src/lib.rs`
- Create: `crates/rustcloud-db/src/error.rs`
- Create: `crates/rustcloud-db/src/pool.rs`
- Modify: `Cargo.toml` (workspace members)

- [ ] **Step 1: Add `rustcloud-db` to the workspace**

Modify `Cargo.toml` — append `crates/rustcloud-db` to `members`:
```toml
[workspace]
members = [
    "crates/rustcloud-config",
    "crates/rustcloud-db",
    "crates/rustcloud-server",
    "xtask",
]
```

Under `[workspace.dependencies]`, add:
```toml
rustcloud-db = { path = "crates/rustcloud-db" }
```

- [ ] **Step 2: Write `crates/rustcloud-db/Cargo.toml`**

Write `crates/rustcloud-db/Cargo.toml`:
```toml
[package]
name = "rustcloud-db"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
async-trait.workspace = true
rustcloud-config.workspace = true
secrecy.workspace = true
sqlx.workspace = true
thiserror.workspace = true
tokio.workspace = true
tracing.workspace = true

[dev-dependencies]
tempfile.workspace = true
testcontainers.workspace = true
testcontainers-modules.workspace = true
```

- [ ] **Step 3: Write the error type**

Write `crates/rustcloud-db/src/error.rs`:
```rust
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("sqlx error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("invalid database URL: {0}")]
    InvalidUrl(String),
    #[error("migration error in namespace `{namespace}` version {version}: {message}")]
    Migration {
        namespace: String,
        version: i64,
        message: String,
    },
}

pub type DbResult<T> = Result<T, DbError>;
```

- [ ] **Step 4: Write the failing test for `DbPool::connect`**

Write `crates/rustcloud-db/src/pool.rs`:
```rust
use crate::error::{DbError, DbResult};
use rustcloud_config::{DbType, FileConfig};
use secrecy::ExposeSecret;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{mysql::MySqlPoolOptions, postgres::PgPoolOptions};
use sqlx::{MySqlPool, PgPool, SqlitePool};

#[derive(Debug, Clone)]
pub enum DbPool {
    Sqlite(SqlitePool),
    MySql(MySqlPool),
    Postgres(PgPool),
}

impl DbPool {
    /// Connect using settings from `config`.
    pub async fn connect(config: &FileConfig) -> DbResult<Self> {
        todo!("implemented in step 6")
    }

    /// Convenience: a short label for logging.
    pub fn dialect(&self) -> &'static str {
        match self {
            DbPool::Sqlite(_) => "sqlite",
            DbPool::MySql(_) => "mysql",
            DbPool::Postgres(_) => "postgres",
        }
    }

    /// Close the pool and wait for in-flight connections to drain.
    pub async fn close(&self) {
        match self {
            DbPool::Sqlite(p) => p.close().await,
            DbPool::MySql(p) => p.close().await,
            DbPool::Postgres(p) => p.close().await,
        }
    }
}

/// Build the dialect-appropriate connection URL for MySQL or Postgres.
/// SQLite uses `SqliteConnectOptions` directly to avoid Windows-path URL issues.
fn build_url(config: &FileConfig) -> DbResult<String> {
    match config.dbtype {
        DbType::Sqlite => Err(DbError::InvalidUrl(
            "build_url is not used for sqlite; SqliteConnectOptions is used directly".into(),
        )),
        DbType::Mysql => {
            let host = config.dbhost.as_deref().ok_or_else(|| DbError::InvalidUrl("dbhost required".into()))?;
            let port = config.dbport.unwrap_or(3306);
            let user = config.dbuser.as_deref().unwrap_or("");
            let password = config.dbpassword.as_ref().map(|s| s.expose_secret().to_string()).unwrap_or_default();
            Ok(format!("mysql://{}:{}@{}:{}/{}", user, password, host, port, config.dbname))
        }
        DbType::Pgsql => {
            let host = config.dbhost.as_deref().ok_or_else(|| DbError::InvalidUrl("dbhost required".into()))?;
            let port = config.dbport.unwrap_or(5432);
            let user = config.dbuser.as_deref().unwrap_or("");
            let password = config.dbpassword.as_ref().map(|s| s.expose_secret().to_string()).unwrap_or_default();
            Ok(format!("postgres://{}:{}@{}:{}/{}", user, password, host, port, config.dbname))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustcloud_config::CacheConfig;
    use secrecy::SecretString;
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn cfg_sqlite(path: PathBuf) -> FileConfig {
        FileConfig {
            instanceid: "test".into(),
            secret: SecretString::new("s".into()),
            passwordsalt: SecretString::new("ps".into()),
            installed: true,
            version: "31.0.0.0".into(),
            versionstring: "31.0.0".into(),
            dbtype: DbType::Sqlite,
            dbhost: None,
            dbport: None,
            dbname: path.to_string_lossy().into(),
            dbuser: None,
            dbpassword: None,
            dbtableprefix: "oc_".into(),
            db_pool_max: 4,
            datadirectory: "/tmp".into(),
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
        }
    }

    #[tokio::test]
    async fn connects_to_sqlite_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let cfg = cfg_sqlite(path);
        let pool = DbPool::connect(&cfg).await.unwrap();
        assert_eq!(pool.dialect(), "sqlite");

        // Smoke-test an actual query through the connection.
        let one: i64 = match &pool {
            DbPool::Sqlite(p) => sqlx::query_scalar("SELECT 1").fetch_one(p).await.unwrap(),
            _ => unreachable!(),
        };
        assert_eq!(one, 1);
        pool.close().await;
    }

    #[test]
    fn mysql_url_without_host_errors() {
        let mut cfg = cfg_sqlite(PathBuf::from("ignored.db"));
        cfg.dbtype = DbType::Mysql;
        cfg.dbhost = None;
        let err = build_url(&cfg).unwrap_err();
        assert!(matches!(err, DbError::InvalidUrl(_)));
    }

    #[test]
    fn pgsql_url_builds() {
        let mut cfg = cfg_sqlite(PathBuf::from("ignored.db"));
        cfg.dbtype = DbType::Pgsql;
        cfg.dbhost = Some("127.0.0.1".into());
        cfg.dbport = Some(5433);
        cfg.dbuser = Some("u".into());
        cfg.dbpassword = Some(SecretString::new("p".into()));
        cfg.dbname = "d".into();
        let url = build_url(&cfg).unwrap();
        assert_eq!(url, "postgres://u:p@127.0.0.1:5433/d");
    }
}
```

- [ ] **Step 5: Write `crates/rustcloud-db/src/lib.rs`**

Write `crates/rustcloud-db/src/lib.rs`:
```rust
//! Multi-dialect database layer for Rustcloud.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §6.

mod error;
mod pool;

pub mod migrate;

pub use error::{DbError, DbResult};
pub use pool::DbPool;
```

We forward-declare `pub mod migrate;` so the next task can drop in its file.

Create an empty `crates/rustcloud-db/src/migrate.rs` for now:
```rust
// Implemented in Task 8.
```

- [ ] **Step 6: Implement `DbPool::connect`**

Replace the body of `connect` in `crates/rustcloud-db/src/pool.rs`:
```rust
    pub async fn connect(config: &FileConfig) -> DbResult<Self> {
        let max = config.db_pool_max;
        match config.dbtype {
            DbType::Sqlite => {
                let opts = SqliteConnectOptions::new()
                    .filename(&config.dbname)
                    .create_if_missing(true);
                let pool = SqlitePoolOptions::new()
                    .max_connections(max)
                    .connect_with(opts)
                    .await?;
                Ok(DbPool::Sqlite(pool))
            }
            DbType::Mysql => {
                let url = build_url(config)?;
                let pool = MySqlPoolOptions::new()
                    .max_connections(max)
                    .connect(&url)
                    .await?;
                Ok(DbPool::MySql(pool))
            }
            DbType::Pgsql => {
                let url = build_url(config)?;
                let pool = PgPoolOptions::new()
                    .max_connections(max)
                    .connect(&url)
                    .await?;
                Ok(DbPool::Postgres(pool))
            }
        }
    }
```

- [ ] **Step 7: Run the unit tests (SQLite-only path)**

Run:
```
cargo test -p rustcloud-db --lib
```
Expected: 3 tests passed (`connects_to_sqlite_file`, `mysql_url_without_host_errors`, `pgsql_url_builds`).

MySQL and Postgres connection tests come later as integration tests (Task 13) — they need docker-compose running.

- [ ] **Step 8: Commit**

```
git add Cargo.toml crates/rustcloud-db
git commit -m "feat(db): add DbPool enum dispatching over Sqlite/MySql/Postgres

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 8: MigrationRunner with namespace tracking

**Files:**
- Modify: `crates/rustcloud-db/src/migrate.rs`
- Modify: `crates/rustcloud-db/src/lib.rs`

The migration table `oc_migrations` is created idempotently by the runner itself; registered migrations build on top.

- [ ] **Step 1: Write the failing test**

Replace `crates/rustcloud-db/src/migrate.rs`:
```rust
//! Per-namespace migration runner.
//!
//! See spec §6.4.

use crate::{DbPool, DbResult};
use std::collections::BTreeMap;

/// A single migration: its version within a namespace, and SQL for each dialect.
#[derive(Debug, Clone)]
pub struct Migration {
    pub version: i64,
    /// Short human-readable identifier.
    pub name: &'static str,
    /// SQLite-dialect SQL.
    pub sqlite: &'static str,
    /// MySQL-dialect SQL.
    pub mysql: &'static str,
    /// Postgres-dialect SQL.
    pub postgres: &'static str,
}

/// A set of migrations for one namespace.
#[derive(Debug, Clone)]
pub struct MigrationSet {
    pub namespace: &'static str,
    pub migrations: &'static [Migration],
}

pub struct MigrationRunner<'a> {
    pool: &'a DbPool,
    sets: Vec<MigrationSet>,
    prefix: String,
}

impl<'a> MigrationRunner<'a> {
    pub fn new(pool: &'a DbPool, prefix: impl Into<String>) -> Self {
        Self { pool, sets: Vec::new(), prefix: prefix.into() }
    }

    pub fn register(&mut self, set: MigrationSet) -> &mut Self {
        self.sets.push(set);
        self
    }

    /// Apply all pending migrations across registered namespaces.
    /// Returns the count actually applied.
    pub async fn run(&self) -> DbResult<usize> {
        ensure_tracking_table(self.pool, &self.prefix).await?;
        let mut applied = 0;
        for set in &self.sets {
            applied += run_namespace(self.pool, &self.prefix, set).await?;
        }
        Ok(applied)
    }

    /// List applied (namespace, version) pairs. For debugging / tests.
    pub async fn applied(&self) -> DbResult<BTreeMap<String, Vec<i64>>> {
        ensure_tracking_table(self.pool, &self.prefix).await?;
        list_applied(self.pool, &self.prefix).await
    }
}

async fn ensure_tracking_table(pool: &DbPool, prefix: &str) -> DbResult<()> {
    let table = format!("{}migrations", prefix);
    let sql = match pool {
        DbPool::Sqlite(_) => format!(
            "CREATE TABLE IF NOT EXISTS {table} (
                namespace TEXT NOT NULL,
                version INTEGER NOT NULL,
                applied_at TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (namespace, version)
            )"
        ),
        DbPool::MySql(_) => format!(
            "CREATE TABLE IF NOT EXISTS {table} (
                namespace VARCHAR(64) NOT NULL,
                version BIGINT NOT NULL,
                applied_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (namespace, version)
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4"
        ),
        DbPool::Postgres(_) => format!(
            "CREATE TABLE IF NOT EXISTS {table} (
                namespace TEXT NOT NULL,
                version BIGINT NOT NULL,
                applied_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (namespace, version)
            )"
        ),
    };
    execute(pool, &sql).await?;
    Ok(())
}

async fn run_namespace(pool: &DbPool, prefix: &str, set: &MigrationSet) -> DbResult<usize> {
    let applied = list_applied_for(pool, prefix, set.namespace).await?;
    let mut count = 0;
    for migration in set.migrations {
        if applied.contains(&migration.version) {
            continue;
        }
        let sql = pick_sql(pool, migration);
        // Each migration runs in its own transaction-ish unit: execute the SQL, then
        // record the row. For SQLite/MySQL we can't easily wrap multi-statement migration
        // SQL inside sqlx's transaction without DDL caveats — so we keep it simple and
        // accept that a partial failure leaves an unrecorded migration. The runner is
        // designed to be idempotent at the SQL level (CREATE TABLE IF NOT EXISTS, etc.).
        execute_multi(pool, sql).await.map_err(|e| crate::DbError::Migration {
            namespace: set.namespace.into(),
            version: migration.version,
            message: e.to_string(),
        })?;
        record_migration(pool, prefix, set.namespace, migration.version).await?;
        tracing::info!(namespace = set.namespace, version = migration.version, name = migration.name, "applied migration");
        count += 1;
    }
    Ok(count)
}

fn pick_sql<'m>(pool: &DbPool, migration: &'m Migration) -> &'m str {
    match pool {
        DbPool::Sqlite(_) => migration.sqlite,
        DbPool::MySql(_) => migration.mysql,
        DbPool::Postgres(_) => migration.postgres,
    }
}

async fn execute(pool: &DbPool, sql: &str) -> DbResult<()> {
    match pool {
        DbPool::Sqlite(p) => { sqlx::query(sql).execute(p).await?; }
        DbPool::MySql(p) => { sqlx::query(sql).execute(p).await?; }
        DbPool::Postgres(p) => { sqlx::query(sql).execute(p).await?; }
    }
    Ok(())
}

/// Execute a migration SQL string that may contain multiple statements separated by `;`.
/// sqlx's `query().execute()` only runs a single statement, so we split.
async fn execute_multi(pool: &DbPool, sql: &str) -> DbResult<()> {
    for statement in split_statements(sql) {
        let trimmed = statement.trim();
        if trimmed.is_empty() {
            continue;
        }
        execute(pool, trimmed).await?;
    }
    Ok(())
}

/// Naive `;` splitter. Migration SQL must not contain semicolons inside string literals or
/// comments. This is a deliberate simplifying constraint; the migrations we write follow it.
fn split_statements(sql: &str) -> Vec<&str> {
    sql.split(';').collect()
}

async fn record_migration(pool: &DbPool, prefix: &str, namespace: &str, version: i64) -> DbResult<()> {
    let table = format!("{}migrations", prefix);
    let sql = format!("INSERT INTO {table} (namespace, version) VALUES (?, ?)");
    let pg_sql = format!("INSERT INTO {table} (namespace, version) VALUES ($1, $2)");
    match pool {
        DbPool::Sqlite(p) => { sqlx::query(&sql).bind(namespace).bind(version).execute(p).await?; }
        DbPool::MySql(p) => { sqlx::query(&sql).bind(namespace).bind(version).execute(p).await?; }
        DbPool::Postgres(p) => { sqlx::query(&pg_sql).bind(namespace).bind(version).execute(p).await?; }
    }
    Ok(())
}

async fn list_applied_for(pool: &DbPool, prefix: &str, namespace: &str) -> DbResult<Vec<i64>> {
    let table = format!("{}migrations", prefix);
    let sql = format!("SELECT version FROM {table} WHERE namespace = ? ORDER BY version");
    let pg_sql = format!("SELECT version FROM {table} WHERE namespace = $1 ORDER BY version");
    let rows: Vec<i64> = match pool {
        DbPool::Sqlite(p) => sqlx::query_scalar(&sql).bind(namespace).fetch_all(p).await?,
        DbPool::MySql(p) => sqlx::query_scalar(&sql).bind(namespace).fetch_all(p).await?,
        DbPool::Postgres(p) => sqlx::query_scalar(&pg_sql).bind(namespace).fetch_all(p).await?,
    };
    Ok(rows)
}

async fn list_applied(pool: &DbPool, prefix: &str) -> DbResult<BTreeMap<String, Vec<i64>>> {
    let table = format!("{}migrations", prefix);
    let sql = format!("SELECT namespace, version FROM {table} ORDER BY namespace, version");
    let rows: Vec<(String, i64)> = match pool {
        DbPool::Sqlite(p) => sqlx::query_as(&sql).fetch_all(p).await?,
        DbPool::MySql(p) => sqlx::query_as(&sql).fetch_all(p).await?,
        DbPool::Postgres(p) => sqlx::query_as(&sql).fetch_all(p).await?,
    };
    let mut out: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    for (ns, v) in rows {
        out.entry(ns).or_default().push(v);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustcloud_config::{CacheConfig, DbType, FileConfig};
    use secrecy::SecretString;
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn cfg_sqlite(path: PathBuf) -> FileConfig {
        FileConfig {
            instanceid: "test".into(),
            secret: SecretString::new("s".into()),
            passwordsalt: SecretString::new("ps".into()),
            installed: true,
            version: "31.0.0.0".into(),
            versionstring: "31.0.0".into(),
            dbtype: DbType::Sqlite,
            dbhost: None,
            dbport: None,
            dbname: path.to_string_lossy().into(),
            dbuser: None,
            dbpassword: None,
            dbtableprefix: "oc_".into(),
            db_pool_max: 4,
            datadirectory: "/tmp".into(),
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
        }
    }

    const TEST_MIGRATIONS: &[Migration] = &[
        Migration {
            version: 1,
            name: "create_widgets",
            sqlite: "CREATE TABLE widgets (id INTEGER PRIMARY KEY, label TEXT NOT NULL)",
            mysql:  "CREATE TABLE widgets (id BIGINT PRIMARY KEY, label VARCHAR(255) NOT NULL)",
            postgres: "CREATE TABLE widgets (id BIGINT PRIMARY KEY, label TEXT NOT NULL)",
        },
        Migration {
            version: 2,
            name: "add_widget_color",
            sqlite: "ALTER TABLE widgets ADD COLUMN color TEXT",
            mysql:  "ALTER TABLE widgets ADD COLUMN color VARCHAR(32)",
            postgres: "ALTER TABLE widgets ADD COLUMN color TEXT",
        },
    ];

    #[tokio::test]
    async fn applies_migrations_in_order() {
        let dir = tempdir().unwrap();
        let cfg = cfg_sqlite(dir.path().join("test.db"));
        let pool = DbPool::connect(&cfg).await.unwrap();

        let mut runner = MigrationRunner::new(&pool, "oc_");
        runner.register(MigrationSet { namespace: "core_test", migrations: TEST_MIGRATIONS });
        let applied = runner.run().await.unwrap();
        assert_eq!(applied, 2);

        let map = runner.applied().await.unwrap();
        assert_eq!(map.get("core_test"), Some(&vec![1, 2]));

        pool.close().await;
    }

    #[tokio::test]
    async fn second_run_is_idempotent() {
        let dir = tempdir().unwrap();
        let cfg = cfg_sqlite(dir.path().join("test.db"));
        let pool = DbPool::connect(&cfg).await.unwrap();

        let mut runner = MigrationRunner::new(&pool, "oc_");
        runner.register(MigrationSet { namespace: "core_test", migrations: TEST_MIGRATIONS });
        let first = runner.run().await.unwrap();
        let second = runner.run().await.unwrap();
        assert_eq!(first, 2);
        assert_eq!(second, 0);

        pool.close().await;
    }

    #[tokio::test]
    async fn separate_namespaces_track_independently() {
        let dir = tempdir().unwrap();
        let cfg = cfg_sqlite(dir.path().join("test.db"));
        let pool = DbPool::connect(&cfg).await.unwrap();

        static NS_A_MIGRATIONS: &[Migration] = &[Migration {
            version: 1, name: "a1",
            sqlite: "CREATE TABLE a (id INTEGER PRIMARY KEY)",
            mysql: "", postgres: "",
        }];
        static NS_B_MIGRATIONS: &[Migration] = &[Migration {
            version: 1, name: "b1",
            sqlite: "CREATE TABLE b (id INTEGER PRIMARY KEY)",
            mysql: "", postgres: "",
        }];

        let mut runner = MigrationRunner::new(&pool, "oc_");
        runner.register(MigrationSet { namespace: "ns_a", migrations: NS_A_MIGRATIONS });
        runner.register(MigrationSet { namespace: "ns_b", migrations: NS_B_MIGRATIONS });
        runner.run().await.unwrap();

        let map = runner.applied().await.unwrap();
        assert_eq!(map.get("ns_a"), Some(&vec![1]));
        assert_eq!(map.get("ns_b"), Some(&vec![1]));
        pool.close().await;
    }
}
```

- [ ] **Step 2: Re-export `migrate` symbols**

Modify `crates/rustcloud-db/src/lib.rs`:
```rust
//! Multi-dialect database layer for Rustcloud.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §6.

mod error;
mod pool;

pub mod migrate;

pub use error::{DbError, DbResult};
pub use migrate::{Migration, MigrationRunner, MigrationSet};
pub use pool::DbPool;
```

- [ ] **Step 3: Run tests**

Run:
```
cargo test -p rustcloud-db --lib
```
Expected: 6 tests pass (3 from Task 7, 3 new). If any fail with sqlx connection errors, the SQLite URL building may need adjusting — confirm `sqlite:./...?mode=rwc` opens the file in read-write-create mode.

- [ ] **Step 4: Commit**

```
git add crates/rustcloud-db
git commit -m "feat(db): add per-namespace MigrationRunner with idempotent reruns

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 9: Core migration 0001 — `oc_appconfig`

**Files:**
- Create: `migrations/core/0001_initial/sqlite.sql`
- Create: `migrations/core/0001_initial/mysql.sql`
- Create: `migrations/core/0001_initial/postgres.sql`
- Create: `crates/rustcloud-db/src/core_migrations.rs`
- Modify: `crates/rustcloud-db/src/lib.rs`

In this phase the only core table we create (beyond `oc_migrations`, which the runner manages itself) is `oc_appconfig`. Users / sessions / files etc. land in their own sub-projects with their own migration sources.

- [ ] **Step 1: Write the SQLite SQL**

Write `migrations/core/0001_initial/sqlite.sql`:
```sql
CREATE TABLE oc_appconfig (
    appid       TEXT    NOT NULL,
    configkey   TEXT    NOT NULL,
    configvalue TEXT    NOT NULL DEFAULT '',
    PRIMARY KEY (appid, configkey)
);

CREATE INDEX appconfig_appid_idx ON oc_appconfig(appid);
```

- [ ] **Step 2: Write the MySQL SQL**

Write `migrations/core/0001_initial/mysql.sql`:
```sql
CREATE TABLE oc_appconfig (
    appid       VARCHAR(32)    NOT NULL,
    configkey   VARCHAR(64)    NOT NULL,
    configvalue LONGTEXT       NOT NULL,
    PRIMARY KEY (appid, configkey)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE INDEX appconfig_appid_idx ON oc_appconfig(appid);
```

- [ ] **Step 3: Write the Postgres SQL**

Write `migrations/core/0001_initial/postgres.sql`:
```sql
CREATE TABLE oc_appconfig (
    appid       VARCHAR(32)  NOT NULL,
    configkey   VARCHAR(64)  NOT NULL,
    configvalue TEXT         NOT NULL DEFAULT '',
    PRIMARY KEY (appid, configkey)
);

CREATE INDEX appconfig_appid_idx ON oc_appconfig(appid);
```

- [ ] **Step 4: Embed the SQL into a `core_migrations` module**

Write `crates/rustcloud-db/src/core_migrations.rs`:
```rust
//! Migrations for the `core` namespace.
//!
//! The SQL is `include_str!`'d from `migrations/core/`. Adding a new migration:
//!   1. Add files at `migrations/core/<NNNN>_<name>/{sqlite,mysql,postgres}.sql`.
//!   2. Append a `Migration` to `CORE_MIGRATIONS` below with a strictly increasing `version`.
//!   3. Run `cargo xtask prepare` (later phase) to refresh the offline sqlx cache.

use crate::migrate::{Migration, MigrationSet};

pub const CORE_NAMESPACE: &str = "core";

pub const CORE_MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial",
        sqlite: include_str!("../../../migrations/core/0001_initial/sqlite.sql"),
        mysql:  include_str!("../../../migrations/core/0001_initial/mysql.sql"),
        postgres: include_str!("../../../migrations/core/0001_initial/postgres.sql"),
    },
];

pub fn core_set() -> MigrationSet {
    MigrationSet {
        namespace: CORE_NAMESPACE,
        migrations: CORE_MIGRATIONS,
    }
}
```

- [ ] **Step 5: Re-export the `core_set()` helper**

Modify `crates/rustcloud-db/src/lib.rs`:
```rust
//! Multi-dialect database layer for Rustcloud.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §6.

mod core_migrations;
mod error;
mod pool;

pub mod migrate;

pub use core_migrations::{core_set, CORE_NAMESPACE};
pub use error::{DbError, DbResult};
pub use migrate::{Migration, MigrationRunner, MigrationSet};
pub use pool::DbPool;
```

- [ ] **Step 6: Add a unit test that the core migration applies cleanly against SQLite**

Append to `crates/rustcloud-db/src/core_migrations.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DbPool, MigrationRunner};
    use rustcloud_config::{CacheConfig, DbType, FileConfig};
    use secrecy::SecretString;
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn cfg_sqlite(path: PathBuf) -> FileConfig {
        FileConfig {
            instanceid: "test".into(),
            secret: SecretString::new("s".into()),
            passwordsalt: SecretString::new("ps".into()),
            installed: true,
            version: "31.0.0.0".into(),
            versionstring: "31.0.0".into(),
            dbtype: DbType::Sqlite,
            dbhost: None,
            dbport: None,
            dbname: path.to_string_lossy().into(),
            dbuser: None,
            dbpassword: None,
            dbtableprefix: "oc_".into(),
            db_pool_max: 4,
            datadirectory: "/tmp".into(),
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
        }
    }

    #[tokio::test]
    async fn core_migration_applies_against_sqlite() {
        let dir = tempdir().unwrap();
        let cfg = cfg_sqlite(dir.path().join("test.db"));
        let pool = DbPool::connect(&cfg).await.unwrap();

        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        let applied = runner.run().await.unwrap();
        assert_eq!(applied, 1);

        // Verify oc_appconfig exists and accepts a row.
        match &pool {
            DbPool::Sqlite(p) => {
                sqlx::query("INSERT INTO oc_appconfig (appid, configkey, configvalue) VALUES (?, ?, ?)")
                    .bind("core").bind("instanceid").bind("hello")
                    .execute(p).await.unwrap();
                let value: String = sqlx::query_scalar("SELECT configvalue FROM oc_appconfig WHERE appid = ? AND configkey = ?")
                    .bind("core").bind("instanceid")
                    .fetch_one(p).await.unwrap();
                assert_eq!(value, "hello");
            }
            _ => unreachable!(),
        }
        pool.close().await;
    }
}
```

- [ ] **Step 7: Run tests**

Run:
```
cargo test -p rustcloud-db --lib
```
Expected: 7 tests pass.

- [ ] **Step 8: Commit**

```
git add crates/rustcloud-db migrations
git commit -m "feat(db): add core 0001 migration creating oc_appconfig

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 10: rustcloud-server `migrate` subcommand

**Files:**
- Modify: `crates/rustcloud-server/Cargo.toml`
- Modify: `crates/rustcloud-server/src/main.rs`

- [ ] **Step 1: Add `rustcloud-db` dependency**

Modify `crates/rustcloud-server/Cargo.toml` — replace `[dependencies]`:
```toml
[dependencies]
anyhow.workspace = true
clap.workspace = true
rustcloud-config.workspace = true
rustcloud-db.workspace = true
tokio.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
```

- [ ] **Step 2: Wire up the migrate command**

Modify `crates/rustcloud-server/src/main.rs` — replace the `match` block in `main`:
```rust
    match cli.command() {
        Cmd::Version => {
            println!("rustcloud-server {} (build target subproject: platform-core)", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Cmd::Serve => {
            let config = rustcloud_config::load(&cli.config, &[])?;
            info!(
                instanceid = %config.instanceid,
                dbtype = %config.dbtype.as_str(),
                "loaded config; serve subcommand not yet implemented"
            );
            anyhow::bail!("`serve` is implemented in a later phase");
        }
        Cmd::Migrate => {
            let config = rustcloud_config::load(&cli.config, &[])?;
            info!(dbtype = %config.dbtype.as_str(), "connecting to database");

            let pool = rustcloud_db::DbPool::connect(&config).await?;
            info!(dialect = pool.dialect(), "connected");

            let mut runner = rustcloud_db::MigrationRunner::new(&pool, &config.dbtableprefix);
            runner.register(rustcloud_db::core_set());
            let applied = runner.run().await?;
            info!(applied, "migrations complete");

            pool.close().await;
            Ok(())
        }
    }
```

- [ ] **Step 3: Build the workspace**

Run:
```
cargo build
```
Expected: clean.

- [ ] **Step 4: Smoke-test against a SQLite config**

Create `fixture.toml`:
```toml
instanceid = "smoketest"
secret = "s"
passwordsalt = "ps"
installed = true
version = "31.0.0.0"
versionstring = "31.0.0"
dbtype = "sqlite"
dbname = "smoketest.db"
datadirectory = "./data"
trusted_domains = ["localhost"]
```

Run:
```
cargo run -p rustcloud-server -- --config fixture.toml migrate
```

Expected output (log fields, JSON or plain depending on terminal):
- "connecting to database" with `dbtype=sqlite`
- "connected" with `dialect=sqlite`
- "applied migration" with `namespace=core version=1 name=initial`
- "migrations complete" with `applied=1`

Verify the file `smoketest.db` exists.

Re-run the same command. Expected: "migrations complete" with `applied=0` (idempotent).

Clean up: delete `smoketest.db` and `fixture.toml`.

- [ ] **Step 5: Commit**

```
git add crates/rustcloud-server
git commit -m "feat(server): implement migrate subcommand against DbPool

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 11: `cargo xtask check-all` exercises the multi-dialect path

The existing `check-all` just runs `cargo test --workspace` which only hits SQLite (in-process). Multi-dialect tests live in `tests/migrate_end_to_end.rs` (Task 13) and use `testcontainers-rs` so they're picky about Docker availability. We don't change `check-all` for now — it stays unit-test-only locally. CI gets the docker-driven matrix in Task 12.

This task adds a `cargo xtask up`/`cargo xtask down` convenience that starts/stops the dev DBs.

**Files:**
- Modify: `xtask/src/main.rs`

- [ ] **Step 1: Extend the xtask CLI**

Modify `xtask/src/main.rs` — replace the `enum Cmd` and `match` block:
```rust
#[derive(Subcommand)]
enum Cmd {
    /// Run `cargo fmt --check && cargo clippy && cargo test`.
    CheckAll,
    /// Start MySQL + Postgres via docker compose.
    Up,
    /// Stop the dev docker compose stack.
    Down,
    /// Stub — implemented in a later phase.
    Build,
    /// Stub — implemented in a later phase.
    Dev,
    /// Stub — implemented in a later phase (no query! macros yet).
    Prepare,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Cmd::CheckAll => check_all(),
        Cmd::Up => compose(&["up", "-d", "--wait"]),
        Cmd::Down => compose(&["down", "-v"]),
        Cmd::Build => bail!("`build` is implemented in a later phase"),
        Cmd::Dev => bail!("`dev` is implemented in a later phase"),
        Cmd::Prepare => bail!("`prepare` is implemented in a later phase"),
    }
}

fn compose(args: &[&str]) -> Result<()> {
    let mut all = vec!["compose", "-f", "dev/docker-compose.yml"];
    all.extend_from_slice(args);
    run("docker", &all)
}
```

- [ ] **Step 2: Verify the subcommands list updated**

Run:
```
cargo xtask --help
```
Expected: lists `check-all`, `up`, `down`, `build`, `dev`, `prepare`.

- [ ] **Step 3: Manually verify `up` and `down` (skip if Docker isn't installed locally)**

Run:
```
cargo xtask up
```
Expected: docker compose pulls/starts the two services; healthchecks pass.

Run:
```
docker ps --format "{{.Names}} {{.State}}"
```
Expected: both `mysql` and `postgres` containers are running.

Run:
```
cargo xtask down
```
Expected: containers stopped and volumes removed.

If Docker isn't installed locally, document that as a prerequisite in the next task's README update; CI handles it.

- [ ] **Step 4: Commit**

```
git add xtask
git commit -m "feat(xtask): add `up`/`down` to manage dev MySQL + Postgres

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 12: CI workflow

**Files:**
- Create: `.github/workflows/ci.yml`

CI runs `cargo xtask check-all` on Linux, plus the multi-dialect integration tests (Task 13's tests) against MySQL + Postgres services.

- [ ] **Step 1: Write the CI workflow**

Write `.github/workflows/ci.yml`:
```yaml
name: CI

on:
  push:
    branches: [master, main]
  pull_request:

env:
  CARGO_TERM_COLOR: always
  RUST_BACKTRACE: 1
  SQLX_OFFLINE: "true"

jobs:
  fmt-and-clippy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all -- --check
      - run: cargo clippy --workspace --all-targets -- -D warnings

  test-sqlite:
    runs-on: ubuntu-latest
    needs: fmt-and-clippy
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: cargo test --workspace --lib --bins

  test-multidialect:
    runs-on: ubuntu-latest
    needs: fmt-and-clippy
    services:
      mysql:
        image: mysql:8.4
        env:
          MYSQL_ROOT_PASSWORD: rustcloud
          MYSQL_DATABASE: rustcloud
          MYSQL_USER: rustcloud
          MYSQL_PASSWORD: rustcloud
        ports:
          - 3307:3306
        options: >-
          --health-cmd="mysqladmin ping -prustcloud"
          --health-interval=5s
          --health-timeout=5s
          --health-retries=12
      postgres:
        image: postgres:16
        env:
          POSTGRES_DB: rustcloud
          POSTGRES_USER: rustcloud
          POSTGRES_PASSWORD: rustcloud
        ports:
          - 5433:5432
        options: >-
          --health-cmd="pg_isready -U rustcloud"
          --health-interval=5s
          --health-timeout=5s
          --health-retries=12
    env:
      RUSTCLOUD_TEST_MYSQL_URL:    mysql://rustcloud:rustcloud@127.0.0.1:3307/rustcloud
      RUSTCLOUD_TEST_POSTGRES_URL: postgres://rustcloud:rustcloud@127.0.0.1:5433/rustcloud
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: cargo test --test migrate_end_to_end -- --include-ignored
```

The `test-multidialect` job runs only the integration test created in Task 13. It uses GitHub Actions service containers (cheaper to start than `testcontainers-rs` in CI) and exposes their URLs via env vars that the test reads.

- [ ] **Step 2: Commit**

```
git add .github
git commit -m "ci: add fmt+clippy and multi-dialect test workflow

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 13: End-to-end multi-dialect integration test

**Files:**
- Create: `crates/rustcloud-db/tests/migrate_end_to_end.rs`

Cargo integration tests live in a package's `tests/` directory; we put this one inside `rustcloud-db` so it has direct access to the crate's API.

- [ ] **Step 1: Write the integration test**

Write `crates/rustcloud-db/tests/migrate_end_to_end.rs`:
```rust
//! End-to-end migrate flow per dialect.
//!
//! Reads URLs from env vars; SQLite uses a temp file. MySQL and Postgres tests are
//! `#[ignore]` by default so contributors without Docker aren't blocked. CI runs
//! `cargo test -- --include-ignored` to enable them.

use rustcloud_config::{CacheConfig, DbType, FileConfig};
use rustcloud_db::{core_set, DbPool, MigrationRunner};
use secrecy::SecretString;
use std::net::SocketAddr;
use std::path::PathBuf;
use tempfile::tempdir;

fn base_config() -> FileConfig {
    FileConfig {
        instanceid: "it".into(),
        secret: SecretString::new("s".into()),
        passwordsalt: SecretString::new("ps".into()),
        installed: true,
        version: "31.0.0.0".into(),
        versionstring: "31.0.0".into(),
        dbtype: DbType::Sqlite,
        dbhost: None,
        dbport: None,
        dbname: String::new(),
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
    }
}

async fn assert_appconfig_table_usable(pool: &DbPool) {
    // Cross-dialect placeholders: SQLite/MySQL use `?`, Postgres uses `$N`.
    // For a smoke test, write a row using the dialect-appropriate query.
    let insert_sql: &str = match pool {
        DbPool::Postgres(_) => "INSERT INTO oc_appconfig (appid, configkey, configvalue) VALUES ($1, $2, $3)",
        _ => "INSERT INTO oc_appconfig (appid, configkey, configvalue) VALUES (?, ?, ?)",
    };
    let select_sql: &str = match pool {
        DbPool::Postgres(_) => "SELECT configvalue FROM oc_appconfig WHERE appid = $1 AND configkey = $2",
        _ => "SELECT configvalue FROM oc_appconfig WHERE appid = ? AND configkey = ?",
    };
    match pool {
        DbPool::Sqlite(p) => {
            sqlx::query(insert_sql).bind("core").bind("k").bind("v").execute(p).await.unwrap();
            let v: String = sqlx::query_scalar(select_sql).bind("core").bind("k").fetch_one(p).await.unwrap();
            assert_eq!(v, "v");
        }
        DbPool::MySql(p) => {
            sqlx::query(insert_sql).bind("core").bind("k").bind("v").execute(p).await.unwrap();
            let v: String = sqlx::query_scalar(select_sql).bind("core").bind("k").fetch_one(p).await.unwrap();
            assert_eq!(v, "v");
        }
        DbPool::Postgres(p) => {
            sqlx::query(insert_sql).bind("core").bind("k").bind("v").execute(p).await.unwrap();
            let v: String = sqlx::query_scalar(select_sql).bind("core").bind("k").fetch_one(p).await.unwrap();
            assert_eq!(v, "v");
        }
    }
}

#[tokio::test]
async fn migrate_sqlite() {
    let dir = tempdir().unwrap();
    let mut cfg = base_config();
    cfg.dbname = dir.path().join("it.db").to_string_lossy().into();

    let pool = DbPool::connect(&cfg).await.unwrap();
    let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
    runner.register(core_set());
    let applied = runner.run().await.unwrap();
    assert_eq!(applied, 1);

    assert_appconfig_table_usable(&pool).await;
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires MySQL — run with --include-ignored after `cargo xtask up`"]
async fn migrate_mysql() {
    let url = std::env::var("RUSTCLOUD_TEST_MYSQL_URL")
        .unwrap_or_else(|_| "mysql://rustcloud:rustcloud@127.0.0.1:3307/rustcloud".into());
    let cfg = mysql_config_from_url(&url);
    let pool = DbPool::connect(&cfg).await.unwrap();

    // Tests may share a database; drop our migration tracking + appconfig first.
    if let DbPool::MySql(p) = &pool {
        let _ = sqlx::query("DROP TABLE IF EXISTS oc_appconfig").execute(p).await;
        let _ = sqlx::query("DROP TABLE IF EXISTS oc_migrations").execute(p).await;
    }

    let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
    runner.register(core_set());
    let applied = runner.run().await.unwrap();
    assert_eq!(applied, 1);

    assert_appconfig_table_usable(&pool).await;
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires Postgres — run with --include-ignored after `cargo xtask up`"]
async fn migrate_postgres() {
    let url = std::env::var("RUSTCLOUD_TEST_POSTGRES_URL")
        .unwrap_or_else(|_| "postgres://rustcloud:rustcloud@127.0.0.1:5433/rustcloud".into());
    let cfg = postgres_config_from_url(&url);
    let pool = DbPool::connect(&cfg).await.unwrap();

    if let DbPool::Postgres(p) = &pool {
        let _ = sqlx::query("DROP TABLE IF EXISTS oc_appconfig").execute(p).await;
        let _ = sqlx::query("DROP TABLE IF EXISTS oc_migrations").execute(p).await;
    }

    let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
    runner.register(core_set());
    let applied = runner.run().await.unwrap();
    assert_eq!(applied, 1);

    assert_appconfig_table_usable(&pool).await;
    pool.close().await;
}

// --- URL → config helpers (parsing a URL is the simplest way to populate FileConfig
//     fields from env without reinventing the wheel) ---

fn mysql_config_from_url(url: &str) -> FileConfig {
    let parsed = parse_url(url);
    let mut cfg = base_config();
    cfg.dbtype = DbType::Mysql;
    cfg.dbhost = Some(parsed.host);
    cfg.dbport = Some(parsed.port);
    cfg.dbuser = Some(parsed.user);
    cfg.dbpassword = parsed.password.map(SecretString::new);
    cfg.dbname = parsed.database;
    cfg
}

fn postgres_config_from_url(url: &str) -> FileConfig {
    let parsed = parse_url(url);
    let mut cfg = base_config();
    cfg.dbtype = DbType::Pgsql;
    cfg.dbhost = Some(parsed.host);
    cfg.dbport = Some(parsed.port);
    cfg.dbuser = Some(parsed.user);
    cfg.dbpassword = parsed.password.map(SecretString::new);
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
    // Format: scheme://user:pass@host:port/db
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
    ParsedUrl { user, password, host, port, database: database.to_string() }
}
```

- [ ] **Step 2: Run the SQLite test locally**

Run:
```
cargo test -p rustcloud-db --test migrate_end_to_end migrate_sqlite
```
Expected: PASS.

- [ ] **Step 3: Run the full suite locally with docker up**

If Docker is available:
```
cargo xtask up
cargo test -p rustcloud-db --test migrate_end_to_end -- --include-ignored
cargo xtask down
```
Expected: all three tests pass.

If Docker is not available locally: skip this step. CI will verify (Task 12).

- [ ] **Step 4: Commit**

```
git add crates/rustcloud-db/tests
git commit -m "test(db): add end-to-end multi-dialect migrate integration test

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 14: Phase 1 acceptance + README

**Files:**
- Modify: `README.md`
- Create: `docs/superpowers/plans/2026-05-10-platform-core-phase-1-foundations.changelog.md`

The phase is acceptance-tested by:

1. `cargo build --workspace` succeeds.
2. `cargo xtask check-all` is green.
3. `cargo test -p rustcloud-db --test migrate_end_to_end migrate_sqlite` is green.
4. CI's `test-multidialect` job is green against MySQL + Postgres.
5. `cargo run -p rustcloud-server -- --config <fixture> migrate` connects, migrates, and exits cleanly against all three backends.

- [ ] **Step 1: Update README with what works now**

Replace `README.md`:
```markdown
# Rustcloud

A Rust port of [Nextcloud server](https://github.com/nextcloud/server), with a Dioxus frontend.

**Status:** early — Phase 1 (Foundations) complete. The binary can boot, load layered config, connect to SQLite/MySQL/Postgres, run core migrations, and exit. No HTTP surface yet; later phases add it.

See `docs/superpowers/specs/` for design specs and `docs/superpowers/plans/` for implementation plans.

## Quick start

```bash
# 1. Copy the example config and edit it.
cp config/config.toml.example config/config.toml
# (Set `installed = true` and pick a `dbtype`.)

# 2a. SQLite: nothing else needed. Skip to step 3.

# 2b. MySQL or Postgres: start the dev DBs.
cargo xtask up

# 3. Run migrations.
cargo run -p rustcloud-server -- migrate
```

## Development

```bash
cargo xtask check-all     # fmt + clippy + tests (SQLite only)
cargo xtask up            # start MySQL + Postgres for multi-dialect tests
cargo test -p rustcloud-db --test migrate_end_to_end -- --include-ignored
cargo xtask down          # stop dev DBs
```

## Workspace layout

- `crates/rustcloud-config` — layered TOML config loader (figment-based).
- `crates/rustcloud-db` — `DbPool` enum over Sqlite/MySql/Postgres, `MigrationRunner`.
- `crates/rustcloud-server` — the binary; CLI, tracing, lifecycle.
- `xtask/` — project automation (`check-all`, `up`, `down`).
- `migrations/core/` — core SQL migrations, per-dialect.

Future phases add `rustcloud-cache`, `-i18n`, `-ocs`, `-core`, `-http`, `-ui`.

## License

AGPL-3.0-or-later.
```

- [ ] **Step 2: Final acceptance check — full clean build + tests**

Run:
```
cargo clean
cargo xtask check-all
```
Expected: PASS end-to-end (fmt + clippy + SQLite tests). Time: 1-5 minutes depending on machine.

If Docker is available:
```
cargo xtask up
cargo test -p rustcloud-db --test migrate_end_to_end -- --include-ignored
cargo xtask down
```
Expected: 3 tests pass.

- [ ] **Step 3: Write the Phase 1 changelog**

Write `docs/superpowers/plans/2026-05-10-platform-core-phase-1-foundations.changelog.md`:
```markdown
# Phase 1 (Foundations) — Changelog

Completed: <DATE>

## What works

- Cargo workspace with `rustcloud-config`, `rustcloud-db`, `rustcloud-server`, `xtask`.
- Layered config: TOML base + `config.local.toml` overlay + `RUSTCLOUD_*` env vars + CLI overrides. Sensitive fields use `secrecy::SecretString`.
- `DbPool` enum over `sqlx::SqlitePool` / `MySqlPool` / `PgPool`. `connect()` dispatches on `config.dbtype`.
- `MigrationRunner` with namespace tracking (`oc_migrations`). Idempotent re-runs. Per-dialect SQL.
- Core migration 0001 creates `oc_appconfig` matching Nextcloud's shape across all three dialects.
- `rustcloud-server` subcommands: `version`, `migrate`, `serve` (stubbed).
- CI: fmt + clippy + SQLite tests + multi-dialect tests against GitHub Actions service containers.
- `cargo xtask` commands: `check-all`, `up`, `down`.

## What's deferred

- HTTP surface: Phase 3 (`rustcloud-http`).
- UI: Phase 4 (`rustcloud-ui` + Dioxus Fullstack).
- Cache trait + memory impl: Phase 2.
- i18n loader: Phase 2.
- OCS envelope + capabilities: Phase 2.
- AppState facade: Phase 2.
- `cargo xtask prepare` / `dev` / `build`: filled in as later phases need them.

## Known limitations

- `MigrationRunner` doesn't wrap migration SQL in a transaction (DDL portability issues across MySQL). Rely on idempotent SQL (`CREATE TABLE IF NOT EXISTS`, etc.) for safety.
- The migration runner splits SQL on `;` naively; migration files must not contain semicolons inside string literals or comments.
- No offline sqlx cache yet — no `sqlx::query!` macros used in Phase 1. Phase 2 introduces the first compile-time-checked queries and the `cargo xtask prepare` flow.
```

Fill in `<DATE>` with the actual completion date.

- [ ] **Step 4: Commit**

```
git add README.md docs/superpowers/plans/2026-05-10-platform-core-phase-1-foundations.changelog.md
git commit -m "docs: phase 1 acceptance — README + changelog

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

- [ ] **Step 5: Push the branch (if using a remote)**

Confirm with the user before pushing. If pushing:
```
git push -u origin master
```

---

## Phase 1 Self-Review (executor reads + applies before declaring complete)

After all 14 tasks land, verify against the spec's acceptance criteria (§13):

| Spec criterion | Phase 1 status |
|---|---|
| 1. `cargo xtask check-all` passes against all three backends | ✓ (CI runs all three; locally SQLite only) |
| 2. `cargo xtask build` produces a static binary with embedded UI assets | Deferred to Phase 4 (no UI yet) |
| 3. Binary boots, runs migrations, serves traffic against all three DBs | Boots + migrates ✓; serves traffic ✗ (Phase 3) |
| 4. `curl /status.php` returns Nextcloud-shaped JSON | Deferred to Phase 3 |
| 5. `curl /ocs/v2.php/cloud/capabilities` returns OCS envelope | Deferred to Phase 3 |
| 6. Browser at `/` SSR'd + hydrated | Deferred to Phase 4 |
| 7. `/login` flow | Deferred to Phase 4 |
| 8. Middleware enforcement (trusted-domain, proxy, CSRF, security headers) | Deferred to Phase 3 |
| 9. Single + multi-dialect tests green in CI | ✓ for migrations |

Phase 1's local acceptance: criteria 1 + 3 (migrate portion) + 9 (migrate portion) are green. Remaining criteria belong to later phases.

If any of the green criteria fail, fix before declaring phase complete.

---
