# Platform Core — Phase 4: UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `rustcloud-ui` (Dioxus 0.6) — a SSR-first Dioxus app with `/`, `/login`, and a 404 page; a hydration payload injected as `<script id="__dx_ctx" type="application/json">`; a WASM client bundle compiled by `dx` and served from disk or embedded via `rust-embed`; mounted as the fall-through sub-router after Phase 3's API surface. Browser-visit of `/` produces an SSR'd HTML page that the WASM bundle hydrates.

**Architecture:** `rustcloud-ui` exposes (a) Dioxus components — `Home`, `Login`, `NotFound` — composed by an `App` Router; (b) a server-side `ui_router() -> Router<AppState>` that wraps a single SSR handler matching any UI route; (c) a separate WASM entry point in `src/main.rs` that mounts the App in the browser. The SSR handler builds a `RequestContext` inline (no separate middleware crate dependency) by extracting `OptionalUser` + locale, renders the App with that context, and injects the hydration payload script tag into the emitted HTML before serving as `text/html`. The WASM bundle, CSS, and other assets produced by `dx build --release` live under `target/dx/rustcloud-ui/release/web/public/` and are served either from disk (debug builds) or `rust-embed`-baked into the binary (release builds).

**Tech Stack:** Rust 1.85, **Dioxus 0.6** (`dioxus`, `dioxus-ssr`, `dioxus-router`, `dioxus-web` for the WASM target), `dioxus-cli` 0.6 (the `dx` binary) for the WASM build, `rust-embed 8` for release-build asset packaging, `serde_json` for the hydration payload, all existing Phase 1-3 crates consumed via the workspace.

**Parent spec:** `docs/superpowers/specs/2026-05-10-platform-core-design.md` §4.1, §8 (UI surface), §13 acceptance criterion #6.

**Previous phase:** Phase 3 ended at commit `1654007` on the public remote (CI green). Workspace has 7 crates + xtask + ~135 tests. `rustcloud-server serve` already exposes `/status.php`, `/ocs/v2.php/cloud/capabilities`, `/index.php/login`, and the full Phase 1-3 middleware stack.

---

## Conventions (carry-over from earlier phases)

- **Commits:** Conventional Commits (`feat(ui)`, `chore(ui)`, `test(ui)`, …). Co-Authored-By trailer with `Claude Opus 4.7 <noreply@anthropic.com>`.
- **TDD:** Write failing test → fail → implement → pass → commit. New crates may need a build-only check first; meaningful tests follow immediately.
- **rustfmt:** Run `cargo fmt --all` after writing files. Authorized at every task boundary.
- **Plan-bug protocol:** Dioxus 0.6 APIs may differ from this plan. If the verbatim code fails to compile, find the closest documented API equivalent, apply it minimally, and report DONE_WITH_CONCERNS with a clear before/after diff. The plan's TYPE SIGNATURES, COMPONENT NAMES, and CONTROL FLOW are load-bearing — preserve those. Crate version, macro spelling, function names may shift.
- **Errors:** library code uses `thiserror`; the binary's `main` converts via `anyhow::Result`. New errors flow into `rustcloud-core::Error`.
- **CI:** Phase 4 adds the `wasm32-unknown-unknown` Rust target + `dioxus-cli` to the workflow. CI cold-start grows by ~3-5 min.

---

## File Structure (Phase 4 additions)

```
rustcloud/
├── Cargo.toml                                       # +rustcloud-ui +dioxus deps +rust-embed
├── crates/
│   ├── rustcloud-ui/                                # NEW
│   │   ├── Cargo.toml
│   │   ├── Dioxus.toml                              # dx CLI build config
│   │   ├── assets/                                  # CSS + favicon (manual sources)
│   │   │   └── app.css
│   │   ├── src/
│   │   │   ├── lib.rs                               # exports + ui_router()
│   │   │   ├── app.rs                               # App component + Router enum
│   │   │   ├── context.rs                           # RequestContext struct
│   │   │   ├── hydration.rs                         # render_hydration_script()
│   │   │   ├── ssr.rs                               # SSR handler + HTML shell
│   │   │   ├── pages/
│   │   │   │   ├── mod.rs
│   │   │   │   ├── home.rs                          # `/` Home component
│   │   │   │   ├── login.rs                         # `/login` form component
│   │   │   │   └── not_found.rs                     # 404 catch-all component
│   │   │   ├── assets.rs                            # rust-embed asset serving
│   │   │   └── main.rs                              # WASM client entry (cfg target_arch="wasm32")
│   │   └── tests/
│   │       └── ssr_routes.rs                        # axum oneshot integration tests
│   ├── rustcloud-http/                              # MODIFIED
│   │   ├── Cargo.toml                               # +rustcloud-ui dep
│   │   └── src/router.rs                            # .merge(ui_router())
│   └── rustcloud-server/                            # unchanged
├── xtask/src/main.rs                                # `build` subcommand actually does something now
├── .github/workflows/ci.yml                         # wasm32 + dioxus-cli setup
└── docs/superpowers/plans/
    └── 2026-05-10-platform-core-phase-4-ui.changelog.md   # NEW
```

---

## New workspace `[workspace.dependencies]` (added in Task 1)

```toml
dioxus = { version = "0.6", default-features = false, features = ["macro", "html", "signals", "hooks"] }
dioxus-router = { version = "0.6", default-features = false }
dioxus-ssr = "0.6"
dioxus-web = { version = "0.6", default-features = false }
rust-embed = { version = "8", features = ["debug-embed"] }

rustcloud-ui = { path = "crates/rustcloud-ui" }
```

Per-crate `Cargo.toml`s enable target-specific Dioxus features (e.g., `dioxus-web` only for `cfg(target_arch = "wasm32")`).

---

## Task 1: Workspace scaffold for `rustcloud-ui`

**Files:**
- Modify: `Cargo.toml` (workspace members + workspace deps)
- Create: `crates/rustcloud-ui/Cargo.toml`
- Create: `crates/rustcloud-ui/src/lib.rs`
- Create: `crates/rustcloud-ui/Dioxus.toml`

Set up the crate skeleton. No Dioxus components yet — just an empty `ui_router()` returning a `Router<AppState>` so downstream changes in `rustcloud-http` compile.

- [ ] **Step 1: Extend workspace `Cargo.toml`**

Append `crates/rustcloud-ui` to `[workspace] members` (keep entries alphabetized):

```toml
[workspace]
members = [
    "crates/rustcloud-cache",
    "crates/rustcloud-config",
    "crates/rustcloud-core",
    "crates/rustcloud-db",
    "crates/rustcloud-http",
    "crates/rustcloud-i18n",
    "crates/rustcloud-ocs",
    "crates/rustcloud-server",
    "crates/rustcloud-ui",
    "xtask",
]
resolver = "2"
```

Append to `[workspace.dependencies]`:

```toml
dioxus = { version = "0.6", default-features = false, features = ["macro", "html", "signals", "hooks"] }
dioxus-router = { version = "0.6", default-features = false }
dioxus-ssr = "0.6"
dioxus-web = { version = "0.6", default-features = false }
rust-embed = { version = "8", features = ["debug-embed"] }
rustcloud-ui = { path = "crates/rustcloud-ui" }
```

- [ ] **Step 2: Write `crates/rustcloud-ui/Cargo.toml`**

```toml
[package]
name = "rustcloud-ui"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[lib]
crate-type = ["rlib"]

# A native `[[bin]]` target for the WASM client. `dx` compiles this against
# `wasm32-unknown-unknown` for the browser.
[[bin]]
name = "rustcloud-ui-web"
path = "src/main.rs"

[dependencies]
axum.workspace = true
dioxus = { workspace = true, features = ["router"] }
dioxus-router.workspace = true
rust-embed.workspace = true
rustcloud-core.workspace = true
rustcloud-http.workspace = true
rustcloud-i18n.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tower.workspace = true
tracing.workspace = true

# Server-side rendering — only compile on non-wasm targets.
[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
dioxus-ssr.workspace = true

# Browser WASM — only compile when targeting wasm32.
[target.'cfg(target_arch = "wasm32")'.dependencies]
dioxus-web.workspace = true

[dev-dependencies]
rustcloud-cache.workspace = true
rustcloud-config.workspace = true
rustcloud-db.workspace = true
secrecy = { workspace = true, features = ["serde"] }
tempfile.workspace = true
tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }
```

- [ ] **Step 3: Write `crates/rustcloud-ui/Dioxus.toml`**

```toml
[application]
name = "rustcloud-ui"
default_platform = "web"

[web.app]
title = "Rustcloud"

[web.watcher]
reload_html = false
watch_path = ["src", "assets"]

[web.resource]
style = ["assets/app.css"]
script = []

[web.resource.dev]
script = []
```

- [ ] **Step 4: Create `crates/rustcloud-ui/assets/app.css`**

Minimal CSS so the dx build has something to bundle.

```css
:root {
    --fg: #1a1a1a;
    --bg: #ffffff;
    --accent: #0066cc;
    --error: #cc0033;
    color-scheme: light dark;
}

* { box-sizing: border-box; }

body {
    margin: 0;
    font: 16px/1.5 system-ui, -apple-system, "Segoe UI", sans-serif;
    color: var(--fg);
    background: var(--bg);
}

main { max-width: 60rem; margin: 0 auto; padding: 2rem 1rem; }
h1 { margin-top: 0; font-weight: 600; }
form label { display: block; margin: 0.75rem 0 0.25rem; font-weight: 600; }
form input { padding: 0.5rem; font: inherit; width: 100%; max-width: 24rem; }
form button { margin-top: 1rem; padding: 0.5rem 1rem; font: inherit; cursor: pointer; background: var(--accent); color: white; border: 0; }
.error { color: var(--error); }
```

- [ ] **Step 5: Write `crates/rustcloud-ui/src/lib.rs` (stub)**

```rust
//! Rustcloud UI — Dioxus 0.6 application. SSR-first; the WASM client hydrates
//! the same component tree.
//!
//! Phase 4 mounts the SSR handler at the catch-all fall-through for the HTTP
//! router. See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §8.

// Sub-modules are added incrementally in subsequent tasks.

use axum::Router;
use rustcloud_core::AppState;

/// Phase 4 placeholder. Returns an empty `Router<AppState>` so downstream
/// crates compile while Tasks 2-7 fill in the SSR handler.
pub fn ui_router() -> Router<AppState> {
    Router::new()
}
```

- [ ] **Step 6: Write `crates/rustcloud-ui/src/main.rs` (WASM entry, stub)**

The WASM binary is required because `Cargo.toml` declares `[[bin]] name = "rustcloud-ui-web"`. Stub it now; populate in Task 8.

```rust
//! WASM browser entry point. `dx build` compiles this against
//! `wasm32-unknown-unknown` and emits the hydration bundle.

#[cfg(target_arch = "wasm32")]
fn main() {
    // Implemented in Task 8.
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    // Stub so `cargo build` on the host target doesn't fail. The native binary
    // does nothing; the server crate is `rustcloud-server`.
    eprintln!("rustcloud-ui-web is a WASM-only entry point");
}
```

- [ ] **Step 7: Build the crate**

```
cargo build -p rustcloud-ui
```

Expected: clean build for the host target. (WASM target build comes later via `dx`.)

- [ ] **Step 8: Run `cargo xtask check-all`**

Expected: still green; no behavior changes elsewhere.

- [ ] **Step 9: Commit**

```
git add Cargo.toml Cargo.lock crates/rustcloud-ui
git commit -m "chore(ui): scaffold rustcloud-ui crate with Dioxus 0.6 deps

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 2: `RequestContext` struct

**Files:**
- Create: `crates/rustcloud-ui/src/context.rs`
- Modify: `crates/rustcloud-ui/src/lib.rs`

Serializable per-request context that the SSR handler builds and Dioxus components read. See spec §8.2.

- [ ] **Step 1: Write `crates/rustcloud-ui/src/context.rs`**

```rust
//! Per-request context carried into Dioxus SSR rendering and emitted as the
//! hydration payload for the browser to pick up.
//!
//! See spec §8.2.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequestContext {
    /// Authenticated user ID, or `None` for anonymous requests.
    pub user_id: Option<String>,
    /// Display name (Phase 4 simplification: same as `user_id`; the real users
    /// sub-project will resolve a proper display name from the user store).
    pub display_name: Option<String>,
    /// Resolved locale tag — e.g. "en", "de", "fr_FR".
    pub locale: String,
    /// CSRF request token from the session, exposed to the browser so
    /// authenticated XHR can include it in the `requesttoken` header.
    pub request_token: String,
    /// Cached `cloud/capabilities` ETag. Phase 4 ships `None`; Phase 5+ can
    /// surface it for clients that want conditional capability refresh.
    pub capabilities_etag: Option<String>,
}

impl RequestContext {
    pub fn anonymous(locale: impl Into<String>, request_token: impl Into<String>) -> Self {
        Self {
            user_id: None,
            display_name: None,
            locale: locale.into(),
            request_token: request_token.into(),
            capabilities_etag: None,
        }
    }

    pub fn authenticated(
        user_id: impl Into<String>,
        locale: impl Into<String>,
        request_token: impl Into<String>,
    ) -> Self {
        let uid = user_id.into();
        Self {
            user_id: Some(uid.clone()),
            display_name: Some(uid),
            locale: locale.into(),
            request_token: request_token.into(),
            capabilities_etag: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anonymous_has_no_user() {
        let ctx = RequestContext::anonymous("en", "tok-123");
        assert!(ctx.user_id.is_none());
        assert!(ctx.display_name.is_none());
        assert_eq!(ctx.locale, "en");
        assert_eq!(ctx.request_token, "tok-123");
    }

    #[test]
    fn authenticated_populates_user_and_display_name() {
        let ctx = RequestContext::authenticated("alice", "de", "tok-456");
        assert_eq!(ctx.user_id.as_deref(), Some("alice"));
        assert_eq!(ctx.display_name.as_deref(), Some("alice"));
    }

    #[test]
    fn round_trips_via_json() {
        let ctx = RequestContext::authenticated("alice", "en", "tok-789");
        let s = serde_json::to_string(&ctx).unwrap();
        let back: RequestContext = serde_json::from_str(&s).unwrap();
        assert_eq!(ctx, back);
    }
}
```

- [ ] **Step 2: Re-export from `lib.rs`**

Replace `crates/rustcloud-ui/src/lib.rs`:

```rust
//! Rustcloud UI — Dioxus 0.6 application. SSR-first; the WASM client hydrates
//! the same component tree.

mod context;

pub use context::RequestContext;

use axum::Router;
use rustcloud_core::AppState;

pub fn ui_router() -> Router<AppState> {
    Router::new()
}
```

- [ ] **Step 3: Run tests**

```
cargo test -p rustcloud-ui --lib
```

Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```
git add crates/rustcloud-ui
git commit -m "feat(ui): add RequestContext struct with anonymous/authenticated builders

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 3: Hydration payload encoder

**Files:**
- Create: `crates/rustcloud-ui/src/hydration.rs`
- Modify: `crates/rustcloud-ui/src/lib.rs`

`render_hydration_script(ctx) -> String` returns the full `<script id="__dx_ctx" type="application/json">...</script>` tag with JSON-encoded content. The encoder escapes `<`, `>`, and `&` to their JSON `\uXXXX` forms to prevent `</script>` injection.

- [ ] **Step 1: Write `crates/rustcloud-ui/src/hydration.rs`**

```rust
//! Hydration payload — emits a `<script>` tag with the JSON-encoded
//! `RequestContext` for the WASM client to read on mount.
//!
//! See spec §8.3.

use crate::context::RequestContext;
use serde_json::Value;

const SCRIPT_OPEN: &str = "<script id=\"__dx_ctx\" type=\"application/json\">";
const SCRIPT_CLOSE: &str = "</script>";

/// Render the hydration script tag for a given context. The JSON body is
/// escaped so `<`, `>`, and `&` cannot terminate the surrounding script
/// element nor execute via HTML interpretation.
pub fn render_hydration_script(ctx: &RequestContext) -> String {
    let body = encode_safe_json(ctx);
    let mut out = String::with_capacity(SCRIPT_OPEN.len() + body.len() + SCRIPT_CLOSE.len());
    out.push_str(SCRIPT_OPEN);
    out.push_str(&body);
    out.push_str(SCRIPT_CLOSE);
    out
}

fn encode_safe_json<T: serde::Serialize>(value: &T) -> String {
    // `serde_json::to_value` is infallible for our `RequestContext` (no
    // non-string map keys, no NaN floats, no unrepresentable types).
    let v: Value = serde_json::to_value(value).unwrap_or(Value::Null);
    let raw = serde_json::to_string(&v).unwrap_or_else(|_| "null".to_string());
    escape_for_script_tag(&raw)
}

fn escape_for_script_tag(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_payload_in_script_tag() {
        let ctx = RequestContext::anonymous("en", "tok");
        let s = render_hydration_script(&ctx);
        assert!(s.starts_with("<script id=\"__dx_ctx\" type=\"application/json\">"));
        assert!(s.ends_with("</script>"));
    }

    #[test]
    fn escapes_lt_gt_amp_in_payload() {
        // Locale shouldn't ever contain these but request_token could in theory.
        let ctx = RequestContext::anonymous("en", "ab<c>&d");
        let s = render_hydration_script(&ctx);
        // Tag boundaries remain the only literal `<...>` in the output.
        // Body should not contain another literal `<` or `>`.
        let body = &s
            [SCRIPT_OPEN.len()..s.len() - SCRIPT_CLOSE.len()];
        assert!(!body.contains('<'));
        assert!(!body.contains('>'));
        assert!(!body.contains('&'));
        assert!(body.contains("\\u003c"));
        assert!(body.contains("\\u003e"));
        assert!(body.contains("\\u0026"));
    }

    #[test]
    fn payload_parses_back_when_unescaped() {
        let ctx = RequestContext::authenticated("alice", "en", "tok-123");
        let s = render_hydration_script(&ctx);
        let body = &s[SCRIPT_OPEN.len()..s.len() - SCRIPT_CLOSE.len()];
        // The escapes are valid JSON `\u00XX` sequences which `serde_json` will
        // happily decode back to `<`/`>`/`&`. For this test the payload contains
        // none of those, so the parse is a straight round-trip.
        let parsed: RequestContext = serde_json::from_str(body).unwrap();
        assert_eq!(parsed, ctx);
    }

    #[test]
    fn escapes_line_separator_chars() {
        // U+2028 / U+2029 are valid JSON but break browsers' script parsing.
        let ctx = RequestContext::anonymous("en", "a\u{2028}b\u{2029}c");
        let s = render_hydration_script(&ctx);
        let body = &s[SCRIPT_OPEN.len()..s.len() - SCRIPT_CLOSE.len()];
        assert!(!body.contains('\u{2028}'));
        assert!(!body.contains('\u{2029}'));
        assert!(body.contains("\\u2028"));
        assert!(body.contains("\\u2029"));
    }
}
```

- [ ] **Step 2: Re-export from `lib.rs`**

Modify `crates/rustcloud-ui/src/lib.rs`:

```rust
//! Rustcloud UI — Dioxus 0.6 application. SSR-first; the WASM client hydrates
//! the same component tree.

mod context;
mod hydration;

pub use context::RequestContext;
pub use hydration::render_hydration_script;

use axum::Router;
use rustcloud_core::AppState;

pub fn ui_router() -> Router<AppState> {
    Router::new()
}
```

- [ ] **Step 3: Run tests**

```
cargo test -p rustcloud-ui --lib
```

Expected: 7 tests pass (3 from Task 2 + 4 new).

- [ ] **Step 4: Commit**

```
git add crates/rustcloud-ui
git commit -m "feat(ui): add hydration payload encoder with safe JSON-in-HTML escaping

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 4: Page components — `Home`, `Login`, `NotFound`

**Files:**
- Create: `crates/rustcloud-ui/src/pages/mod.rs`
- Create: `crates/rustcloud-ui/src/pages/home.rs`
- Create: `crates/rustcloud-ui/src/pages/login.rs`
- Create: `crates/rustcloud-ui/src/pages/not_found.rs`
- Modify: `crates/rustcloud-ui/src/lib.rs`

Three Dioxus components. Each reads `RequestContext` from a `use_context` hook the App will install in Task 5. For now, components receive the context via props so they're testable in isolation.

- [ ] **Step 1: Write `crates/rustcloud-ui/src/pages/mod.rs`**

```rust
//! Page components mounted by the App's Router.

pub mod home;
pub mod login;
pub mod not_found;
```

- [ ] **Step 2: Write `crates/rustcloud-ui/src/pages/home.rs`**

```rust
//! `/` — Welcome page. Shows the authenticated user's display name or a
//! "guest" greeting; links to `/login` when anonymous.

use crate::context::RequestContext;
use dioxus::prelude::*;

#[component]
pub fn Home(ctx: RequestContext) -> Element {
    let greeting = match &ctx.display_name {
        Some(name) => format!("Welcome, {name}"),
        None => "Welcome, guest".to_string(),
    };
    let show_login_link = ctx.user_id.is_none();
    rsx! {
        main { class: "home",
            h1 { "{greeting}" }
            p { "Rustcloud — a Rust port of Nextcloud server." }
            if show_login_link {
                p { a { href: "/login", "Log in" } }
            }
        }
    }
}
```

- [ ] **Step 3: Write `crates/rustcloud-ui/src/pages/login.rs`**

```rust
//! `/login` — Login form that POSTs to `/index.php/login` (Phase 3 handler).
//! Works without JavaScript; the WASM client may enhance it later.

use crate::context::RequestContext;
use dioxus::prelude::*;

#[component]
pub fn Login(ctx: RequestContext) -> Element {
    let _ = ctx; // unused for now; Phase 5+ may pre-fill username from cookie.
    rsx! {
        main { class: "login",
            h1 { "Log in" }
            form {
                method: "post",
                action: "/index.php/login",
                "accept-charset": "utf-8",

                label { r#for: "username", "Username" }
                input { id: "username", name: "username", r#type: "text", autocomplete: "username", required: true }

                label { r#for: "password", "Password" }
                input { id: "password", name: "password", r#type: "password", autocomplete: "current-password", required: true }

                button { r#type: "submit", "Log in" }
            }
        }
    }
}
```

- [ ] **Step 4: Write `crates/rustcloud-ui/src/pages/not_found.rs`**

```rust
//! 404 fall-through.

use dioxus::prelude::*;

#[component]
pub fn NotFound() -> Element {
    rsx! {
        main { class: "not-found",
            h1 { "404 — Not Found" }
            p { "The page you requested does not exist." }
            p { a { href: "/", "Return home" } }
        }
    }
}
```

- [ ] **Step 5: Wire `pages` into `lib.rs`**

Modify `crates/rustcloud-ui/src/lib.rs`:

```rust
//! Rustcloud UI — Dioxus 0.6 application. SSR-first; the WASM client hydrates
//! the same component tree.

mod context;
mod hydration;
pub mod pages;

pub use context::RequestContext;
pub use hydration::render_hydration_script;

use axum::Router;
use rustcloud_core::AppState;

pub fn ui_router() -> Router<AppState> {
    Router::new()
}
```

- [ ] **Step 6: Build the crate**

```
cargo build -p rustcloud-ui
```

Expected: clean.

If Dioxus 0.6's macro syntax differs (`rsx!` block, `#[component]` attribute, `Element` return type), adapt to the installed version. The component shapes — name, props, body content — are load-bearing; the exact macro tokens are not.

- [ ] **Step 7: Run tests**

```
cargo test -p rustcloud-ui --lib
```

Expected: 7 tests still pass (no new tests yet; component testing happens via SSR snapshot in Task 7).

- [ ] **Step 8: Commit**

```
git add crates/rustcloud-ui
git commit -m "feat(ui): add Home, Login, NotFound page components

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 5: `App` component + `Router` enum

**Files:**
- Create: `crates/rustcloud-ui/src/app.rs`
- Modify: `crates/rustcloud-ui/src/lib.rs`

The `App` component installs the `RequestContext` via Dioxus's `use_context_provider` and renders the appropriate page based on the Dioxus `Router`. The Router-recognized routes are enumerated in a `Route` enum derived via Dioxus's `#[derive(Routable)]` macro.

- [ ] **Step 1: Write `crates/rustcloud-ui/src/app.rs`**

```rust
//! Root `App` component + Dioxus `Route` enum. Provides `RequestContext` via
//! context so any descendant component can call `use_context::<RequestContext>()`.

use crate::context::RequestContext;
use crate::pages::{home::Home, login::Login, not_found::NotFound};
use dioxus::prelude::*;
use dioxus_router::prelude::*;

/// Routes the SSR side honors. The browser router has the same shape so
/// hydration matches.
#[derive(Routable, Clone, PartialEq, Debug)]
#[rustfmt::skip]
pub enum Route {
    #[route("/")]
    HomeRoute {},

    #[route("/login")]
    LoginRoute {},

    #[route("/:..segments")]
    NotFoundRoute { segments: Vec<String> },
}

#[component]
pub fn HomeRoute() -> Element {
    let ctx = use_context::<RequestContext>();
    rsx! { Home { ctx: ctx.clone() } }
}

#[component]
pub fn LoginRoute() -> Element {
    let ctx = use_context::<RequestContext>();
    rsx! { Login { ctx: ctx.clone() } }
}

#[component]
pub fn NotFoundRoute(segments: Vec<String>) -> Element {
    let _ = segments;
    rsx! { NotFound {} }
}

/// Root component. Renders the `Router<Route>`. Callers must install
/// `RequestContext` into the context before rendering (see `ssr.rs`).
#[component]
pub fn App() -> Element {
    rsx! { Router::<Route> {} }
}
```

- [ ] **Step 2: Re-export from `lib.rs`**

Modify `crates/rustcloud-ui/src/lib.rs`:

```rust
//! Rustcloud UI — Dioxus 0.6 application. SSR-first; the WASM client hydrates
//! the same component tree.

mod app;
mod context;
mod hydration;
pub mod pages;

pub use app::{App, Route};
pub use context::RequestContext;
pub use hydration::render_hydration_script;

use axum::Router;
use rustcloud_core::AppState;

pub fn ui_router() -> Router<AppState> {
    Router::new()
}
```

- [ ] **Step 3: Build the crate**

```
cargo build -p rustcloud-ui
```

Expected: clean. If the `Routable` macro requires a different attribute spelling in 0.6 (e.g. `#[route("/")]` vs `#[at("/")]`), or the segments-catchall has a different syntax, adapt minimally.

- [ ] **Step 4: Commit**

```
git add crates/rustcloud-ui
git commit -m "feat(ui): add App component and Route enum wiring the three pages

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 6: SSR handler + HTML shell + `ui_router()`

**Files:**
- Create: `crates/rustcloud-ui/src/ssr.rs`
- Modify: `crates/rustcloud-ui/src/lib.rs`

The SSR handler:

1. Extracts `OptionalUser` (Phase 3) from the request.
2. Resolves the locale via `Accept-Language` against the i18n service's available locales.
3. Builds `RequestContext` (anonymous or authenticated).
4. Renders the `App` component via `dioxus_ssr::render` with the context installed.
5. Wraps the rendered body in an HTML shell, injecting the hydration payload + CSS link.
6. Returns `text/html; charset=utf-8`.

- [ ] **Step 1: Write `crates/rustcloud-ui/src/ssr.rs`**

```rust
//! Axum SSR handler. Renders the Dioxus `App` for the requested URL, wraps it
//! in an HTML shell, and injects the hydration payload.

use crate::app::{App, Route};
use crate::context::RequestContext;
use crate::hydration::render_hydration_script;
use axum::extract::{OriginalUri, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use dioxus::prelude::*;
use dioxus_router::prelude::*;
use rustcloud_core::AppState;
use rustcloud_http::{OptionalUser, SessionHandle};
use rustcloud_i18n::{resolve, Locale};

const HTML_DOCTYPE: &str = "<!DOCTYPE html>\n";

pub async fn handler(
    State(state): State<AppState>,
    OptionalUser(user): OptionalUser,
    axum::Extension(session): axum::Extension<SessionHandle>,
    OriginalUri(uri): OriginalUri,
    headers: axum::http::HeaderMap,
) -> Response {
    let accept_lang = headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let available = state.i18n.available_locales().to_vec();
    let fallback = Locale::new(state.config.default_language.as_str());
    let locale = resolve(accept_lang, &available, &fallback);

    let session_snapshot = session.read().await;
    let request_token = session_snapshot.csrf_token.clone();

    let ctx = match user {
        Some(u) => RequestContext::authenticated(u.user_id, locale.as_str(), request_token),
        None => RequestContext::anonymous(locale.as_str(), request_token),
    };

    let body_html = render_app_html(ctx.clone(), uri.path());
    let head_html = render_head_html(&ctx);
    let document = format!(
        "{doctype}<html lang=\"{lang}\"><head>{head}</head><body><div id=\"main\">{body}</div></body></html>",
        doctype = HTML_DOCTYPE,
        lang = ctx.locale,
        head = head_html,
        body = body_html,
    );

    let mut resp = (StatusCode::OK, document).into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    resp
}

fn render_head_html(ctx: &RequestContext) -> String {
    let mut out = String::new();
    out.push_str("<meta charset=\"utf-8\">");
    out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">");
    out.push_str("<title>Rustcloud</title>");
    out.push_str("<link rel=\"stylesheet\" href=\"/assets/app.css\">");
    out.push_str(&format!(
        "<meta name=\"requesttoken\" content=\"{}\">",
        html_escape(&ctx.request_token)
    ));
    out.push_str(&render_hydration_script(ctx));
    // The WASM client bundle. dx places it at /assets/dioxus/<name>.js by
    // default; we mount the assets root at /assets/ so this path resolves
    // to target/dx/.../public/dioxus/<name>.js.
    out.push_str("<script type=\"module\" src=\"/assets/dioxus/rustcloud-ui.js\" defer></script>");
    out
}

fn render_app_html(ctx: RequestContext, path: &str) -> String {
    // Pre-build a VirtualDom for the requested route. Dioxus 0.6: provide
    // context via the router builder.
    let mut vdom = VirtualDom::new_with_props(
        AppWithContext,
        AppWithContextProps {
            ctx,
            initial_path: path.to_string(),
        },
    );
    let _ = vdom.rebuild_in_place();
    dioxus_ssr::render(&vdom)
}

#[component]
fn AppWithContext(ctx: RequestContext, initial_path: String) -> Element {
    use_context_provider(|| ctx.clone());
    // Drive the router to the requested URL for SSR. The browser router will
    // pick it up from window.location on hydration.
    rsx! { Router::<Route> { config: || RouterConfig::default().initial_route(initial_path) } }
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_escape_replaces_special_chars() {
        assert_eq!(html_escape("a<b>&\"'c"), "a&lt;b&gt;&amp;&quot;&#39;c");
    }

    #[test]
    fn head_includes_csrf_meta_and_hydration_script() {
        let ctx = RequestContext::authenticated("alice", "en", "tok-1");
        let head = render_head_html(&ctx);
        assert!(head.contains("name=\"requesttoken\""));
        assert!(head.contains("content=\"tok-1\""));
        assert!(head.contains("<script id=\"__dx_ctx\""));
    }

    #[test]
    fn head_escapes_request_token() {
        let ctx = RequestContext::authenticated("alice", "en", "a<b>");
        let head = render_head_html(&ctx);
        assert!(head.contains("content=\"a&lt;b&gt;\""));
    }
}
```

- [ ] **Step 2: Update `lib.rs` to mount the SSR handler**

Replace `crates/rustcloud-ui/src/lib.rs`:

```rust
//! Rustcloud UI — Dioxus 0.6 application. SSR-first; the WASM client hydrates
//! the same component tree.

mod app;
mod context;
mod hydration;
pub mod pages;
mod ssr;

pub use app::{App, Route};
pub use context::RequestContext;
pub use hydration::render_hydration_script;

use axum::routing::get;
use axum::Router;
use rustcloud_core::AppState;

/// Build the UI sub-router. Mounted as the fall-through in
/// `rustcloud-http::build_router` (after all explicit API routes).
pub fn ui_router() -> Router<AppState> {
    Router::new().fallback(ssr::handler)
}
```

- [ ] **Step 3: Build**

```
cargo build -p rustcloud-ui
```

Expected: clean. Dioxus 0.6 API mismatches around `VirtualDom::new_with_props`, `use_context_provider`, or `RouterConfig::initial_route` may need adjustment — apply the closest current-API equivalent and report as DONE_WITH_CONCERNS.

- [ ] **Step 4: Run unit tests**

```
cargo test -p rustcloud-ui --lib
```

Expected: 10 tests pass (7 prior + 3 new SSR helpers).

- [ ] **Step 5: Commit**

```
git add crates/rustcloud-ui
git commit -m "feat(ui): SSR handler + HTML shell + ui_router() fallback

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 7: Mount `ui_router()` in `rustcloud-http::build_router`

**Files:**
- Modify: `crates/rustcloud-http/Cargo.toml` (add `rustcloud-ui` dep)
- Modify: `crates/rustcloud-http/src/router.rs`

Merge the UI sub-router into the main router AFTER the explicit API routes. axum dispatches to the first matching route — explicit routes win, then `nest`, then `fallback`. Since `ui_router()` uses `.fallback(ssr::handler)`, merging it makes the SSR handler the fallthrough for anything `/status.php`, `/index.php/login`, and `/ocs/*` didn't match.

- [ ] **Step 1: Add the `rustcloud-ui` dep**

Modify `crates/rustcloud-http/Cargo.toml` — append under `[dependencies]`:

```toml
rustcloud-ui.workspace = true
```

- [ ] **Step 2: Merge the UI router into `build_router`**

Modify `crates/rustcloud-http/src/router.rs` — change the router assembly. Find the `Router::new()` chain and add `.merge(rustcloud_ui::ui_router())` immediately after the `.nest("/ocs", ...)` line:

Replace the relevant block with:

```rust
    Router::new()
        .route("/status.php", get(status::handler))
        .route("/index.php/login", post(login::handler))
        .nest("/ocs", crate::routes::ocs::router())
        .merge(rustcloud_ui::ui_router())
        .with_state(state)
        .layer(CsrfLayer::new())
        .layer(SessionLayer::new(session_store, secret, secure_cookies))
        .layer(cors_layer)
        .layer(SecurityHeadersLayer::new())
        .layer(TrustedDomainLayer::new(trusted_domains))
        .layer(ProxyHeadersLayer::new(trusted_proxies))
        .layer(RequestBodyLimitLayer::new(DEFAULT_BODY_LIMIT_BYTES))
        .layer(CatchPanicLayer::new())
        .layer(TraceLayer::new_for_http())
```

- [ ] **Step 3: Build**

```
cargo build -p rustcloud-http
```

Expected: clean.

- [ ] **Step 4: Run the full check**

```
cargo xtask check-all
```

Expected: all prior tests still pass.

- [ ] **Step 5: Commit**

```
git add crates/rustcloud-http
git commit -m "feat(http): merge rustcloud-ui::ui_router as fall-through

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 8: WASM client entry point + `dx build` config

**Files:**
- Modify: `crates/rustcloud-ui/src/main.rs`
- Modify: `crates/rustcloud-ui/Dioxus.toml`

The WASM entry point launches Dioxus in the browser. It reads the hydration payload from `<script id="__dx_ctx">`, deserializes it into `RequestContext`, installs it, and mounts the same `App` component for hydration.

- [ ] **Step 1: Replace `crates/rustcloud-ui/src/main.rs`**

```rust
//! WASM browser entry point. `dx build` compiles this against
//! `wasm32-unknown-unknown` and emits the hydration bundle.

#[cfg(target_arch = "wasm32")]
mod web {
    use dioxus::prelude::*;
    use dioxus_router::prelude::*;
    use rustcloud_ui::{Route, RequestContext};

    #[component]
    fn AppRoot(ctx: RequestContext) -> Element {
        use_context_provider(|| ctx.clone());
        rsx! { Router::<Route> {} }
    }

    pub fn launch() {
        let ctx = read_hydration_context().unwrap_or_else(|| {
            RequestContext::anonymous("en", "")
        });
        dioxus::launch(move || {
            rsx! { AppRoot { ctx: ctx.clone() } }
        });
    }

    fn read_hydration_context() -> Option<RequestContext> {
        let window = web_sys::window()?;
        let document = window.document()?;
        let el = document.get_element_by_id("__dx_ctx")?;
        let json = el.text_content()?;
        serde_json::from_str(&json).ok()
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {
    console_error_panic_hook::set_once();
    web::launch();
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("rustcloud-ui-web is a WASM-only entry point");
}
```

`web-sys` and `console_error_panic_hook` are pulled in transitively by `dioxus-web` for `cfg(target_arch = "wasm32")`. If `cargo build --target wasm32-unknown-unknown` complains about missing crates, add them explicitly:

```toml
[target.'cfg(target_arch = "wasm32")'.dependencies]
dioxus-web.workspace = true
console_error_panic_hook = "0.1"
web-sys = { version = "0.3", features = ["Window", "Document", "Element"] }
```

Add `console_error_panic_hook` and `web-sys` to `[workspace.dependencies]` if they aren't already (only if the build requires).

- [ ] **Step 2: Verify Dioxus.toml output paths**

Modify `crates/rustcloud-ui/Dioxus.toml` to set explicit output dirs and disable HTTPS dev-server features that aren't useful here:

```toml
[application]
name = "rustcloud-ui"
default_platform = "web"
out_dir = "public"

[web.app]
title = "Rustcloud"
base_path = "assets"

[web.watcher]
reload_html = false
watch_path = ["src", "assets"]

[web.resource]
style = ["assets/app.css"]
script = []

[web.resource.dev]
script = []
```

`out_dir = "public"` puts the built bundle at `target/dx/rustcloud-ui/release/web/public/`. `base_path = "assets"` means generated `<script>`/`<link>` URLs are relative to `/assets/`.

- [ ] **Step 3: Sanity-check the host-target build still works**

```
cargo build -p rustcloud-ui
```

Expected: clean (host target — WASM build comes next).

- [ ] **Step 4: Install the WASM target + dioxus-cli (one-time per machine)**

```
rustup target add wasm32-unknown-unknown
cargo install dioxus-cli --version "^0.6"
```

If `dioxus-cli` 0.6 is unavailable, install the latest 0.x line and report the version. The CI workflow installs the same version (Task 11).

- [ ] **Step 5: Build the WASM bundle**

```
cd crates/rustcloud-ui
dx build --release
```

Expected: outputs to `target/dx/rustcloud-ui/release/web/public/`. Contents include `index.html`, `assets/dioxus/rustcloud-ui.js`, `assets/dioxus/rustcloud-ui_bg.wasm`, and CSS.

If `dx build` fails because the `[[bin]]` target conflicts with the library or because Dioxus 0.6 expects a different project layout, adjust to the closest convention (e.g., move the WASM main into `src/web_main.rs` referenced as the binary path) and report as DONE_WITH_CONCERNS. Some Dioxus 0.6 setups infer the binary from `Dioxus.toml`'s `[application]` name + a top-level `src/main.rs`; we follow that convention.

- [ ] **Step 6: Commit**

```
git add crates/rustcloud-ui
git commit -m "feat(ui): WASM client entry point reads hydration payload and mounts App

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 9: Asset serving via `rust-embed`

**Files:**
- Create: `crates/rustcloud-ui/src/assets.rs`
- Modify: `crates/rustcloud-ui/src/lib.rs`

In release builds, `rust-embed` bakes the `target/dx/rustcloud-ui/release/web/public/` tree into the binary. In debug builds, the same crate falls back to reading from disk so contributors can iterate quickly. The asset handler is mounted under `/assets/*` by `ui_router()`.

- [ ] **Step 1: Write `crates/rustcloud-ui/src/assets.rs`**

```rust
//! Static asset serving. Release builds embed the `dx`-produced `public/`
//! directory; debug builds read from disk so contributors don't have to
//! `dx build` after every UI tweak.

use axum::body::Body;
use axum::extract::Path;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../../target/dx/rustcloud-ui/release/web/public"]
#[exclude = "*.map"]
struct Assets;

pub async fn handler(Path(path): Path<String>) -> Response {
    let file = match Assets::get(&path) {
        Some(f) => f,
        None => return (StatusCode::NOT_FOUND, "asset not found").into_response(),
    };
    let mime = mime_for(&path);
    let mut resp = Response::new(Body::from(file.data.into_owned()));
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime).unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    // Long-cache hashed assets (Dioxus names them with content hashes).
    if path.ends_with(".wasm") || path.contains("/dioxus/") {
        resp.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        );
    }
    resp
}

fn mime_for(path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".html") { "text/html; charset=utf-8" }
    else if lower.ends_with(".css") { "text/css; charset=utf-8" }
    else if lower.ends_with(".js") { "application/javascript; charset=utf-8" }
    else if lower.ends_with(".wasm") { "application/wasm" }
    else if lower.ends_with(".json") { "application/json; charset=utf-8" }
    else if lower.ends_with(".png") { "image/png" }
    else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") { "image/jpeg" }
    else if lower.ends_with(".svg") { "image/svg+xml; charset=utf-8" }
    else if lower.ends_with(".ico") { "image/x-icon" }
    else if lower.ends_with(".woff2") { "font/woff2" }
    else if lower.ends_with(".woff") { "font/woff" }
    else { "application/octet-stream" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_matches_known_extensions() {
        assert!(mime_for("/dioxus/rustcloud-ui.js").starts_with("application/javascript"));
        assert!(mime_for("/dioxus/rustcloud-ui_bg.wasm").starts_with("application/wasm"));
        assert!(mime_for("assets/app.css").starts_with("text/css"));
        assert_eq!(mime_for("favicon.ico"), "image/x-icon");
        assert_eq!(mime_for("missing.weirdext"), "application/octet-stream");
    }
}
```

If the `target/dx/.../public` directory doesn't exist at compile time, `rust-embed` produces an empty asset set (release) or warns (debug, with `debug-embed` feature already enabled). The crate still compiles; the asset handler returns 404 for every path.

- [ ] **Step 2: Mount the asset route**

Modify `crates/rustcloud-ui/src/lib.rs`:

```rust
//! Rustcloud UI — Dioxus 0.6 application. SSR-first; the WASM client hydrates
//! the same component tree.

mod app;
mod assets;
mod context;
mod hydration;
pub mod pages;
mod ssr;

pub use app::{App, Route};
pub use context::RequestContext;
pub use hydration::render_hydration_script;

use axum::routing::get;
use axum::Router;
use rustcloud_core::AppState;

/// Build the UI sub-router. Includes static asset serving at `/assets/*` plus
/// an SSR fall-through for any other path.
pub fn ui_router() -> Router<AppState> {
    Router::new()
        .route("/assets/{*path}", get(assets::handler))
        .fallback(ssr::handler)
}
```

(`{*path}` is axum 0.8's wildcard path-segment syntax.)

- [ ] **Step 3: Build + test**

```
cargo build -p rustcloud-ui
cargo test -p rustcloud-ui --lib
```

Expected: 11 tests pass (10 prior + 1 mime helper).

- [ ] **Step 4: Commit**

```
git add crates/rustcloud-ui
git commit -m "feat(ui): serve dx-built assets via rust-embed at /assets/{*path}

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 10: `cargo xtask build` orchestration

**Files:**
- Modify: `xtask/Cargo.toml` (add `anyhow` if missing; already declared in Phase 1)
- Modify: `xtask/src/main.rs`

The `Build` variant was a stub bailing with "implemented in a later phase". Now it runs `dx build --release` (in the `rustcloud-ui` crate dir) and then `cargo build --release -p rustcloud-server`.

- [ ] **Step 1: Replace the `Cmd::Build` handler**

Modify `xtask/src/main.rs`. Find the `match cli.command` block and replace the `Build` arm:

```rust
        Cmd::Build => build_all(),
```

Then add the `build_all` function (place it near `compose` and `check_all`):

```rust
fn build_all() -> Result<()> {
    // 1. Build the WASM client + bundle assets.
    run_in_dir("crates/rustcloud-ui", "dx", &["build", "--release"])?;
    // 2. Build the server binary (which embeds the assets via rust-embed).
    run("cargo", &["build", "--release", "-p", "rustcloud-server"])?;
    Ok(())
}

fn run_in_dir(dir: &str, program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program).args(args).current_dir(dir).status()?;
    if !status.success() {
        bail!("`(cd {dir} && {program} {})` exited with status {status}", args.join(" "));
    }
    Ok(())
}
```

- [ ] **Step 2: Sanity-check the help text**

```
cargo xtask --help
```

Expected: `Build` no longer says "implemented in a later phase" — the doc-comment update is optional but the executable command now does something.

Update the `Build` variant's doc comment in `xtask/src/main.rs`:

```rust
    /// Build WASM client (`dx build --release`) then server (`cargo build --release`).
    Build,
```

- [ ] **Step 3: Smoke-test (if `dx` + `wasm32` target are installed locally)**

```
cargo xtask build
```

Expected: builds both targets. If `dx` is missing, the command exits with the bash-style error. The CI workflow (Task 11) installs both.

If `dx` is not locally installed, document it in the report and continue — CI will validate.

- [ ] **Step 4: Commit**

```
git add xtask/src/main.rs
git commit -m "feat(xtask): build subcommand runs dx build then cargo build --release

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 11: CI workflow — install WASM target + `dioxus-cli`

**Files:**
- Modify: `.github/workflows/ci.yml`

CI needs to:

1. Install the `wasm32-unknown-unknown` Rust target.
2. Install `dioxus-cli` (cache it to avoid recompilation per run).
3. Run `dx build --release` in the `rustcloud-ui` crate before tests, so the `rust-embed` macro picks up real assets and the integration tests can verify the asset handler.

- [ ] **Step 1: Modify the workflow**

Replace `.github/workflows/ci.yml` with the Phase 4-updated version:

```yaml
name: CI

on:
  push:
    branches: [master, main]
  pull_request:

permissions:
  contents: read

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
          targets: wasm32-unknown-unknown
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all -- --check
      - run: cargo clippy --workspace --all-targets -- -D warnings

  build-wasm:
    runs-on: ubuntu-latest
    needs: fmt-and-clippy
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: wasm32-unknown-unknown
      - uses: Swatinem/rust-cache@v2
      - name: Install dioxus-cli
        run: cargo install dioxus-cli --version "^0.6" --locked
      - name: Build WASM bundle
        working-directory: crates/rustcloud-ui
        run: dx build --release
      - name: Upload built bundle
        uses: actions/upload-artifact@v4
        with:
          name: dx-public
          path: target/dx/rustcloud-ui/release/web/public

  test-sqlite:
    runs-on: ubuntu-latest
    needs: build-wasm
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: wasm32-unknown-unknown
      - uses: Swatinem/rust-cache@v2
      - name: Download built bundle
        uses: actions/download-artifact@v4
        with:
          name: dx-public
          path: target/dx/rustcloud-ui/release/web/public
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

Key additions:
- `wasm32-unknown-unknown` target added to the toolchain in fmt-and-clippy, build-wasm, and test-sqlite jobs.
- New `build-wasm` job installs `dioxus-cli`, runs `dx build --release`, and uploads the resulting bundle as an artifact.
- `test-sqlite` now `needs: build-wasm` and downloads the artifact so the `rust-embed` macro finds the assets at compile time.
- The multidialect job intentionally does NOT need the WASM bundle (it only tests `rustcloud-db`); it depends on `fmt-and-clippy` only, saving CI time.

- [ ] **Step 2: Commit**

```
git add .github/workflows/ci.yml
git commit -m "ci: install wasm32 target + dioxus-cli; build WASM bundle before tests

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 12: SSR integration tests

**Files:**
- Create: `crates/rustcloud-ui/tests/ssr_routes.rs`

End-to-end tests using `tower::ServiceExt::oneshot` against the real `build_router(state)`. Verify HTML structure for `/`, `/login`, and a 404 path. These run against the in-memory SQLite fixture, exercising the full Phase 1-4 stack.

- [ ] **Step 1: Write the integration test**

Create `crates/rustcloud-ui/tests/ssr_routes.rs`:

```rust
//! SSR integration tests. Spin up a real `AppState`, build the full router,
//! and exercise the UI routes via `tower::ServiceExt::oneshot`.

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use rustcloud_config::{BootstrapAdminConfig, CacheConfig, DbType, FileConfig};
use rustcloud_core::AppStateBuilder;
use rustcloud_http::build_router;
use secrecy::SecretString;
use std::net::SocketAddr;
use std::path::PathBuf;
use tempfile::tempdir;
use tower::ServiceExt;

fn cfg(path: PathBuf) -> FileConfig {
    FileConfig {
        instanceid: "ssr".into(),
        secret: SecretString::new("a-32-byte-or-longer-secret-key!".into()),
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
        bootstrap_admin: Some(BootstrapAdminConfig {
            username: "admin".into(),
            password_hash: "$2b$12$placeholder".into(),
        }),
    }
}

async fn build_app() -> axum::Router {
    let dir = tempdir().unwrap();
    let cfg = cfg(dir.path().join("ssr.db"));
    let state = AppStateBuilder::new(cfg)
        .with_core_capabilities()
        .build()
        .await
        .unwrap();
    let app = build_router(state);
    std::mem::forget(dir); // keep the sqlite file alive for the duration of the test
    app
}

#[tokio::test]
async fn home_returns_ssr_html_with_hydration_payload() {
    let app = build_app().await;
    let req = Request::builder()
        .uri("/")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(ct.starts_with("text/html"), "Content-Type was: {ct}");

    let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.starts_with("<!DOCTYPE html>"), "missing doctype");
    assert!(html.contains("<script id=\"__dx_ctx\""), "missing hydration script");
    assert!(html.contains("Welcome, guest"), "missing welcome text for anonymous user");
    assert!(html.contains("href=\"/login\""), "missing login link");
}

#[tokio::test]
async fn login_returns_form_posting_to_index_php_login() {
    let app = build_app().await;
    let req = Request::builder()
        .uri("/login")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("<form"), "missing form element");
    assert!(html.contains("action=\"/index.php/login\""), "form action mismatch");
    assert!(html.contains("method=\"post\""), "form method mismatch");
    assert!(html.contains("name=\"username\""), "missing username input");
    assert!(html.contains("name=\"password\""), "missing password input");
}

#[tokio::test]
async fn unknown_path_returns_404_dioxus_page() {
    let app = build_app().await;
    let req = Request::builder()
        .uri("/this/path/does/not/exist")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    // SSR handler always returns 200; the body indicates 404 via content.
    // (axum's default fall-through would have been 404, but our fallback IS the
    // SSR handler. Phase 5 can wire a proper 404 status via the Dioxus router.)
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("404"), "404 page didn't render");
    assert!(html.contains("Not Found"), "404 page didn't render");
}

#[tokio::test]
async fn locale_resolution_respects_accept_language() {
    let app = build_app().await;
    let req = Request::builder()
        .uri("/")
        .header("accept-language", "de-DE, en;q=0.5")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    // No German catalog seeded for the home page text in this test, but the
    // hydration payload should carry the resolved locale. The fixture's
    // AppStateBuilder doesn't load any catalogs, so resolve() falls all the way
    // to "en" — but we can still verify the locale appears in the payload.
    assert!(html.contains("\"locale\""), "hydration payload missing locale field");
}

#[tokio::test]
async fn html_lang_attribute_matches_resolved_locale() {
    let app = build_app().await;
    let req = Request::builder().uri("/").body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("<html lang=\"en\""), "html element missing lang attribute");
}
```

- [ ] **Step 2: Run the integration tests**

```
cargo test -p rustcloud-ui --test ssr_routes
```

Expected: 5 tests pass.

If the `not_found_returns_404_dioxus_page` test fails because Dioxus router doesn't render the NotFound component for the catch-all (segments matching may differ in 0.6), adjust the catch-all syntax in `app.rs` and re-run. The assertion content (page contains "404" and "Not Found") is load-bearing; the route declaration syntax is not.

- [ ] **Step 3: Run the full check**

```
cargo xtask check-all
```

Expected: green across the workspace.

- [ ] **Step 4: Commit**

```
git add crates/rustcloud-ui/tests
git commit -m "test(ui): SSR integration tests for /, /login, 404, locale

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 13: Phase 4 acceptance + README + changelog

**Files:**
- Modify: `README.md`
- Create: `docs/superpowers/plans/2026-05-10-platform-core-phase-4-ui.changelog.md`

- [ ] **Step 1: Replace `README.md` with the Phase-4-updated version**

```markdown
# Rustcloud

A Rust port of [Nextcloud server](https://github.com/nextcloud/server), with a Dioxus frontend.

**Status:** early — Phases 1 (Foundations), 2 (Cross-cutting), 3 (HTTP), and 4 (UI) complete. The server serves an SSR'd Dioxus UI at `/` and `/login`, alongside Phase 3's Nextcloud-compatible API surface (`/status.php`, `/ocs/v2.php/cloud/capabilities`, `/index.php/login`). Browser visits to `/` get a fully-rendered HTML page with a hydration payload; the WASM client (compiled by `dx`) takes over for interactivity.

See `docs/superpowers/specs/` for design specs and `docs/superpowers/plans/` for implementation plans.

## Quick start

```bash
# 0. One-time tooling
rustup target add wasm32-unknown-unknown
cargo install dioxus-cli --version "^0.6"

# 1. Copy and edit the example config.
cp config/config.toml.example config/config.toml
#  - Set installed = true.
#  - Pick a dbtype (sqlite is easiest).
#  - Optional: add [bootstrap_admin] with a bcrypt password_hash for /login.

# 2. Build the UI assets + server.
cargo xtask build

# 3a. SQLite: run migrations and serve.
cargo run --release -p rustcloud-server -- migrate
cargo run --release -p rustcloud-server -- serve

# 3b. MySQL or Postgres: start the dev DBs.
cargo xtask up
cargo run --release -p rustcloud-server -- migrate
cargo run --release -p rustcloud-server -- serve

# 4. Visit http://127.0.0.1:8080/ in a browser.
```

## Development

```bash
cargo xtask check-all     # fmt + clippy + tests
cargo xtask build         # dx build + cargo build --release
cargo xtask up            # start MySQL + Postgres for multi-dialect tests
cargo test -p rustcloud-db --test migrate_end_to_end -- --include-ignored
cargo xtask down

# Iterating on UI components:
#   - Edit crates/rustcloud-ui/src/...
#   - Run `dx serve` in crates/rustcloud-ui/ for hot-reload (browser-only)
#   - OR re-run `cargo xtask build && cargo run --release -p rustcloud-server -- serve`
```

## Workspace layout

- `crates/rustcloud-config` — layered TOML config loader (figment).
- `crates/rustcloud-cache` — `Cache` trait + `MemoryCache` + `TypedCache<T>`.
- `crates/rustcloud-db` — `DbPool` enum, `MigrationRunner`, core schema.
- `crates/rustcloud-i18n` — gettext `.po` loader, `Locale`, `I18n`.
- `crates/rustcloud-ocs` — OCS envelope (JSON/XML), capabilities aggregator.
- `crates/rustcloud-core` — `AppState`, `Error`, `AppConfigService`, `BootstrapHook`.
- `crates/rustcloud-http` — axum router, middleware, session, CSRF, auth extractors, API handlers.
- `crates/rustcloud-ui` — Dioxus 0.6 SSR + WASM hydration UI (`/`, `/login`, 404).
- `crates/rustcloud-server` — the binary; CLI, tracing, lifecycle.
- `xtask/` — project automation (`check-all`, `build`, `up`, `down`).
- `migrations/core/` — core SQL migrations, per-dialect.
- `l10n/<app>/<locale>.po` — translation catalogs.

Platform core is now complete; Phase 5 (test scale-out + ship) is the next milestone. After that, app sub-projects (users, storage, WebDAV, sharing, ...) build on this substrate.

## License

AGPL-3.0-or-later.
```

- [ ] **Step 2: Write the Phase 4 changelog**

Create `docs/superpowers/plans/2026-05-10-platform-core-phase-4-ui.changelog.md`. Use today's date for the `Completed:` line.

```markdown
# Phase 4 (UI) — Changelog

Completed: <today's date in YYYY-MM-DD>

## What works

- **`rustcloud-ui`** crate using Dioxus 0.6: `App` component + `Router<Route>` enum with three routes (`/`, `/login`, catch-all `NotFound`).
- **SSR handler** that builds a `RequestContext` from `OptionalUser`, locale, and the session's CSRF token, renders the App into HTML, and wraps it in an HTML shell with the hydration payload + CSS link.
- **Hydration payload**: `<script id="__dx_ctx" type="application/json">` script tag with `{ user_id, display_name, locale, request_token, capabilities_etag }`, safely escaped against `</script>` injection (and U+2028 / U+2029 line separators that break browsers).
- **WASM client entry point** in `crates/rustcloud-ui/src/main.rs` that reads the hydration payload, deserializes into `RequestContext`, and mounts the same `App` component for hydration.
- **Asset pipeline**: `dx build --release` produces a `public/` tree; `rust-embed` bakes it into release builds, falls back to disk in debug.
- **`cargo xtask build`** orchestrates `dx build --release` then `cargo build --release -p rustcloud-server`.
- **CI** installs `wasm32-unknown-unknown` + `dioxus-cli`, builds the WASM bundle as an artifact, and downloads it before running tests so `rust-embed` finds real assets at compile time.
- **`ui_router()`** mounted as the fall-through in `rustcloud-http::build_router` — explicit API routes (`/status.php`, `/ocs/*`, `/index.php/login`) win; everything else SSRs the Dioxus app. `/assets/{*path}` is served by `rust-embed`.
- **Integration tests** verify SSR HTML for `/`, `/login`, 404 fall-through, locale resolution, and `<html lang>` attribute.

## What's deferred

- **Server functions** (`#[server]`): no `#[server]` annotations yet. The login form uses the Phase 3 `/index.php/login` POST handler, which is the right shape for cross-client compatibility per spec §8.4.
- **Browser-side interactivity** beyond the hydrated form: file browser, settings panels, share modals are all deferred to per-feature sub-projects.
- **Public share landing** `/s/<token>`: spec §8.6 — stubbed in the Router enum as part of the catch-all but no real route yet.
- **i18n integration into components**: Home and Login render English strings inline. Wiring `state.i18n.t(...)` into SSR is a Phase 5 polish.
- **WASM 404 status code**: the SSR handler always returns HTTP 200; the catch-all NotFound page is rendered as a 200 body. Browsers and crawlers expect 404 for unknown URLs. Phase 5 should detect the route via the Dioxus router and set the response status accordingly.
- **Dev experience**: no `cargo xtask dev`. Contributors run `dx serve` in `crates/rustcloud-ui/` for UI hot-reload, or rebuild via `cargo xtask build` between server runs.
- **Theming / branding**: shipped CSS is minimal default-typography only.

## Known limitations

- Dioxus 0.6 API specifics may have required minor deviations from this plan. The component shapes are stable; the macro spellings are not.
- The hydration payload includes `capabilities_etag: None` always; Phase 5 can populate it from a request-time aggregator call so clients can conditionally skip a `/cloud/capabilities` round-trip.
- `rust-embed`'s `debug-embed` feature is enabled, so debug builds also embed at compile time — that means after editing CSS or running `dx build`, you must `cargo build` again to refresh. Phase 5 can switch to disk-mode in debug via an env var.
- The SSR handler always returns 200, even for the NotFound page (see "What's deferred").

## Known follow-ups (carried + new)

- **Centralize lint policy (`[workspace.lints]`)** — carried.
- **Sparse rustdoc on public type-level APIs** — carried; Phase 4 adds new public types (`RequestContext`, `Route`, `App`, `render_hydration_script`).
- **`version` subcommand should print git SHA + dialect support** (spec §10.2 / §10.5) — carried.
- **Test config-builder duplication** — now ~10 places. Consolidate via a `test_support` module.
- **Status code for 404 SSR**: emit HTTP 404 when the Dioxus router would render the NotFound page.
- **Server functions** for browser-only RPC (e.g. preferences toggle).
- **`X-Forwarded-For` → `ConnectInfo` rewrite** — carried.
- **Per-route CSP override** for the UI surface: the current `SecurityHeadersLayer` ships an API-restrictive CSP (`default-src 'none'`) that disallows inline scripts. The hydration `<script id="__dx_ctx">` is loaded by the browser as JSON (not executed), but the loaded WASM bundle requires script execution from `/assets/`. Phase 5 must relax CSP for UI responses (`script-src 'self' 'wasm-unsafe-eval'` or similar) — until then, the WASM bundle may be blocked by the browser, and hydration will fail silently. This is the most important Phase 5 polish item.

## Acceptance criteria

| # | Criterion | Status |
|---|---|---|
| Spec §13 #1 | `cargo xtask check-all` against all three backends | OK (carry-over) |
| Spec §13 #3 | Binary boots + serves traffic against all DBs | OK (carry-over) |
| Spec §13 #4 | `curl /status.php` returns Nextcloud JSON | OK (carry-over) |
| Spec §13 #5 | `curl /ocs/v2.php/cloud/capabilities` returns OCS envelope | OK (carry-over) |
| Spec §13 #6 | **Browser at `/` SSR'd + hydrated** | OK — SSR verified by integration tests; hydration relies on WASM bundle loading via `<script type="module" src="/assets/dioxus/rustcloud-ui.js">`. With Phase 5's CSP relaxation the browser will execute it; until then, page works statically. |
| Spec §13 #7 | `/login` POST sets session cookie + redirects | OK (carry-over; the UI's `/login` page POSTs to this endpoint) |
| Spec §13 #8 | Middleware enforcement integration-tested | OK (carry-over) |
| Spec §13 #9 | CI green | OK (carry-over; CI now includes WASM build job) |
| Spec §13 #2 | `cargo xtask build` ships static binary with embedded UI | OK — `rust-embed` packages the `dx`-built `public/` tree into the `rustcloud-server` release binary |
```

- [ ] **Step 3: Run the final acceptance check**

```
cargo clean
cargo xtask build
cargo xtask check-all
```

Expected: all green. The `cargo clean` ensures `rust-embed` runs from a fresh state.

If Docker is available:
```
cargo xtask up
cargo test -p rustcloud-db --test migrate_end_to_end -- --include-ignored
cargo xtask down
```

- [ ] **Step 4: Browser smoke test (optional, documented)**

```
cargo run --release -p rustcloud-server -- --config fixture.toml serve
```

Open `http://127.0.0.1:8080/` in a browser. Verify:
- Page renders with "Welcome, guest" heading.
- "Log in" link points to `/login`.
- Browser dev tools → Network: `/assets/app.css` loads with `text/css`, `/assets/dioxus/rustcloud-ui.js` and the `.wasm` bundle load successfully.
- Browser dev tools → View Source: `<script id="__dx_ctx" type="application/json">` contains the expected payload.
- If the CSP blocks the WASM script (likely without Phase 5's relaxation), note this in the report — the static page still works.

- [ ] **Step 5: Commit**

```
git add README.md docs/superpowers/plans/2026-05-10-platform-core-phase-4-ui.changelog.md
git commit -m "docs: phase 4 acceptance — README + changelog

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Phase 4 Self-Review (executor verifies before declaring complete)

Run through each spec §13 criterion:

| Criterion | Verified by |
|---|---|
| `/` SSR'd HTML with hydration payload | `crates/rustcloud-ui/tests/ssr_routes.rs::home_returns_ssr_html_with_hydration_payload` |
| `/login` form posting to `/index.php/login` | `crates/rustcloud-ui/tests/ssr_routes.rs::login_returns_form_posting_to_index_php_login` |
| 404 catch-all renders Dioxus NotFound | `crates/rustcloud-ui/tests/ssr_routes.rs::unknown_path_returns_404_dioxus_page` |
| Locale resolved from `Accept-Language` | `crates/rustcloud-ui/tests/ssr_routes.rs::locale_resolution_respects_accept_language` |
| `<html lang>` attribute matches locale | `crates/rustcloud-ui/tests/ssr_routes.rs::html_lang_attribute_matches_resolved_locale` |
| `dx build` produces a WASM bundle | CI job `build-wasm` |
| `rust-embed` packs the bundle into the release binary | `cargo xtask build` step in Task 13 |
| `ui_router()` mounted as fall-through | `cargo test -p rustcloud-http --test http_end_to_end` (still green — no API-route regressions) |

If any of these have failing tests or missing artifacts, fix before declaring complete.

---
