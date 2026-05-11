# Platform Core — Phase 5: Test Scale-Out + Ship Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close out platform-core: relax CSP so the WASM bundle actually executes (completing spec §13 #6 hydration), emit proper 404 status from the Dioxus router, add a Playwright E2E test that verifies SSR-then-hydration in a real headless Chromium, centralize lint policy, consolidate the duplicated test fixture, instrument the cache failure paths, expand the `version` subcommand, roll out rustdoc on remaining public types, write `CONTRIBUTING.md`, and mark every spec §13 acceptance criterion green.

**Architecture:** Most of Phase 5 is targeted polish — small, focused changes across many crates. Two larger changes are notable: (a) `SecurityHeadersLayer` becomes content-type-aware so HTML responses get a CSP that allows the WASM bundle (`script-src 'self' 'wasm-unsafe-eval'`) while JSON/XML responses keep the restrictive `default-src 'none'`; (b) a new `e2e/` directory at the workspace root holds the Playwright project, with a dedicated CI job that builds the release server, starts it on an ephemeral port, and runs the headless browser test. Hydration is verified by a `data-hydrated` attribute that flips from `"false"` (SSR) to `"true"` (post-mount via `use_effect`).

**Tech Stack:** Rust 1.85, existing Phase 1-4 workspace, `tracing` instrumentation, `vergen-gix` (or a small `build.rs`) for git SHA capture, Node 20 LTS + `@playwright/test 1.49` + headless Chromium for the E2E test, no other new Rust crates.

**Parent spec:** `docs/superpowers/specs/2026-05-10-platform-core-design.md` §13 (acceptance criteria). Phase 5 also addresses the carry-over follow-ups documented in every previous phase's changelog.

**Previous phase:** Phase 4 ended at commit `f793011` on the public remote. Workspace has 9 crates + xtask + ~150 tests. SSR works, asset pipeline works, integration tests run in CI. The remaining blocker for spec §13 #6 is CSP — the WASM bundle is blocked from executing by `default-src 'none'`.

---

## Conventions (carry-over from earlier phases)

- **Commits:** Conventional Commits (`feat`, `fix`, `chore`, `docs`, `test`, `refactor`) with the Co-Authored-By trailer.
- **TDD:** Failing test first when feasible. Some Phase 5 tasks are purely additive (docs, lints) and don't have a test surface.
- **rustfmt:** Run `cargo fmt --all` after writing files. Authorized at every task boundary.
- **`cargo xtask check-all` must stay green after every commit.**
- **Plan-bug protocol:** if verbatim code fails to compile/test, fix minimally and report DONE_WITH_CONCERNS with a clear diff.
- **No new Rust workspace deps** unless explicitly listed in a task. Phase 5 deliberately avoids adding crates.

---

## File Structure (Phase 5 additions and modifications)

```
rustcloud/
├── Cargo.toml                                       # [workspace.lints] + vergen-gix
├── CONTRIBUTING.md                                  # NEW — MSRV + dev tooling notes
├── crates/
│   ├── rustcloud-config/
│   │   ├── Cargo.toml                               # [features] test-support
│   │   └── src/
│   │       ├── lib.rs                               # gate test_support behind feature
│   │       └── test_support.rs                      # NEW — minimal_sqlite_config helper
│   ├── rustcloud-core/
│   │   ├── Cargo.toml                               # drop unused deps; +tracing in non-test
│   │   └── src/
│   │       └── appconfig.rs                         # CACHE_TTL const + tracing::warn!
│   ├── rustcloud-http/
│   │   └── src/
│   │       ├── middleware/security_headers.rs       # content-type-aware CSP
│   │       └── routes/ui.rs                         # 404 on NotFoundRoute
│   ├── rustcloud-server/
│   │   ├── Cargo.toml                               # vergen-gix build dep
│   │   ├── build.rs                                 # NEW — capture git SHA
│   │   └── src/main.rs                              # version subcommand expansion
│   └── rustcloud-ui/
│       └── src/
│           ├── app.rs                               # HomeRoute hydration marker
│           └── pages/{home,login,not_found}.rs      # rustdoc rollout
├── e2e/                                             # NEW — Playwright project
│   ├── .gitignore
│   ├── package.json
│   ├── playwright.config.ts
│   └── tests/
│       └── hydration.spec.ts
├── .github/workflows/ci.yml                         # +e2e job
└── docs/superpowers/plans/
    └── 2026-05-10-platform-core-phase-5-ship.changelog.md   # NEW
```

Every test file across the workspace gets its hand-rolled `FileConfig` literal replaced with a call to `rustcloud_config::test_support::minimal_sqlite_config(path)`. About 10 files affected, each is a one-shot diff.

---

## Task 1: Test fixture consolidation

**Files:**
- Modify: `crates/rustcloud-config/Cargo.toml`
- Modify: `crates/rustcloud-config/src/lib.rs`
- Create: `crates/rustcloud-config/src/test_support.rs`
- Modify (dev-deps): `crates/rustcloud-cache/Cargo.toml` — no change needed (doesn't use FileConfig)
- Modify (dev-deps): `crates/rustcloud-db/Cargo.toml`, `crates/rustcloud-core/Cargo.toml`, `crates/rustcloud-http/Cargo.toml`, `crates/rustcloud-ui/Cargo.toml`
- Modify (test fixtures): all files that hand-rolled `FileConfig` test literals.

Centralize the `~10`-fold duplicated `cfg_sqlite(...) -> FileConfig` helper. New helper lives in `rustcloud-config::test_support` behind a `test-support` feature flag so it's only compiled into builds that need it.

- [ ] **Step 1: Add the feature flag to `rustcloud-config/Cargo.toml`**

Append to `crates/rustcloud-config/Cargo.toml`:

```toml
[features]
test-support = []
```

- [ ] **Step 2: Write `crates/rustcloud-config/src/test_support.rs`**

```rust
//! Test-only helpers for building `FileConfig` instances. Compiled only when
//! the `test-support` feature is enabled.
//!
//! The helper produces a minimal, valid SQLite-backed config suitable for
//! integration and unit tests. Callers mutate specific fields to exercise
//! particular code paths (e.g., setting `bootstrap_admin`).

use crate::types::{BootstrapAdminConfig, CacheConfig, DbType, FileConfig};
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
```

- [ ] **Step 3: Gate `test_support` behind the feature in `lib.rs`**

Modify `crates/rustcloud-config/src/lib.rs`. Find the existing module list and add:

```rust
#[cfg(feature = "test-support")]
pub mod test_support;
```

(Order alphabetically with the existing `mod loader; mod types;` declarations.)

- [ ] **Step 4: Verify the helper builds and its own tests pass**

```
cargo test -p rustcloud-config --features test-support
```

Expected: 14 prior tests + 2 new `test_support::tests` tests pass.

```
cargo build -p rustcloud-config
```

Expected: clean (without the feature, `test_support` isn't compiled — no warnings either).

- [ ] **Step 5: Add `test-support` feature to consumer dev-deps**

Modify each consumer's `Cargo.toml` — change `rustcloud-config.workspace = true` in `[dev-dependencies]` to:

```toml
rustcloud-config = { workspace = true, features = ["test-support"] }
```

Apply to:
- `crates/rustcloud-db/Cargo.toml`
- `crates/rustcloud-core/Cargo.toml`
- `crates/rustcloud-http/Cargo.toml`
- `crates/rustcloud-ui/Cargo.toml`

(`rustcloud-config` itself doesn't need the change — `[features] test-support = []` is sufficient and `cargo test --features test-support` enables it for its own tests.)

- [ ] **Step 6: Replace hand-rolled fixtures**

Audit every test file that constructs `FileConfig { ... }` by hand. Each one becomes a one-line call:

```rust
let cfg = rustcloud_config::test_support::minimal_sqlite_config(path);
```

Or for tests needing `bootstrap_admin`:

```rust
let cfg = rustcloud_config::test_support::sqlite_config_with_admin(path, "admin", &hash);
```

If a test mutates other fields, do so via direct field assignment on the returned struct (it's `Clone + Debug`):

```rust
let mut cfg = rustcloud_config::test_support::minimal_sqlite_config(path);
cfg.dbtype = DbType::Mysql; // example
```

Files known to have `cfg_sqlite` / `cfg_with_admin` / `base_config` style helpers (from previous-phase reviews):

1. `crates/rustcloud-db/src/pool.rs` — `tests` module
2. `crates/rustcloud-db/src/migrate.rs` — `tests` module
3. `crates/rustcloud-db/src/core_migrations.rs` — `tests` module
4. `crates/rustcloud-db/tests/migrate_end_to_end.rs` — `base_config` + URL-config builders
5. `crates/rustcloud-core/src/appconfig.rs` — `tests::cfg_sqlite`
6. `crates/rustcloud-core/src/state.rs` — `tests::cfg_sqlite`
7. `crates/rustcloud-core/tests/app_state_build.rs` — `cfg_sqlite`
8. `crates/rustcloud-http/src/routes/status.rs` — `tests::cfg_sqlite`
9. `crates/rustcloud-http/src/routes/login.rs` — `tests::cfg_with_admin`
10. `crates/rustcloud-http/src/routes/ocs/capabilities.rs` — `tests::cfg_sqlite`
11. `crates/rustcloud-http/tests/cors.rs` — `cfg`
12. `crates/rustcloud-http/tests/http_end_to_end.rs` — `cfg`
13. `crates/rustcloud-ui/tests/ssr_routes.rs` — `cfg`

For each, remove the local helper function entirely and replace its call sites with `rustcloud_config::test_support::minimal_sqlite_config(path)` (plus `bootstrap_admin` injection where needed).

The `migrate_end_to_end.rs` file is special — it has `mysql_config_from_url` and `postgres_config_from_url` helpers that produce non-SQLite configs. Keep those (they're not duplicates), but have them START from `minimal_sqlite_config(...)` and mutate `dbtype`, `dbhost`, etc. This removes about 30 lines of duplicated field listing.

- [ ] **Step 7: Run the full test suite**

```
cargo xtask check-all
```

Expected: green. All ~150 tests still pass; each test file is now smaller and less brittle.

- [ ] **Step 8: Commit**

```
git add crates/rustcloud-config crates/rustcloud-cache crates/rustcloud-db crates/rustcloud-core crates/rustcloud-http crates/rustcloud-ui Cargo.lock
git commit -m "refactor(config): consolidate test FileConfig fixture into test-support module

Phase 1-4 accumulated ~10 hand-rolled cfg_sqlite() helpers across crates;
adding a new FileConfig field meant touching all of them. Centralize in
rustcloud-config::test_support behind a feature flag and replace every
call site.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 2: Centralize `[workspace.lints]`

**Files:**
- Modify: `Cargo.toml` (root) — add `[workspace.lints]` table
- Modify: every per-crate `Cargo.toml` — add `[lints] workspace = true`

Establish workspace-wide lint policy. `cargo check` and `cargo clippy` will then warn (or error) per the workspace policy, not just `xtask check-all`. The first task to actually flag the unused deps in `rustcloud-core` (cleaned up in Task 3).

- [ ] **Step 1: Add `[workspace.lints]` to root `Cargo.toml`**

Append to `Cargo.toml`:

```toml
[workspace.lints.rust]
unused_crate_dependencies = "warn"

[workspace.lints.clippy]
all = "warn"
```

We keep `xtask check-all`'s `-D warnings` flag, so these warn-level lints still gate CI. We deliberately don't enable `pedantic` / `nursery` to avoid a massive cleanup pass; that's a later-program decision.

- [ ] **Step 2: Add `[lints] workspace = true` to every member crate**

For each of the 9 member crates and `xtask`, append to their `Cargo.toml`:

```toml
[lints]
workspace = true
```

Crates: `rustcloud-cache`, `rustcloud-config`, `rustcloud-core`, `rustcloud-db`, `rustcloud-http`, `rustcloud-i18n`, `rustcloud-ocs`, `rustcloud-server`, `rustcloud-ui`, `xtask`.

- [ ] **Step 3: Run `cargo clippy --workspace --all-targets`**

Expected: a list of unused-crate-dependencies warnings. These are addressed in Task 3 (rustcloud-core has the bulk).

The build won't fail at this step — `xtask check-all` is the one that escalates warnings to errors. Don't run `xtask check-all` yet; expect it to fail until Task 3 clears the deps.

- [ ] **Step 4: Commit**

```
git add Cargo.toml crates/*/Cargo.toml xtask/Cargo.toml
git commit -m "chore: centralize lint policy via [workspace.lints]

unused_crate_dependencies = warn so future dep drift surfaces during
plain cargo check, not just under xtask check-all's -D warnings.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 3: Clean up unused deps in `rustcloud-core`

**Files:**
- Modify: `crates/rustcloud-core/Cargo.toml`
- Modify: `crates/rustcloud-core/src/appconfig.rs` (will use `tracing` now — see Task 4; for this task, just drop the truly unused ones)

The Phase 2 final review flagged: `async-trait`, `tracing`, `serde`, `serde_json` declared but unreferenced. Task 4 will start using `tracing` (`tracing::warn!` in cache failure paths). The other three remain unused after Task 4 — drop them.

- [ ] **Step 1: Verify which deps are actually used**

Run `cargo clippy -p rustcloud-core --all-targets` after Task 2 lands. Note the list of `unused_crate_dependencies` warnings.

Expected hits (per Phase 2 review): `async-trait`, `serde`, `serde_json`. (`tracing` will move from "unused" to "used" via Task 4, but for now it's also unused.)

- [ ] **Step 2: Drop unused deps from `crates/rustcloud-core/Cargo.toml`**

Find the `[dependencies]` block and remove these three lines:

```toml
async-trait.workspace = true
serde.workspace = true
serde_json.workspace = true
```

Leave `tracing.workspace = true` in place — Task 4 will start using it.

- [ ] **Step 3: Build to confirm**

```
cargo build -p rustcloud-core
cargo test -p rustcloud-core --features test-support
```

Expected: clean build. `tracing` may still be flagged as unused (until Task 4); accept that.

- [ ] **Step 4: Sweep other crates for any unused deps flagged**

Run `cargo clippy --workspace --all-targets` and list all `unused_crate_dependencies` warnings. For each flagged dep that isn't actually used elsewhere in the same crate, remove it. Likely candidates (verify):

- `rustcloud-ocs/Cargo.toml`: `quick-xml` was already dropped in Batch C, verify.
- `rustcloud-ui/Cargo.toml`: `tracing` — verify it's used (the SSR handler may not have a tracing span). If unused, drop.

If a crate genuinely uses a dep that the lint mis-flags, add an `#[allow(unused_crate_dependencies)]` to the crate root with a comment explaining why.

- [ ] **Step 5: Run `cargo xtask check-all`**

Expected: green, no `unused_crate_dependencies` warnings remaining.

- [ ] **Step 6: Commit**

```
git add crates/*/Cargo.toml
git commit -m "chore(core,ui): drop unused workspace deps flagged by unused_crate_dependencies

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 4: `AppConfigService` — `tracing::warn!` on cache failures + lift `CACHE_TTL`

**Files:**
- Modify: `crates/rustcloud-core/src/appconfig.rs`

Phase 2 review flagged two related items: cache `set` / `del` errors are silently swallowed, and the 60-second TTL is a magic literal. Both are tiny fixes; doing them together turns the unused `tracing` dep into a used one.

- [ ] **Step 1: Lift `CACHE_TTL` to a module constant**

Modify `crates/rustcloud-core/src/appconfig.rs`. Just below the imports, add:

```rust
/// Per-key TTL for the `oc_appconfig` write-through cache. Short enough that
/// admin-UI changes propagate within a minute; long enough that hot reads are
/// amortized.
const APPCONFIG_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(60);
```

Find every literal `Duration::from_secs(60)` in this file and replace with `APPCONFIG_CACHE_TTL`.

- [ ] **Step 2: Instrument cache failure paths**

Find the silent-failure swallows. The two call sites are typically:

```rust
let _ = self.cache.set(&ck, sentinel, Some(Duration::from_secs(60))).await;
```

and inside `set()`:

```rust
let _ = self.cache.del(&self.cache_key(appid, key)).await;
```

Replace each with:

```rust
if let Err(e) = self.cache.set(&ck, sentinel, Some(APPCONFIG_CACHE_TTL)).await {
    tracing::warn!(error = %e, appid, key, "failed to write appconfig cache");
}
```

and

```rust
if let Err(e) = self.cache.del(&self.cache_key(appid, key)).await {
    tracing::warn!(error = %e, appid, key, "failed to invalidate appconfig cache");
}
```

The exact variable names (`appid`, `key`) should match the surrounding scope. Adapt as needed.

- [ ] **Step 3: Run tests**

```
cargo test -p rustcloud-core --features test-support
```

Expected: 13 prior tests still pass. No new tests — `tracing::warn!` is observable in operator logs, not in unit-test assertions. (You can verify by hand with `RUST_LOG=warn cargo test` if curious, but not required.)

- [ ] **Step 4: Commit**

```
git add crates/rustcloud-core/src/appconfig.rs
git commit -m "chore(core): tracing::warn! on appconfig cache failures; lift CACHE_TTL

Silent cache failures hid degraded behavior in production. Now logs
the error, appid, and key so a flaky Redis surfaces in tracing.
Also lift the magic 60s TTL to APPCONFIG_CACHE_TTL.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 5: `version` subcommand expansion — git SHA + supported dialects

**Files:**
- Modify: `crates/rustcloud-server/Cargo.toml` (add `vergen-gix` as a build-dep)
- Create: `crates/rustcloud-server/build.rs`
- Modify: `crates/rustcloud-server/src/main.rs`

Spec §10.2 / §10.5 call for the `version` subcommand to print: Rustcloud version, git SHA, supported dialects, reported-as-Nextcloud version. Phase 1 stub prints only "rustcloud-server 0.1.0 (build target subproject: platform-core)". Expand it.

Use `vergen-gix` (a maintained fork of `vergen` using `gix` for git introspection — no shell-out, no `git` binary required) to capture the git SHA at build time. If `vergen-gix` is unavailable on crates.io, fall back to a small `build.rs` that runs `git rev-parse HEAD` via `std::process::Command`.

- [ ] **Step 1: Add `vergen-gix` as a build dependency**

Modify `crates/rustcloud-server/Cargo.toml`. Add a `[build-dependencies]` block (or append if it exists):

```toml
[build-dependencies]
vergen-gix = { version = "1", features = ["build"] }
```

Also add a workspace-level entry to root `Cargo.toml` for hygiene, even though only one crate uses it. Append under `[workspace.dependencies]`:

```toml
vergen-gix = { version = "1", features = ["build"] }
```

Then in the crate Cargo.toml change to:

```toml
[build-dependencies]
vergen-gix.workspace = true
```

- [ ] **Step 2: Write `crates/rustcloud-server/build.rs`**

```rust
//! Build script for `rustcloud-server`. Captures git SHA + build timestamp
//! via `vergen-gix` and emits them as `cargo:rustc-env=...` lines so
//! `env!()` in main.rs can read them.

use std::error::Error;
use vergen_gix::{BuildBuilder, Emitter, GixBuilder};

fn main() -> Result<(), Box<dyn Error>> {
    let build = BuildBuilder::all_build()?;
    let gix = GixBuilder::all_git()?;
    Emitter::default()
        .add_instructions(&build)?
        .add_instructions(&gix)?
        .emit()?;
    Ok(())
}
```

If `vergen-gix`'s API differs from this in your installed version (1.x is stable but minor revisions tweak builder names), the equivalent calls produce env vars like `VERGEN_GIT_SHA` and `VERGEN_BUILD_TIMESTAMP`. Adjust to match the installed crate.

If `vergen-gix` is unavailable at all, replace `build.rs` with this `Command`-based fallback:

```rust
use std::process::Command;

fn main() {
    let sha = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .and_then(|o| if o.status.success() { String::from_utf8(o.stdout).ok() } else { None })
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=RUSTCLOUD_GIT_SHA={}", sha);
    println!("cargo:rerun-if-changed=.git/HEAD");
}
```

Then read `env!("RUSTCLOUD_GIT_SHA")` in main.rs instead of `env!("VERGEN_GIT_SHA")`. Use whichever path actually builds — report the choice in your status.

- [ ] **Step 3: Expand the `Version` subcommand handler in `main.rs`**

Modify `crates/rustcloud-server/src/main.rs`. Find the `Cmd::Version =>` arm and replace:

```rust
        Cmd::Version => {
            println!(
                "rustcloud-server {pkg_ver}\n\
                 git:       {git_sha}\n\
                 dialects:  sqlite, mysql, postgres\n\
                 subproject: platform-core",
                pkg_ver = env!("CARGO_PKG_VERSION"),
                git_sha = option_env!("VERGEN_GIT_SHA")
                    .or_else(|| option_env!("RUSTCLOUD_GIT_SHA"))
                    .unwrap_or("unknown"),
            );
            Ok(())
        }
```

The `option_env!` chain accommodates both build.rs paths (vergen-gix emits `VERGEN_GIT_SHA`; the fallback emits `RUSTCLOUD_GIT_SHA`).

- [ ] **Step 4: Verify**

```
cargo build -p rustcloud-server
cargo run -p rustcloud-server -- version
```

Expected output (SHA will be the actual HEAD commit):

```
rustcloud-server 0.1.0
git:       <40-char hex>
dialects:  sqlite, mysql, postgres
subproject: platform-core
```

- [ ] **Step 5: Run the workspace test suite**

```
cargo xtask check-all
```

Expected: green. The existing `cli.rs` tests don't assert on stdout content, so the version-print expansion doesn't break them.

- [ ] **Step 6: Commit**

```
git add Cargo.toml Cargo.lock crates/rustcloud-server
git commit -m "feat(server): version subcommand prints git SHA + supported dialects

Spec §10.2 / §10.5 call for git SHA + dialect coverage in the version
output so operators can tell what binary they're running. Captured at
build time via vergen-gix.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 6: Content-type-aware CSP

**Files:**
- Modify: `crates/rustcloud-http/src/middleware/security_headers.rs`

The current `SecurityHeadersLayer` ships one CSP for everything: `default-src 'none'; frame-ancestors 'self'; base-uri 'self'`. That's correct for JSON/XML responses but blocks the WASM bundle on HTML responses, defeating spec §13 #6 hydration.

Make the layer check the response `Content-Type` and pick the policy:

- HTML (`text/html`): `default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'self'; base-uri 'self'`
- Anything else (JSON, XML, plain text): the existing restrictive policy.

- [ ] **Step 1: Update the constants and call site**

Modify `crates/rustcloud-http/src/middleware/security_headers.rs`. Replace the CSP constant and the `call` method's header-set block:

Find:

```rust
const CSP: (&str, &str) = (
    "content-security-policy",
    "default-src 'none'; frame-ancestors 'self'; base-uri 'self'",
);
```

Replace with:

```rust
const CSP_API: &str = "default-src 'none'; frame-ancestors 'self'; base-uri 'self'";
const CSP_UI: &str = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'self'; base-uri 'self'";

fn csp_for_content_type(ct: Option<&axum::http::HeaderValue>) -> &'static str {
    match ct.and_then(|v| v.to_str().ok()) {
        Some(s) if s.starts_with("text/html") => CSP_UI,
        _ => CSP_API,
    }
}
```

Then update the `call` body. Find the block that inserts headers (the `for (name, value) in &[HSTS, XCTO, REFERRER, XFO, CSP] { ... }` loop) and replace with:

```rust
            let mut resp = inner.call(req).await?;
            let headers = resp.headers_mut();
            for (name, value) in &[HSTS, XCTO, REFERRER, XFO] {
                headers.insert(
                    HeaderName::from_static(name),
                    HeaderValue::from_static(value),
                );
            }
            let csp = csp_for_content_type(headers.get(axum::http::header::CONTENT_TYPE));
            headers.insert(
                HeaderName::from_static("content-security-policy"),
                HeaderValue::from_static(csp),
            );
            Ok(resp)
```

- [ ] **Step 2: Update the existing test**

The current test (`all_baseline_security_headers_present`) asserts CSP is set. Strengthen it: confirm the API path gets `default-src 'none'`. Add a second test asserting an HTML response gets the UI CSP.

Replace the test module at the bottom of `security_headers.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, HeaderValue, Request, Response, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    async fn plain_response() -> &'static str {
        "ok"
    }

    async fn html_response() -> Response<Body> {
        let mut resp = Response::new(Body::from("<html><body>hi</body></html>"));
        resp.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        *resp.status_mut() = StatusCode::OK;
        resp
    }

    #[tokio::test]
    async fn all_baseline_security_headers_present() {
        let app = Router::new().route("/", get(plain_response)).layer(SecurityHeadersLayer::new());
        let req = Request::builder().uri("/").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let h = resp.headers();
        assert!(h.get("strict-transport-security").is_some());
        assert!(h.get("x-content-type-options").is_some());
        assert!(h.get("referrer-policy").is_some());
        assert!(h.get("x-frame-options").is_some());
        assert!(h.get("content-security-policy").is_some());
        assert_eq!(h.get("x-content-type-options").unwrap(), "nosniff");
    }

    #[tokio::test]
    async fn non_html_response_gets_restrictive_csp() {
        let app = Router::new().route("/", get(plain_response)).layer(SecurityHeadersLayer::new());
        let req = Request::builder().uri("/").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let csp = resp.headers().get("content-security-policy").unwrap().to_str().unwrap();
        assert!(csp.starts_with("default-src 'none'"));
    }

    #[tokio::test]
    async fn html_response_gets_ui_csp_allowing_wasm() {
        let app = Router::new().route("/", get(html_response)).layer(SecurityHeadersLayer::new());
        let req = Request::builder().uri("/").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let csp = resp.headers().get("content-security-policy").unwrap().to_str().unwrap();
        assert!(csp.contains("'wasm-unsafe-eval'"));
        assert!(csp.contains("script-src 'self'"));
    }
}
```

- [ ] **Step 3: Run the tests**

```
cargo test -p rustcloud-http --lib middleware::security_headers
```

Expected: 3 tests pass (was 1; +2).

- [ ] **Step 4: Run the full check**

```
cargo xtask check-all
```

Expected: green workspace-wide.

- [ ] **Step 5: Commit**

```
git add crates/rustcloud-http/src/middleware/security_headers.rs
git commit -m "feat(http): content-type-aware CSP — HTML routes get wasm-unsafe-eval

The API-restrictive default-src 'none' blocked the WASM bundle on
text/html responses, preventing hydration. Pick the CSP at response
time from Content-Type so JSON/XML keep the strict policy and HTML
gets script-src 'self' 'wasm-unsafe-eval'.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 7: Dioxus router 404 status

**Files:**
- Modify: `crates/rustcloud-ui/src/ssr.rs`
- Modify: `crates/rustcloud-http/src/routes/ui.rs`

The current SSR handler returns 200 for every path, including unknown ones (which the Dioxus `NotFoundRoute` catch-all renders as a 404 page). Real clients and crawlers expect HTTP 404 for unknown URLs.

Approach: parse the request path through `Route::parse_path(...)` before rendering. If the parser yields `Route::NotFoundRoute { .. }`, set the response status to 404; otherwise 200.

- [ ] **Step 1: Add a route-parsing helper to `crates/rustcloud-ui/src/ssr.rs`**

Inside `ssr.rs`, add a `pub fn resolve_route(path: &str) -> Route` helper. Dioxus's `Routable` trait provides `Route::from_str(path)` via `FromStr`. The exact API may differ; the documented Dioxus 0.6 pattern is:

```rust
use std::str::FromStr;
let route = Route::from_str(path).unwrap_or(Route::NotFoundRoute { segments: vec![] });
```

Append to `ssr.rs`:

```rust
/// Parse a request path into a `Route`. Falls back to `NotFoundRoute` so the
/// caller doesn't have to unwrap.
pub fn resolve_route(path: &str) -> crate::Route {
    use std::str::FromStr;
    crate::Route::from_str(path)
        .unwrap_or_else(|_| crate::Route::NotFoundRoute { segments: vec![] })
}

/// True if the resolved route is the catch-all 404. Used by the HTTP handler
/// to set the response status.
pub fn is_not_found(route: &crate::Route) -> bool {
    matches!(route, crate::Route::NotFoundRoute { .. })
}

#[cfg(test)]
mod resolve_tests {
    use super::*;

    #[test]
    fn home_path_resolves_to_home_route() {
        let r = resolve_route("/");
        assert!(!is_not_found(&r));
    }

    #[test]
    fn login_path_resolves() {
        let r = resolve_route("/login");
        assert!(!is_not_found(&r));
    }

    #[test]
    fn unknown_path_resolves_to_not_found() {
        let r = resolve_route("/nonexistent/path");
        assert!(is_not_found(&r));
    }
}
```

If `Route::from_str` isn't the correct Dioxus 0.6 API, the alternative is `<Route as Routable>::parse_str(path)` or similar. Adapt minimally. The two helpers (`resolve_route`, `is_not_found`) are the public surface — the implementation can call whichever Dioxus method actually exists.

- [ ] **Step 2: Use the helper in the HTTP handler**

Modify `crates/rustcloud-http/src/routes/ui.rs`. Find the handler body — there's a section building the document via `format!("{doctype}<html lang=\"{lang}\">...")` and a `let mut resp = (StatusCode::OK, document).into_response();`. Modify to consult `is_not_found`:

```rust
    let path = uri.path();
    let resolved_route = rustcloud_ui::resolve_route(path);
    let status = if rustcloud_ui::is_not_found(&resolved_route) {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::OK
    };

    // ...existing code that builds ctx + document...

    let mut resp = (status, document).into_response();
```

You'll need to re-export the helpers from `rustcloud-ui::lib.rs`:

```rust
#[cfg(not(target_arch = "wasm32"))]
pub use ssr::{is_not_found, resolve_route, render_app_html, render_head_html, HTML_DOCTYPE};
```

(Add `is_not_found` and `resolve_route` to the existing re-export list.)

- [ ] **Step 3: Update the SSR integration test**

In `crates/rustcloud-ui/tests/ssr_routes.rs`, find the `unknown_path_returns_404_dioxus_page` test. It currently asserts `StatusCode::OK` with a comment that 404 status is deferred to Phase 5. Update to assert `StatusCode::NOT_FOUND`:

```rust
#[tokio::test]
async fn unknown_path_returns_404_dioxus_page() {
    let app = build_app().await;
    let req = Request::builder()
        .uri("/this/path/does/not/exist")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("404"));
    assert!(html.contains("Not Found"));
}
```

- [ ] **Step 4: Run tests**

```
cargo test -p rustcloud-ui
```

Expected: 11 lib + 5 integration + 3 resolve_tests = 19 tests pass. (The previous 5 integration count grows to 5; the new `resolve_tests` module adds 3.)

```
cargo xtask check-all
```

Expected: green.

- [ ] **Step 5: Commit**

```
git add crates/rustcloud-ui crates/rustcloud-http/src/routes/ui.rs
git commit -m "feat(ui,http): SSR handler returns 404 when Dioxus router resolves to NotFoundRoute

Unknown URLs now produce HTTP 404 with the rendered NotFound page,
not HTTP 200. Crawlers + monitoring tools expect this.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 8: Add hydration marker to the UI

**Files:**
- Modify: `crates/rustcloud-ui/src/app.rs`

Add a `data-hydrated` attribute that flips from `"false"` (SSR) to `"true"` (post-mount on the client). The Playwright test in Task 9 waits for this signal to confirm hydration ran.

- [ ] **Step 1: Modify `App` to render a hydration marker on a wrapping div**

Open `crates/rustcloud-ui/src/app.rs`. Modify the `App` component to wrap the `Router::<Route>` in a `div` with the hydration marker:

```rust
/// Root component. Renders the `Router<Route>` inside a hydration marker div.
/// The `data-hydrated` attribute flips from "false" (SSR) to "true" once the
/// WASM client mounts and runs the effect — Playwright E2E waits on this.
#[component]
pub fn App() -> Element {
    let mut hydrated = use_signal(|| false);
    use_effect(move || {
        hydrated.set(true);
    });
    let value = if hydrated() { "true" } else { "false" };
    rsx! {
        div { id: "app-root", "data-hydrated": "{value}",
            Router::<Route> {}
        }
    }
}
```

`use_effect` is a client-only hook in Dioxus 0.6 — it doesn't fire during SSR — so the SSR'd HTML has `data-hydrated="false"`. On WASM mount, the effect runs, the signal flips, the component re-renders, and the attribute becomes `data-hydrated="true"`.

If `use_effect` semantics differ from this in Dioxus 0.6.3 (e.g., it does fire during SSR), adapt by using a `use_resource` or `use_hook` that's explicitly browser-gated. The test in Task 9 will catch any regression.

- [ ] **Step 2: Update SSR integration tests to verify the initial state**

In `crates/rustcloud-ui/tests/ssr_routes.rs`, add to `home_returns_ssr_html_with_hydration_payload`:

```rust
    assert!(html.contains("data-hydrated=\"false\""), "missing hydration marker");
```

(Append after the existing assertions.)

- [ ] **Step 3: Run tests**

```
cargo test -p rustcloud-ui
```

Expected: all tests pass; the new assertion catches the SSR-side marker.

- [ ] **Step 4: Commit**

```
git add crates/rustcloud-ui/src/app.rs crates/rustcloud-ui/tests/ssr_routes.rs
git commit -m "feat(ui): emit data-hydrated marker for Playwright E2E hydration check

SSR renders data-hydrated=\"false\"; the WASM client's use_effect
runs on mount and flips it to \"true\". The Phase 5 Playwright test
waits on this transition to confirm hydration completed.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 9: Playwright E2E test

**Files:**
- Create: `e2e/.gitignore`
- Create: `e2e/package.json`
- Create: `e2e/playwright.config.ts`
- Create: `e2e/tests/hydration.spec.ts`
- Create: `e2e/README.md`
- Modify: `.gitignore` (root) — confirm `node_modules` covered

This is the most-substantial Phase 5 task. It adds a Node project at `e2e/` that drives a real Chromium browser against a running rustcloud-server, verifying that:
1. The SSR'd page contains the expected content.
2. The WASM bundle loads and hydrates (data-hydrated transitions to "true").
3. After login (POST `/index.php/login`), the home page shows the authenticated greeting.

- [ ] **Step 1: Write `e2e/package.json`**

```json
{
  "name": "rustcloud-e2e",
  "private": true,
  "version": "0.0.0",
  "scripts": {
    "test": "playwright test",
    "test:headed": "playwright test --headed"
  },
  "devDependencies": {
    "@playwright/test": "1.49.0",
    "typescript": "5.6.3"
  }
}
```

- [ ] **Step 2: Write `e2e/.gitignore`**

```
node_modules/
test-results/
playwright-report/
playwright/.cache/
```

- [ ] **Step 3: Write `e2e/playwright.config.ts`**

```ts
import { defineConfig, devices } from "@playwright/test";

const BASE_URL = process.env.RUSTCLOUD_E2E_URL ?? "http://127.0.0.1:18765";

export default defineConfig({
    testDir: "./tests",
    timeout: 30_000,
    expect: { timeout: 5_000 },
    fullyParallel: false,
    forbidOnly: !!process.env.CI,
    retries: process.env.CI ? 1 : 0,
    workers: 1,
    reporter: process.env.CI ? "github" : "list",
    use: {
        baseURL: BASE_URL,
        trace: "retain-on-failure",
        screenshot: "only-on-failure",
    },
    projects: [
        {
            name: "chromium",
            use: { ...devices["Desktop Chrome"] },
        },
    ],
});
```

The test expects the server to already be running at `RUSTCLOUD_E2E_URL` (default `http://127.0.0.1:18765`). The CI workflow (Task 10) starts the server before invoking `npm test` and stops it after.

- [ ] **Step 4: Write `e2e/tests/hydration.spec.ts`**

```ts
import { test, expect } from "@playwright/test";

test.describe("Rustcloud SSR + hydration", () => {
    test("home page SSRs with hydration marker and hydrates", async ({ page }) => {
        // Capture the response before JS executes to verify the SSR snapshot.
        const response = await page.goto("/");
        expect(response).not.toBeNull();
        expect(response!.status()).toBe(200);

        const htmlBeforeJs = await response!.text();
        expect(htmlBeforeJs).toContain("Welcome, guest");
        expect(htmlBeforeJs).toContain("data-hydrated=\"false\"");
        expect(htmlBeforeJs).toContain("<script id=\"__dx_ctx\"");

        // After the WASM bundle loads + use_effect runs, the marker flips.
        await expect(page.locator("#app-root")).toHaveAttribute(
            "data-hydrated",
            "true",
            { timeout: 10_000 },
        );
    });

    test("login flow then home shows authenticated greeting", async ({ page, request }) => {
        // POST to /index.php/login directly (the form's action). Use the
        // request context so we can capture and replay the cookie.
        const loginResp = await request.post("/index.php/login", {
            form: { username: "admin", password: "hunter2" },
            maxRedirects: 0,
        });
        expect(loginResp.status()).toBe(303);

        const cookie = loginResp.headers()["set-cookie"];
        expect(cookie).toContain("oc_sessionPassphrase=");

        // Visit `/` with the new session cookie.
        const sessionValue = /oc_sessionPassphrase=([^;]+)/.exec(cookie!)![1];
        await page.context().addCookies([{
            name: "oc_sessionPassphrase",
            value: sessionValue,
            url: new URL("/", page.context()._options.baseURL!).toString(),
        }]);

        const homeResp = await page.goto("/");
        expect(homeResp!.status()).toBe(200);
        await expect(page.locator("body")).toContainText("Welcome, admin");

        // And hydration still happens.
        await expect(page.locator("#app-root")).toHaveAttribute(
            "data-hydrated",
            "true",
            { timeout: 10_000 },
        );
    });

    test("404 path returns 404 status", async ({ page }) => {
        const response = await page.goto("/this/does/not/exist");
        expect(response!.status()).toBe(404);
        await expect(page.locator("body")).toContainText("Not Found");
    });
});
```

The `page.context()._options.baseURL` access is internal-API; if Playwright 1.49 doesn't expose it, fall back to hard-coding `http://127.0.0.1:18765` or read `process.env.RUSTCLOUD_E2E_URL` again.

- [ ] **Step 5: Write `e2e/README.md`**

```markdown
# Rustcloud E2E (Playwright)

End-to-end tests for the Rustcloud HTTP surface, including real WASM hydration
in a headless Chromium.

## Prerequisites

- Node 20+
- `npm ci` (or `pnpm install`)
- `npx playwright install --with-deps chromium` (one-time)

## Running locally

In one terminal, start the server with a test config that includes
`bootstrap_admin`:

```bash
# Generate a bcrypt hash for "hunter2"
python3 -c "import bcrypt; print(bcrypt.hashpw(b'hunter2', bcrypt.gensalt(12)).decode())"

# Edit config/config.toml.example into a fixture with bind_address = "127.0.0.1:18765"
# and a [bootstrap_admin] section using the hash above.

cargo xtask build
cargo run --release -p rustcloud-server -- --config fixture.toml migrate
cargo run --release -p rustcloud-server -- --config fixture.toml serve
```

In another terminal:

```bash
cd e2e
npm test
```

## CI

The `e2e` job in `.github/workflows/ci.yml` automates the above: builds the
release binary, starts it on `127.0.0.1:18765` with a fixture config, runs
the Playwright tests, then tears down the server.
```

- [ ] **Step 6: Verify the e2e project is well-formed**

You can't actually run Playwright locally without Node + `npm install`. Verify the project structure is correct by:

```
cd e2e
ls
```

Expected files: `package.json`, `playwright.config.ts`, `tests/hydration.spec.ts`, `README.md`, `.gitignore`.

If Node is available locally, run `npm install` to populate `node_modules` and confirm `package.json` parses. Don't commit `node_modules`.

- [ ] **Step 7: Commit**

```
git add e2e
git commit -m "test(e2e): add Playwright project for SSR + hydration + login flow

Three tests against a real Chromium: SSR snapshot + data-hydrated
transition, login flow with cookie replay + authenticated greeting,
and 404 status from the Dioxus router.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 10: CI E2E job

**Files:**
- Modify: `.github/workflows/ci.yml`

Add a new CI job that:
1. Needs `build-wasm` (downloads the WASM bundle artifact).
2. Sets up Node 20.
3. Installs the e2e project deps + Chromium.
4. Builds the rustcloud-server release binary.
5. Writes a fixture config including a bcrypt hash for `bootstrap_admin`.
6. Starts the server in the background and waits for `/status.php` to respond.
7. Runs `npm test` in `e2e/`.
8. Tears down the server.

- [ ] **Step 1: Append the `e2e` job to `.github/workflows/ci.yml`**

After the existing `test-multidialect` job, append:

```yaml
  e2e:
    runs-on: ubuntu-latest
    needs: build-wasm
    timeout-minutes: 25
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: wasm32-unknown-unknown
      - uses: Swatinem/rust-cache@v2
      - name: Download WASM bundle
        uses: actions/download-artifact@v4
        with:
          name: dx-public
          path: target/dx/rustcloud-ui/release/web/public
      - name: Build release server
        run: cargo build --release -p rustcloud-server
      - uses: actions/setup-node@v4
        with:
          node-version: "20"
          cache: "npm"
          cache-dependency-path: e2e/package-lock.json
      - name: Install Playwright deps
        working-directory: e2e
        run: |
          npm ci
          npx playwright install --with-deps chromium
      - name: Generate bcrypt hash for fixture
        id: bcrypt
        run: |
          python3 -m pip install bcrypt
          HASH=$(python3 -c "import bcrypt; print(bcrypt.hashpw(b'hunter2', bcrypt.gensalt(12)).decode())")
          echo "hash=$HASH" >> "$GITHUB_OUTPUT"
      - name: Write fixture config
        run: |
          mkdir -p config
          cat > config/e2e.toml <<EOF
          instanceid     = "e2e"
          secret         = "a-32-byte-or-longer-secret-key!"
          passwordsalt   = "ps"
          installed      = true
          version        = "31.0.0.0"
          versionstring  = "31.0.0"
          dbtype         = "sqlite"
          dbname         = "$GITHUB_WORKSPACE/e2e.db"
          dbtableprefix  = "oc_"
          datadirectory  = "$GITHUB_WORKSPACE/data"
          trusted_domains = ["localhost", "127.0.0.1"]
          loglevel       = "info"
          bind_address   = "127.0.0.1:18765"
          db_pool_max    = 4

          [cache]
          backend = "memory"

          [bootstrap_admin]
          username      = "admin"
          password_hash = "${{ steps.bcrypt.outputs.hash }}"
          EOF
      - name: Migrate
        run: cargo run --release -p rustcloud-server -- --config config/e2e.toml migrate
      - name: Start server
        run: |
          cargo run --release -p rustcloud-server -- --config config/e2e.toml serve &
          echo "SERVER_PID=$!" >> "$GITHUB_ENV"
          # Wait for the server to become ready.
          for i in $(seq 1 30); do
            if curl -sf http://127.0.0.1:18765/status.php >/dev/null; then
              echo "Server ready"
              exit 0
            fi
            sleep 1
          done
          echo "Server did not become ready"
          exit 1
      - name: Run Playwright tests
        working-directory: e2e
        env:
          RUSTCLOUD_E2E_URL: http://127.0.0.1:18765
        run: npm test
      - name: Stop server
        if: always()
        run: |
          if [ -n "$SERVER_PID" ]; then
            kill "$SERVER_PID" 2>/dev/null || true
          fi
      - name: Upload Playwright report on failure
        if: failure()
        uses: actions/upload-artifact@v4
        with:
          name: playwright-report
          path: e2e/playwright-report
          retention-days: 7
```

- [ ] **Step 2: Note about `package-lock.json`**

The first CI run will fail at the `npm ci` step because `e2e/package-lock.json` doesn't exist yet. Run `npm install` in `e2e/` locally to generate it, then commit:

```
cd e2e
npm install
cd ..
git add e2e/package-lock.json
```

If Node isn't available locally, **temporarily** change `npm ci` to `npm install` in the workflow YAML, push, let CI generate it on the first run, then copy it down from the action's cache or use a follow-up commit. The clean approach is to generate it locally.

- [ ] **Step 3: Commit**

```
git add .github/workflows/ci.yml e2e/package-lock.json
git commit -m "ci: run Playwright E2E against a real release server

New e2e job builds the release binary, writes a SQLite-backed fixture
config with a bcrypt admin hash, starts the server on :18765, waits
for /status.php, then runs Playwright. Server is torn down on cleanup;
report uploaded on failure.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 11: Rustdoc rollout

**Files:**
- Modify: many `crates/*/src/*.rs` files (public types and functions only)

The carry-over follow-up from every previous phase: public type-level APIs lack rustdoc. Phase 5 closes that gap. Aim for one-line summaries on every `pub` item; full doc with example for the load-bearing ones (`AppState`, `AppStateBuilder`, `Cache`, `MigrationRunner`, etc. — which mostly already have docs).

This task is mechanical and tedious. It's a single commit to keep history clean.

- [ ] **Step 1: Identify undocumented public items**

Run:

```
RUSTDOCFLAGS="-W missing_docs" cargo doc --workspace --no-deps 2>&1 | grep "missing documentation"
```

The list is the work surface. Expect entries in:

- `rustcloud-cache`: `Cache` trait methods may have docs; `MemoryCache::new` does; check `TypedCache` methods.
- `rustcloud-config`: `BootstrapAdminConfig` fields, `CacheConfig` fields.
- `rustcloud-core`: `AppState` fields, `Error` variants, `AppConfigService` methods.
- `rustcloud-db`: `DbPool` variants, `DbError` variants, `MigrationRunner` methods.
- `rustcloud-i18n`: `Locale::new`, `I18n` methods.
- `rustcloud-ocs`: `OcsStatus` variants, `Format::content_type`, `CapabilityProvider` trait methods.
- `rustcloud-http`: `AuthenticatedUser` fields, `OptionalUser`, `SessionLayer::new`, `CsrfLayer::new`.
- `rustcloud-ui`: `Route` variants, `RequestContext` fields, `App` component.

For each missing item, add a one-line `///` doc above it. Examples:

```rust
/// Username of the bootstrap administrator. Compared verbatim against form input.
pub username: String,
/// bcrypt hash of the password. Generate with `bcrypt::hash(password, 12)`.
pub password_hash: String,
```

For module-level docs (`//!`) where missing, prefer pointing at the spec section that defines the module's role.

- [ ] **Step 2: Verify documentation builds cleanly**

```
cargo doc --workspace --no-deps
```

Expected: clean, no warnings.

- [ ] **Step 3: (Optional, but recommended) tighten the workspace lints**

After the rollout, you can promote rustdoc-missing from "warn" to "warn" workspace-wide by adding to root `Cargo.toml`:

```toml
[workspace.lints.rust]
unused_crate_dependencies = "warn"
missing_docs = "warn"
```

Verify `cargo xtask check-all` still passes. If too many missing-docs warnings remain (some library-internal items), keep `missing_docs` off for now and revisit per-program decision.

- [ ] **Step 4: Run the full check**

```
cargo xtask check-all
```

Expected: green.

- [ ] **Step 5: Commit**

```
git add crates
git commit -m "docs: roll out rustdoc on public type-level APIs across the workspace

Closes the carry-over follow-up from Phases 1-4. Every public type,
field, variant, and method now has a one-line summary; load-bearing
items (AppState, MigrationRunner, etc.) keep their existing fuller
docs.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 12: `CONTRIBUTING.md`

**Files:**
- Create: `CONTRIBUTING.md`

Document the development workflow, MSRV, the Cargo.lock pinning rationale, and the path to running the full test suite (including E2E).

- [ ] **Step 1: Write `CONTRIBUTING.md`**

```markdown
# Contributing to Rustcloud

Thanks for your interest! Rustcloud is an early-stage Rust port of
[Nextcloud server](https://github.com/nextcloud/server) with a Dioxus frontend.
The platform-core substrate (HTTP, DB, sessions, OCS envelope, SSR UI) is
nearing completion; per-feature sub-projects (users, storage, WebDAV, sharing,
etc.) will follow.

## Tooling

| Component | Required version | Notes |
|---|---|---|
| Rust toolchain | **1.85.0** (pinned in `rust-toolchain.toml`) | rustup auto-installs on first cargo invocation |
| WASM target | `wasm32-unknown-unknown` | `rustup target add wasm32-unknown-unknown` |
| Dioxus CLI | `dioxus-cli ^0.6` | `cargo install dioxus-cli --version "^0.6" --locked` |
| Node | 20+ | only for the Playwright E2E suite under `e2e/` |
| Docker (optional) | recent | for multi-dialect DB tests via `cargo xtask up` |

## MSRV and `Cargo.lock`

The MSRV is **Rust 1.85**. `Cargo.lock` is committed and pins specific
transitive versions of `serde_with`, `home`, `url`, and `idna` to keep
the dep tree buildable on 1.85. If `cargo update` pulls a crate that
requires a newer rustc, either pin via `cargo update -p <crate> --precise <ver>`
or update the toolchain. Don't drift silently.

## Workflow

```bash
# Format + lint + tests (SQLite only, in-process)
cargo xtask check-all

# Start MySQL + Postgres for the multi-dialect db tests
cargo xtask up
cargo test -p rustcloud-db --test migrate_end_to_end -- --include-ignored
cargo xtask down

# Build the WASM bundle + release server
cargo xtask build

# Run the server
cargo run --release -p rustcloud-server -- --config config/config.toml serve

# Playwright E2E (requires server already running on :18765)
cd e2e
npm ci
npx playwright install --with-deps chromium
npm test
```

## CI

Every push runs (in `.github/workflows/ci.yml`):

1. `fmt-and-clippy` — `cargo fmt --check` + `cargo clippy --workspace --all-targets -- -D warnings`.
2. `build-wasm` — installs `dioxus-cli`, runs `dx build --release`, uploads bundle artifact.
3. `test-sqlite` — downloads bundle, runs `cargo test --workspace`.
4. `test-multidialect` — spins up MySQL + Postgres service containers, runs `cargo test --test migrate_end_to_end -- --include-ignored`.
5. `e2e` — builds release server, starts on `:18765`, runs Playwright against a real Chromium.

## Commit conventions

[Conventional Commits](https://www.conventionalcommits.org/): `feat`, `fix`,
`chore`, `docs`, `test`, `refactor`. Each commit body should explain the *why*
in plain prose. Include the `Co-Authored-By` trailer when AI tooling
contributed to the change.

## Filing issues / PRs

This is an early-stage repo. File issues at
<https://github.com/robot-head/rustcloud/issues>. Before opening a PR, run
`cargo xtask check-all` locally; expect CI to also exercise the E2E job.
```

- [ ] **Step 2: Commit**

```
git add CONTRIBUTING.md
git commit -m "docs: add CONTRIBUTING.md with MSRV, tooling, and workflow notes

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 13: Phase 5 acceptance + changelog + spec §13 update

**Files:**
- Modify: `README.md`
- Create: `docs/superpowers/plans/2026-05-10-platform-core-phase-5-ship.changelog.md`

Final pass: update the README to declare platform-core complete, write the Phase 5 changelog, and reconcile spec §13 acceptance markers.

- [ ] **Step 1: Update `README.md`**

Replace the existing README with the Phase 5 version. The only change from the Phase 4 README is the status line and a small reference to `CONTRIBUTING.md`:

```markdown
# Rustcloud

A Rust port of [Nextcloud server](https://github.com/nextcloud/server), with a Dioxus frontend.

**Status:** platform-core complete. The server boots, serves the Nextcloud-compatible API surface (`/status.php`, `/ocs/v2.php/cloud/capabilities`, `/index.php/login`), and renders an SSR'd Dioxus UI that hydrates in the browser. Spec §13 acceptance criteria are all green (verified by `cargo xtask check-all` + the Playwright E2E suite). Per-feature sub-projects (users, storage, WebDAV, sharing, calendar/contacts, etc.) build on this substrate.

See `docs/superpowers/specs/` for design specs, `docs/superpowers/plans/` for implementation plans, and `CONTRIBUTING.md` for dev workflow.

## Quick start

```bash
# 0. One-time tooling
rustup target add wasm32-unknown-unknown
cargo install dioxus-cli --version "^0.6"

# 1. Copy and edit the example config.
cp config/config.toml.example config/config.toml
#  - Set installed = true.
#  - Pick a dbtype (sqlite is easiest).
#  - For login, add a [bootstrap_admin] section with a bcrypt password hash.

# 2. Build UI + server.
cargo xtask build

# 3. Run migrations + serve.
cargo run --release -p rustcloud-server -- migrate
cargo run --release -p rustcloud-server -- serve

# 4. Visit http://127.0.0.1:8080/ in a browser.
```

## Workspace layout

- `crates/rustcloud-config` — layered TOML config loader.
- `crates/rustcloud-cache` — `Cache` trait + `MemoryCache` + `TypedCache<T>`.
- `crates/rustcloud-db` — `DbPool` enum, `MigrationRunner`, core schema.
- `crates/rustcloud-i18n` — gettext `.po` loader, `Locale`, `I18n`.
- `crates/rustcloud-ocs` — OCS envelope (JSON/XML), capabilities aggregator.
- `crates/rustcloud-core` — `AppState`, `Error`, `AppConfigService`, `BootstrapHook`.
- `crates/rustcloud-http` — axum router, middleware, session, CSRF, auth extractors, API handlers.
- `crates/rustcloud-ui` — Dioxus 0.6 SSR + WASM hydration UI.
- `crates/rustcloud-server` — the binary; CLI, tracing, lifecycle.
- `xtask/` — project automation.
- `e2e/` — Playwright tests (real-browser SSR + hydration verification).
- `migrations/core/` — core SQL migrations, per-dialect.
- `l10n/<app>/<locale>.po` — translation catalogs.

## License

AGPL-3.0-or-later.
```

- [ ] **Step 2: Write the Phase 5 changelog**

Create `docs/superpowers/plans/2026-05-10-platform-core-phase-5-ship.changelog.md`. Use today's date for the `Completed:` line.

```markdown
# Phase 5 (Ship) — Changelog

Completed: <today's date in YYYY-MM-DD>

## What works

- **Test fixture consolidation**: `rustcloud-config::test_support::minimal_sqlite_config` (behind a `test-support` feature) replaces ~10 hand-rolled `cfg_sqlite` copies across the workspace. Adding a new `FileConfig` field now means changing one helper, not ten.
- **Centralized lint policy**: `[workspace.lints]` at the root + `[lints] workspace = true` in every member. `unused_crate_dependencies = "warn"` surfaces dep drift via plain `cargo check`.
- **`rustcloud-core` cleanup**: drops unused `async-trait` / `serde` / `serde_json`. `tracing` now carries its weight via `AppConfigService` instrumentation.
- **`AppConfigService` instrumentation**: cache `set` / `del` failures now emit `tracing::warn!` with structured fields (error, appid, key). `CACHE_TTL` lifted to a module constant.
- **`version` subcommand expansion**: prints crate version, git SHA (via `vergen-gix`), supported dialects, sub-project marker. Closes spec §10.2 / §10.5 acceptance.
- **Content-type-aware CSP**: `SecurityHeadersLayer` inspects the response `Content-Type` and ships the UI-permissive CSP (`script-src 'self' 'wasm-unsafe-eval'`) for HTML and the API-restrictive `default-src 'none'` for everything else. **Unblocks WASM hydration**.
- **Dioxus router 404 status**: SSR handler parses the request path through `Route::from_str` and returns HTTP 404 when the resolved variant is `NotFoundRoute`. The body is still the Dioxus-rendered 404 page; the status finally matches.
- **Hydration marker**: `App` component wraps its content in `<div id="app-root" data-hydrated="...">`. SSR emits `"false"`; `use_effect` on the WASM client flips it to `"true"`. The Playwright E2E waits on this transition.
- **Playwright E2E** (`e2e/`): three tests against a real Chromium — SSR snapshot + hydration, login-flow + authenticated greeting, 404 status. CI job builds the release server, writes a fixture config with a bcrypt admin hash, runs Playwright, tears down. **Verifies spec §13 #6 end-to-end.**
- **Rustdoc rollout**: every public type/field/variant/method across the workspace has a one-line summary; load-bearing types keep their existing fuller docs.
- **`CONTRIBUTING.md`**: MSRV (1.85), tooling versions (Dioxus 0.6, Node 20), workflow commands, CI layout, commit conventions.

## What's deferred (post-platform-core)

These are explicitly *not* in scope for the platform-core program; they belong to specific feature sub-projects:

- **Real user store**: Bearer auth, Basic auth, app passwords, OAuth2 server, LDAP, SAML, 2FA, constant-time username comparison. The `bootstrap_admin` stand-in handles Phase 4-5 demo needs.
- **Server functions** (`#[server]`): Phase 4 deliberately routed all auth-bearing operations through the OCS API surface for cross-client compatibility. Server functions land per-feature when a UI-only RPC actually needs them.
- **WebDAV / CalDAV / CardDAV**: own sub-projects.
- **File sharing**: own sub-project.
- **Background job runner**: own micro-sub-project.
- **Redis cache backend**: `Cache` trait is ready; implementation lands before multi-node deploy.
- **App / plugin framework lifecycle hooks** beyond `BootstrapHook`: settings UI registration, dependency resolution, navigation entries, etc.
- **Internationalization wiring into UI components**: Home/Login still render English inline.
- **Public share landing** (`/s/<token>`): stub in Route enum, not implemented.
- **Theming engine**.

## Known limitations

- `data-hydrated` marker depends on `use_effect` not running during SSR. Dioxus 0.6 semantics agree, but if a future Dioxus upgrade changes that the E2E test will break loud (good — that's the signal).
- CSP `'wasm-unsafe-eval'` allows the WASM bundle to instantiate but doesn't permit `eval()` or `Function(string)` from JS. Acceptable for Dioxus; revisit if a future asset needs a wider exception.
- Playwright E2E uses `kill $PID` for teardown — if the server hangs, the CI step may need a `timeout` wrapper. Not seen in practice.

## Spec §13 acceptance criteria — final status

| # | Criterion | Status | Verified by |
|---|---|---|---|
| 1 | `cargo xtask check-all` against all three backends | GREEN | CI workflow + multi-dialect job |
| 2 | `cargo xtask build` produces a static binary with embedded UI assets | GREEN | `xtask build` task + `rust-embed` |
| 3 | Binary boots + migrates + serves against fresh SQLite/MySQL/Postgres | GREEN | `migrate_end_to_end` integration tests + serve subcommand |
| 4 | `curl /status.php` returns Nextcloud JSON | GREEN | `http_end_to_end.rs` + `routes::status` tests |
| 5 | `curl /ocs/v2.php/cloud/capabilities` returns OCS envelope | GREEN | `routes::ocs::capabilities` tests + `http_end_to_end.rs` |
| 6 | Browser at `/` SSR'd + hydrated | **GREEN** | Playwright E2E `hydration.spec.ts` — verifies in real Chromium |
| 7 | `/login` POST sets session cookie + redirects | GREEN | `routes::login` tests + E2E login flow test |
| 8 | Middleware enforcement integration-tested | GREEN | per-middleware unit tests + `http_end_to_end.rs` |
| 9 | Single + multi-dialect tests green in CI | GREEN | CI workflow |

Platform-core is complete.
```

- [ ] **Step 3: Run final acceptance**

```
cargo clean
cargo xtask build
cargo xtask check-all
```

Expected: all green from a clean state.

(The E2E suite isn't run as part of `xtask check-all`; it requires the binary running on `:18765`. Manual verification: build, run, then `cd e2e && npm test`. CI's `e2e` job handles automation.)

- [ ] **Step 4: Commit**

```
git add README.md docs/superpowers/plans/2026-05-10-platform-core-phase-5-ship.changelog.md
git commit -m "docs: phase 5 acceptance — README, changelog, spec §13 final status

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Phase 5 Self-Review (executor verifies before declaring complete)

Run through each Phase 5 task and spec §13 criterion:

| Item | Verified by |
|---|---|
| Test fixture consolidated | `cargo test --workspace` still green; no remaining hand-rolled `cfg_sqlite` |
| `[workspace.lints]` table | `cargo clippy --workspace` runs the lint policy |
| `rustcloud-core` unused deps cleaned | `cargo clippy -p rustcloud-core` — no `unused_crate_dependencies` |
| `AppConfigService` tracing + CACHE_TTL | grep `APPCONFIG_CACHE_TTL` + `tracing::warn!` in appconfig.rs |
| `version` subcommand expansion | `cargo run -p rustcloud-server -- version` prints SHA + dialects |
| Content-type CSP | `routes::middleware::security_headers::tests` 3 tests pass |
| Dioxus router 404 | `tests/ssr_routes.rs::unknown_path_returns_404_dioxus_page` asserts 404 |
| Hydration marker | `home_returns_ssr_html_with_hydration_payload` asserts `data-hydrated="false"` in SSR |
| Playwright E2E | CI `e2e` job runs green (or fails loudly on PR) |
| Rustdoc rollout | `RUSTDOCFLAGS="-W missing_docs" cargo doc --workspace --no-deps` clean |
| CONTRIBUTING.md | file exists, links from README |
| Spec §13 acceptance | changelog table marks all 9 criteria GREEN |

If any of these fail, fix before declaring Phase 5 — and therefore platform-core — complete.

---
