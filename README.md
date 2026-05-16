# Crabcloud

A self-hosted cloud platform written in Rust, with a [Dioxus](https://dioxuslabs.com/) frontend that renders on the server and hydrates in the browser. Crabcloud is API-compatible with [Nextcloud](https://nextcloud.com/), so existing Nextcloud desktop, mobile, and WebDAV clients pair against it without modification.

## What it is

- **One static binary.** A single executable serves the WASM client, the SSR'd HTML shell, the WebDAV endpoints, and the OCS API. No PHP runtime, no Apache, no separate frontend build pipeline.
- **Compatible by design.** `/status.php`, `/ocs/v2.php/cloud/*`, `/index.php/login/v2`, and `/remote.php/dav/...` behave like Nextcloud's so existing clients work unchanged. The Nextcloud Admin app speaks to the same OCS surface natively.
- **Pluggable storage.** Filesystem access is abstracted behind an async `Storage` trait, with backends selectable per-mount and an event sink that keeps the file cache and scanner consistent.
- **Cross-platform.** Builds and runs on Linux, macOS, and Windows; bundles to a single binary plus a `public/` asset directory.

## Features

Crabcloud aims for parity with the core Nextcloud Hub. The table below tracks which features are available today.

### Platform

| Feature                                        | Status |
| ---------------------------------------------- | :----: |
| Nextcloud-compatible OCS API (`/ocs/v2.php`)   |   ✅   |
| Capabilities endpoint                          |   ✅   |
| Server-side rendering + WASM hydration         |   ✅   |
| Layered TOML configuration                     |   ✅   |
| Database migrations (SQLite / PostgreSQL / MySQL) | ✅   |
| Localization (gettext `.po` catalogs)          |   ✅   |
| In-memory cache layer                          |   ✅   |
| CLI admin subcommands                          |   ✅   |
| Redis / Memcached cache backends               |   ❌   |
| Background job runner / cron                   |   ❌   |
| Theming & branding                             |   ❌   |
| Maintenance mode                               |   ❌   |
| Audit log                                      |   ❌   |
| Federation (server-to-server)                  |   ❌   |
| App store / third-party apps                   |   ❌   |

### Authentication & users

| Feature                                                | Status |
| ------------------------------------------------------ | :----: |
| Username + password login (bcrypt)                     |   ✅   |
| Session cookies + CSRF protection                      |   ✅   |
| App passwords (per-device tokens)                      |   ✅   |
| Bearer / Basic auth for API and DAV clients            |   ✅   |
| Login Flow v2 (`/index.php/login/v2`)                  |   ✅   |
| User & group management via OCS (`/cloud/users`, `/cloud/groups`) | ✅ |
| Force-logout / disable user                            |   ✅   |
| Bootstrap-admin first-run flow                         |   ✅   |
| Two-factor authentication                              |   ❌   |
| WebAuthn / passkeys                                    |   ❌   |
| LDAP / Active Directory                                |   ❌   |
| SAML / OIDC SSO                                        |   ❌   |
| Brute-force throttling (global)                        |   ❌   |
| Rate-limited public-link password attempts             |   ✅   |

### Files

| Feature                                                | Status |
| ------------------------------------------------------ | :----: |
| WebDAV at `/dav/files/{user}/...` and `/remote.php/dav/...` |   ✅   |
| Chunked uploads (`/dav/uploads/{user}/{id}/...`)       |   ✅   |
| File cache + filesystem scanner                        |   ✅   |
| WebDAV properties & locks                              |   ✅   |
| Files web UI (browse, mkdir, rename, delete, move, upload) |   ✅ |
| Local-disk storage backend                             |   ✅   |
| In-memory storage backend (testing)                    |   ✅   |
| Per-user mount resolution                              |   ✅   |
| Trashbin                                               |   ❌   |
| File versioning                                        |   ❌   |
| Favorites                                              |   ❌   |
| Tags & comments                                        |   ❌   |
| Full-text & metadata search                            |   ❌   |
| Thumbnails / previews                                  |   ❌   |
| S3 / object-storage primary or external backend        |   ❌   |
| SMB / FTP / WebDAV external storage                    |   ❌   |
| Group folders                                          |   ❌   |
| Server-side encryption                                 |   ❌   |
| End-to-end encryption                                  |   ❌   |

### Sharing

| Feature                                                | Status |
| ------------------------------------------------------ | :----: |
| User shares                                            |   ✅   |
| Group shares                                           |   ✅   |
| Granular share permissions                             |   ✅   |
| Public link shares                                     |   ✅   |
| Password-protected public links                        |   ✅   |
| Public-link cookies & session handling                 |   ✅   |
| Share expiration dates                                 |   ❌   |
| Upload-only / file-drop links                          |   ❌   |
| Federated (server-to-server) shares                    |   ❌   |
| Share-by-email                                         |   ❌   |
| Circles                                                |   ❌   |

### Collaboration apps

| Feature                                                | Status |
| ------------------------------------------------------ | :----: |
| Calendar (CalDAV)                                      |   ❌   |
| Contacts (CardDAV)                                     |   ❌   |
| Mail                                                   |   ❌   |
| Talk (chat / calls)                                    |   ❌   |
| Notes                                                  |   ❌   |
| Deck (kanban)                                          |   ❌   |
| Tasks                                                  |   ❌   |
| Photos                                                 |   ❌   |
| Activity feed                                          |   ❌   |
| Notifications                                          |   ❌   |
| Collabora / OnlyOffice integration                     |   ❌   |

## Quick start

```bash
# 0. One-time tooling
rustup target add wasm32-unknown-unknown
cargo install dioxus-cli --version "^0.7"

# 1. Copy and edit the example config.
cp config/config.toml.example config/config.toml
#  - Set installed = true.
#  - Pick a dbtype (sqlite is easiest).
#  - For first login, add a [bootstrap_admin] section with a bcrypt password hash.

# 2. Build the UI and server together. Replace <host-triple> with
#    x86_64-unknown-linux-gnu / x86_64-pc-windows-msvc / aarch64-apple-darwin.
( cd crates/crabcloud-app && dx build --release \
    @client --no-default-features --features web --target wasm32-unknown-unknown \
    @server --no-default-features --features server --target <host-triple> )

# 3. Run migrations, then serve.
./target/dx/crabcloud-app/release/web/server --config config/config.toml migrate
./target/dx/crabcloud-app/release/web/server --config config/config.toml serve

# 4. Create your first admin user (interactive password prompt).
cargo run -p crabcloud-app -- user-add admin --admin

# 5. Visit http://127.0.0.1:8080/.
```

### Pairing a client

1. Sign in at `https://<your-server>/`.
2. Open `Settings → Security` and click **Create app password**.
3. Copy the displayed token (shown once) and paste it into your Nextcloud
   desktop / mobile / WebDAV client as the password.

Alternatively, point a client at `https://<your-server>/index.php/login/v2`
to use Nextcloud's authorize-in-browser flow.

### Administering via OCS

```http
POST /ocs/v2.php/cloud/users         # create user
PUT  /ocs/v2.php/cloud/users/<uid>/disable
GET  /ocs/v2.php/cloud/users?search=<term>
```

Authenticate with an admin session cookie or an admin app password. The
official Nextcloud Admin app works unmodified.

## Development

Crabcloud uses Dioxus 0.7's fullstack model: a single `dx` invocation
drives both the WASM client and the native server binary. `dx` performs
link-time substitution of `manganis` asset placeholders into hashed
`/assets/<…>.ext` paths, so any code path that needs the rendered page to
match the served bundle (notably the Playwright E2E suite) must go
through a `dx`-built binary.

### Hot-reload dev server

From `crates/crabcloud-app/`:

```bash
dx serve --release \
  @client --no-default-features --features web --target wasm32-unknown-unknown \
  @server --no-default-features --features server
```

`dx serve` spawns the server with `IP` + `PORT` env vars; the binary's
`Cmd::Serve` honors these (overriding `bind_address` in `config.toml`) so
HMR ws + asset reload reach the right address.

### Release build output

`target/dx/crabcloud-app/release/web/`:

- `server` (Linux/macOS) or `server.exe` (Windows) — the server binary.
- `public/assets/` — hashed CSS / JS / image / WASM bundles.
- `public/index.html` — generated shell.

### Cargo-only fallback

For CLI subcommands (`migrate`, `user-add`, `files scan`, ...) and
scripted server runs that don't need rendered HTML, plain `cargo` works:

```bash
cargo run --release -p crabcloud-app -- migrate
cargo run --release -p crabcloud-app -- user-add admin --admin
```

The cargo-built binary's SSR HTML contains `manganis` placeholder strings
for stylesheet/script hrefs — production and E2E use the `dx`-built
binary above.

## Workspace layout

- `crates/crabcloud-config` — layered TOML config loader.
- `crates/crabcloud-cache` — `Cache` trait + `MemoryCache` + `TypedCache<T>`.
- `crates/crabcloud-db` — `DbPool` enum, `MigrationRunner`, core schema.
- `crates/crabcloud-filecache` — DB-backed file cache + scanner.
- `crates/crabcloud-fs` — per-user filesystem facade (View + Uploads + mount resolution).
- `crates/crabcloud-i18n` — gettext `.po` loader, `Locale`, `I18n`.
- `crates/crabcloud-ocs` — OCS envelope (JSON/XML), capabilities aggregator.
- `crates/crabcloud-core` — `AppState`, `Error`, `AppConfigService`, `BootstrapHook`.
- `crates/crabcloud-http` — axum router, middleware, session, CSRF, auth extractors, OCS + WebDAV handlers.
- `crates/crabcloud-storage` — `Storage` async trait + `MemoryStorage` and `LocalStorage` backends.
- `crates/crabcloud-users` — user/group/preference stores, password verifier, `UsersService`, app passwords + token store.
- `crates/crabcloud-sharing` — user/group shares with granular permissions.
- `crates/crabcloud-publiclinks` — public link tokens, password gate, cookies, rate limiting.
- `crates/crabcloud-app` — Dioxus fullstack app (SSR + hydration) and the binary entrypoint (CLI, tracing, lifecycle), including the Files web app.
- `xtask/` — project automation.
- `e2e/` — Playwright tests.
- `migrations/core/` — core SQL migrations, per-dialect.
- `l10n/<app>/<locale>.po` — translation catalogs.

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for the development workflow.

## License

AGPL-3.0-or-later.
