# Contributing to Crabcloud

Thanks for your interest! Crabcloud is an early-stage Rust port of
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
cargo test -p crabcloud-db --test migrate_end_to_end -- --include-ignored
cargo xtask down

# Build the WASM bundle + release server
cargo xtask build

# Run the server
cargo run --release -p crabcloud-server -- --config config/config.toml serve

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
<https://github.com/robot-head/crabcloud/issues>. Before opening a PR, run
`cargo xtask check-all` locally; expect CI to also exercise the E2E job.
