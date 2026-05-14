# Crabcloud

A Rust port of [Nextcloud server](https://github.com/nextcloud/server), with a Dioxus frontend.

**Status:** platform-core complete. The server boots, serves the Nextcloud-compatible API surface (`/status.php`, `/ocs/v2.php/cloud/capabilities`, `/index.php/login`), and renders an SSR'd Dioxus UI that hydrates in the browser. Spec §13 acceptance criteria are all green (verified by `cargo xtask check-all` + the Playwright E2E suite). Per-feature sub-projects (users, storage, WebDAV, sharing, calendar/contacts, etc.) build on this substrate.

See `docs/superpowers/specs/` for design specs, `docs/superpowers/plans/` for implementation plans, and `CONTRIBUTING.md` for dev workflow.

## Quick start

```bash
# 0. One-time tooling
rustup target add wasm32-unknown-unknown
cargo install dioxus-cli --version "^0.7"

# 1. Copy and edit the example config.
cp config/config.toml.example config/config.toml
#  - Set installed = true.
#  - Pick a dbtype (sqlite is easiest).
#  - For login, add a [bootstrap_admin] section with a bcrypt password hash.

# 2. Build UI + server (one dx invocation produces both WASM client + server
#    binary with hashed asset paths substituted into SSR HTML). Replace
#    <host-triple> with x86_64-unknown-linux-gnu / x86_64-pc-windows-msvc /
#    aarch64-apple-darwin as appropriate.
( cd crates/crabcloud-app && dx build --release \
    @client --no-default-features --features web --target wasm32-unknown-unknown \
    @server --no-default-features --features server --target <host-triple> )

# 3. Run migrations, then serve via the dx-built binary.
./target/dx/crabcloud-app/release/web/server --config config/config.toml migrate
./target/dx/crabcloud-app/release/web/server --config config/config.toml serve

# 3a. Create your first admin user (interactive password prompt). The CLI
#     subcommands work via cargo too — no dx asset pass needed.
cargo run -p crabcloud-app -- user-add admin --admin

# 3b. (or, for the fresh-install bootstrap path)
#     Add [bootstrap_admin] to config.toml with a bcrypt hash;
#     log in, change your password — your account is now a real DB user.

# 3c. Pair a DAV / desktop / mobile client (Nextcloud-client-compatible):
#     - Visit https://<your-server>/settings/security in your browser.
#     - Click "Create app password", enter a device name (e.g. "Phone").
#     - Copy the displayed token (shown ONCE).
#     - Configure your client with username + that token as the password.
#     - Alternatively, point your client at https://<your-server>/index.php/login/v2
#       and follow Nextcloud's authorize-in-browser flow.

# 3d. Administer users + groups via the OCS API (Nextcloud-compatible):
#     - `POST /ocs/v2.php/cloud/users` with form `userid=<>&password=<>&email=<>&displayName=<>`
#     - `PUT /ocs/v2.php/cloud/users/<uid>/disable` to force-logout a user everywhere
#     - `GET /ocs/v2.php/cloud/users?search=<term>` to search by uid/displayname/email
#     - Authenticate via the admin's session cookie (after logging in) or admin app password.
#     - The Nextcloud Admin app speaks this API natively — point it at https://<server>.

# 4. Visit http://127.0.0.1:8080/ in a browser.
```

## Development

The Dioxus 0.7 fullstack model means a single `dx` invocation drives both the
WASM client and the native server binary. dx performs link-time substitution of
`manganis` asset placeholders into hashed `/assets/<…>.ext` paths in the SSR
HTML, so any code path that needs the rendered page to match the served
bundle (e.g. Playwright E2E) must go through a dx-built binary.

### Hot-reload dev server

From `crates/crabcloud-app/`:

```bash
dx serve --release \
  @client --no-default-features --features web --target wasm32-unknown-unknown \
  @server --no-default-features --features server
```

`dx serve` spawns the server binary with `IP` + `PORT` env vars set; the
binary's `Cmd::Serve` honors these (overriding `bind_address` in
`config.toml`) so HMR ws + asset reload reach the right address. The dev
URL is printed on startup (typically `http://localhost:8080`).

### Release build

From `crates/crabcloud-app/`:

```bash
dx build --release \
  @client --no-default-features --features web --target wasm32-unknown-unknown \
  @server --no-default-features --features server --target <host-triple>
```

Replace `<host-triple>` with `x86_64-unknown-linux-gnu` on Linux,
`x86_64-pc-windows-msvc` on Windows, or `aarch64-apple-darwin` on Apple
silicon. Output lives under `target/dx/crabcloud-app/release/web/`:

- `server` (Linux/macOS) or `server.exe` (Windows) — the server binary.
- `public/assets/` — hashed CSS / JS / image / WASM bundles.
- `public/index.html` — generated shell.

Run it via:

```bash
./target/dx/crabcloud-app/release/web/server --config config/config.toml serve
```

### Cargo-only fallback (CLI subcommands)

For CLI subcommands (`migrate`, `user-add`, `files scan`, etc.) and
scripted server runs that don't need the rendered UI, plain `cargo` works:

```bash
cargo run --release -p crabcloud-app -- migrate
cargo run --release -p crabcloud-app -- user-add admin --admin
```

The cargo-built server binary's SSR HTML will contain `manganis` placeholder
strings for stylesheet/script hrefs (a known artifact of building without
dx's linker pass) — production / E2E use the dx-built binary above.

## Workspace layout

- `crates/crabcloud-config` — layered TOML config loader.
- `crates/crabcloud-cache` — `Cache` trait + `MemoryCache` + `TypedCache<T>`.
- `crates/crabcloud-db` — `DbPool` enum, `MigrationRunner`, core schema.
- `crates/crabcloud-filecache` — DB-backed file cache + scanner consuming Storage events.
- `crates/crabcloud-fs` — per-user filesystem facade (View + Uploads + mount resolution).
- `crates/crabcloud-i18n` — gettext `.po` loader, `Locale`, `I18n`.
- `crates/crabcloud-ocs` — OCS envelope (JSON/XML), capabilities aggregator.
- `crates/crabcloud-core` — `AppState`, `Error`, `AppConfigService`, `BootstrapHook`.
- `crates/crabcloud-http` — axum router, middleware, session, CSRF, auth extractors, API handlers (including the Nextcloud-compatible admin OCS surface at `/ocs/v2.php/cloud/{users,groups}` and the WebDAV files API at `/dav/files/{user}/...` plus the legacy `/remote.php/dav/files/{user}/...` alias, with chunked uploads under `/dav/uploads/{user}/{upload_id}/...`).
- `crates/crabcloud-storage` — `Storage` async trait + `MemoryStorage` and `LocalStorage` backends, `StoragePath` newtype, `EventSink` for cache/scanner consumers.
- `crates/crabcloud-users` — user/group/preference stores, password verifier, `UsersService` façade, bootstrap-admin shim, `AppPasswordService` + `TokenStore` (auth tokens + Bearer/Basic).
- `crates/crabcloud-app` — Dioxus 0.7 Fullstack (SSR + WASM hydration + `#[server]` functions) AND the binary entrypoint (CLI, tracing, lifecycle). Includes the Files web app at `/apps/files/<path>`: browse, download (via the existing `/dav/files/...` WebDAV GET), mkdir/rename/delete, multi-select cut/paste move, upload (single-PUT + chunked via `/dav/uploads/...`). Metadata server fns live at `POST /api/files/{list,mkdir,rename,delete,move,upload_begin}`.
- `xtask/` — project automation.
- `e2e/` — Playwright tests (real-browser SSR + hydration verification).
- `migrations/core/` — core SQL migrations, per-dialect.
- `l10n/<app>/<locale>.po` — translation catalogs.

## License

AGPL-3.0-or-later.
