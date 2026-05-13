# Files Web UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a Dioxus-rendered `/apps/files/` web UI that lets a signed-in user browse, read, and write their home storage — feature parity with desktop/iOS/Android clients for the everyday workflow (no sharing/favorites/trash yet).

**Architecture:** Dioxus 0.7 Fullstack page mounted at `/apps/files/<path>`. Catch-all route. SSR checks the session and redirects anonymous users to `/index.php/login?redirect_url=…`. Metadata operations (list, mkdir, rename, delete, move) go through new `#[server]` functions that call `AppState::view_for(uid)`. Downloads use a plain `<a href>` to the existing `/dav/files/<user>/<path>` WebDAV GET. Uploads use the existing `/dav/uploads/<user>/<id>/…` chunked endpoints via `fetch()`. File-list state is a `use_resource` keyed on a path signal + a refresh trigger that mutations bump.

**Tech Stack:** Rust 1.95, Dioxus 0.7 (Fullstack: SSR + WASM hydrate + server fns), axum, sqlx, tokio. Existing crates: `crabcloud-ui`, `crabcloud-core`, `crabcloud-fs`, `crabcloud-http`. E2E via Playwright in `e2e/`.

---

## Conventions for every batch

- **Branching:** Each batch is a separate PR off `origin/master`. At the start of each batch:
  ```bash
  cd "C:/Users/Matt Stone/git/rustcloud"
  git fetch origin master
  git switch -c sp6/<batch-letter>-<slug> origin/master
  ```
  Example slugs: `a-chrome`, `b-browse`, `c-mutations`, `d-clipboard`, `e-uploads`, `f-tests-polish`.
- **Commit cadence:** Commit at every "Commit" step. Frequent, focused commits.
- **Pre-PR check:** Before opening the PR, run:
  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```
  All three must pass locally.
- **Open the PR:**
  ```bash
  git push -u origin sp6/<batch-letter>-<slug>
  gh pr create --title "sp6: batch <X> — <topic>" --body "$(cat <<'EOF'
  ## Summary
  - <one-line bullets>

  ## Test plan
  - [ ] CI green (fmt-and-clippy, build-wasm, test-sqlite, test-multidialect, e2e)
  - [ ] <batch-specific manual checks>

  🤖 Generated with [Claude Code](https://claude.com/claude-code)
  EOF
  )"
  ```
- **Merge:** Repo auto-merge is disabled. After the 5 non-cosmetic checks (`fmt-and-clippy`, `build-wasm`, `test-sqlite`, `test-multidialect`, `e2e`) all pass:
  ```bash
  gh pr merge --squash --delete-branch
  ```
- **Established workaround:** Whenever a test builds `AppState`, set `cfg.filecache.enabled = false` before `AppStateBuilder::new(cfg).build()` to avoid the scanner-race that's hit other batches. See `crates/crabcloud-http/tests/dav_basic.rs:16-37` for the pattern.
- **Pre-existing patterns to mirror:**
  - **Server function shape**: `crates/crabcloud-ui/src/server_fns.rs` (existing `status` + `login`).
  - **Page component shape**: `crates/crabcloud-ui/src/pages/home.rs` and `pages/login.rs`.
  - **Test shape**: `crates/crabcloud-http/tests/dav_basic.rs` (real `build_router` + bearer token + `oneshot`).
  - **Playwright shape**: `e2e/tests/webdav.spec.ts` (cookie login + `request.fetch` for raw HTTP; for browser tests, `page.goto` + locator assertions like `hydration.spec.ts`).

---

## File-by-file map

New files (created across the six batches):

```
crates/crabcloud-ui/src/pages/files/
├── mod.rs              — FilesRoute component + state context + page layout
├── path.rs             — Segments ↔ UserPath helpers (+ unit tests)
├── chrome.rs           — TopBar, Sidebar wrappers
├── toolbar.rs          — New / Upload buttons + SelectionChip + ClipboardChip
├── breadcrumb.rs       — Breadcrumb component
├── list.rs             — FileList: header row + body + skeleton + empty + error
├── row.rs              — FileRow (checkbox, icon, name/RenameInput, size, mtime, ⋯ menu)
├── mkdir_row.rs        — Inline "New folder" row
├── delete_modal.rs     — Centered modal, single-or-many
├── upload.rs           — DropZone overlay + Upload button + chunked uploader state machine
├── progress_strip.rs   — UploadProgressStrip
├── states.rs           — EmptyFolder / LoadError / Skeleton fragments
├── icons.rs            — Tiny inline SVG icons (folder, file, ⋯)
├── strings.rs          — User-visible strings (i18n anchor)
└── ssr.rs              — Server-only redirect helper (cfg(feature = "server"))

crates/crabcloud-ui/src/server_fns/
├── mod.rs              — re-exports (existing top-level server_fns.rs becomes this dir)
├── status.rs           — moved from server_fns.rs
├── login.rs            — moved from server_fns.rs
└── files.rs            — new: list_dir, mkdir, rename, delete, move_paths, upload_begin

crates/crabcloud-ui/tests/server_fns_files.rs — integration tests (HTTP-level)
e2e/tests/files.spec.ts                       — Playwright e2e
docs/superpowers/specs/2026-05-12-files-web-ui-design.followup-sp7.md — open notes for SP7
```

Files modified:
- `crates/crabcloud-ui/src/lib.rs` (re-export `FilesRoute`, new module wiring)
- `crates/crabcloud-ui/src/app.rs` (add the catch-all `Route::FilesRoute`)
- `crates/crabcloud-ui/src/pages/mod.rs` (re-export `files`)
- `crates/crabcloud-ui/Cargo.toml` (add `base64`, `url`, `futures-util` for client-side use; `axum` already optional)
- `crates/crabcloud-ui/assets/app.css` (small additions for files-page styles)
- `README.md` (workspace bullet)
- `docs/superpowers/specs/2026-05-12-files-web-ui-design.md` (followup-sp7 link)
- `memory/project_rustcloud_program.md` (SP6 status flip)

---

## Batch A — Routing & chrome

**Branch:** `sp6/a-chrome` off `origin/master`.
**Goal:** Hitting `/apps/files/...` (and any sub-path) renders the page shell: top bar, left sidebar, breadcrumb, an "empty placeholder" list region. Anonymous users get redirected to `/index.php/login?redirect_url=...`. No real data yet — the list region is a hardcoded "this folder is empty" fragment.

### Task A1: Segments-to-UserPath helpers

**Files:**
- Create: `crates/crabcloud-ui/src/pages/files/path.rs`
- Modify: `crates/crabcloud-ui/Cargo.toml` (add `crabcloud-fs` to optional deps and the `server` feature; we use `UserPath` server-side and as a plain `String` on the client)

- [ ] **Step 1: Add `crabcloud-fs` to the `crabcloud-ui` `server` feature**

In `crates/crabcloud-ui/Cargo.toml`, under `[dependencies]`:
```toml
crabcloud-fs = { workspace = true, optional = true }
```
And under `[features].server`, append `"dep:crabcloud-fs"` to the list.

- [ ] **Step 2: Write failing unit tests**

Create `crates/crabcloud-ui/src/pages/files/path.rs` with the test module first:
```rust
//! Helpers for the Files page's URL routing. The browser hits
//! `/apps/files/<segments...>`; this module converts between the captured
//! `Vec<String>` and a normalized absolute path string (`"/"`, `"/photos"`,
//! `"/photos/vacation"`). Path validation against `..`/`.`/etc. is left to
//! `UserPath::new` on the server.

/// Join captured route segments into a normalized absolute path. An empty
/// `segments` slice yields `"/"`.
pub fn segments_to_path(segments: &[String]) -> String {
    if segments.is_empty() {
        return "/".to_string();
    }
    let mut out = String::with_capacity(segments.iter().map(|s| s.len() + 1).sum());
    for seg in segments {
        out.push('/');
        out.push_str(seg);
    }
    out
}

/// Split an absolute path into its non-empty segments. `"/"` yields `[]`.
pub fn path_to_segments(path: &str) -> Vec<String> {
    path.trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_segments_yield_root() {
        assert_eq!(segments_to_path(&[]), "/");
    }

    #[test]
    fn single_segment_prefixed() {
        assert_eq!(segments_to_path(&["photos".to_string()]), "/photos");
    }

    #[test]
    fn multiple_segments_joined() {
        let s = vec!["photos".to_string(), "vacation".to_string()];
        assert_eq!(segments_to_path(&s), "/photos/vacation");
    }

    #[test]
    fn root_yields_empty_segments() {
        assert!(path_to_segments("/").is_empty());
    }

    #[test]
    fn path_segments_roundtrip() {
        let s = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert_eq!(path_to_segments(&segments_to_path(&s)), s);
    }

    #[test]
    fn path_to_segments_strips_empty() {
        assert_eq!(
            path_to_segments("//a///b/"),
            vec!["a".to_string(), "b".to_string()]
        );
    }
}
```

- [ ] **Step 3: Wire up the new module**

Create `crates/crabcloud-ui/src/pages/files/mod.rs` with just:
```rust
//! Files web UI — `/apps/files/<path>`. The browser-facing app for browsing,
//! reading, and writing the user's home storage. See
//! `docs/superpowers/specs/2026-05-12-files-web-ui-design.md`.

pub mod path;
```

Modify `crates/crabcloud-ui/src/pages/mod.rs` (or wherever pages are re-exported — check the file first). The existing `lib.rs` declares `pub mod pages;`. If there's no `pages/mod.rs`, the modules are declared inline in `pages.rs`. Update accordingly. For this plan we assume `pub mod files;` lives next to the others. Inspect with `Read` first if uncertain.

- [ ] **Step 4: Run tests**

```
cargo test -p crabcloud-ui pages::files::path::tests
```
Expected: 6 passing.

- [ ] **Step 5: Commit**

```
git add crates/crabcloud-ui/Cargo.toml crates/crabcloud-ui/src/pages/files/
git commit -m "sp6(a): add segments↔path helpers for the files page"
```

### Task A2: Add the `FilesRoute` catch-all

**Files:**
- Modify: `crates/crabcloud-ui/src/app.rs`
- Modify: `crates/crabcloud-ui/src/pages/files/mod.rs`

- [ ] **Step 1: Define a placeholder `Files` component**

In `crates/crabcloud-ui/src/pages/files/mod.rs`, append:
```rust
use crate::context::RequestContext;
use dioxus::prelude::*;

/// Files page entry point. For Batch A this renders a placeholder list so
/// the route is wired end-to-end; Batch B replaces the body with real data.
#[component]
pub fn Files(ctx: RequestContext, path: String) -> Element {
    let _ = (ctx, path);
    rsx! {
        main { class: "files-page",
            p { "Files (placeholder — batch A)" }
        }
    }
}
```

- [ ] **Step 2: Add the route**

In `crates/crabcloud-ui/src/app.rs`, inside the `Route` enum, ABOVE the existing `NotFoundRoute` (route order matters — the catch-all must come last):
```rust
    /// Files browser. Catch-all so paths like `/apps/files/photos/vacation`
    /// route here and the page renders the folder identified by `segments`.
    #[route("/apps/files/:..segments")]
    FilesRoute { segments: Vec<String> },
```

And add the route handler component:
```rust
#[component]
pub fn FilesRoute(segments: Vec<String>) -> Element {
    use crate::pages::files::{path::segments_to_path, Files};
    let ctx = use_context::<RequestContext>();
    let path = segments_to_path(&segments);
    rsx! { Files { ctx, path } }
}
```

Update the existing `use crate::pages::{...}` line to also pull `files` in (the route handler references `Files` via the explicit `use` inside, so only the route enum needs the new module to be discoverable — `pages` module is already public).

- [ ] **Step 3: Build the WASM bundle to confirm route compiles for the browser**

```
cd "C:/Users/Matt Stone/git/rustcloud"
cargo check -p crabcloud-ui --target wasm32-unknown-unknown --features web
```
Expected: clean build (warnings about unused vars are OK at this stage).

- [ ] **Step 4: Build the server side**

```
cargo check -p crabcloud-ui --features server
```
Expected: clean build.

- [ ] **Step 5: Commit**

```
git add crates/crabcloud-ui/src/app.rs crates/crabcloud-ui/src/pages/files/mod.rs
git commit -m "sp6(a): add FilesRoute catch-all + placeholder page"
```

### Task A3: SSR auth redirect helper

**Files:**
- Create: `crates/crabcloud-ui/src/pages/files/ssr.rs`
- Modify: `crates/crabcloud-ui/src/pages/files/mod.rs`

- [ ] **Step 1: Create the redirect helper**

`crates/crabcloud-ui/src/pages/files/ssr.rs`:
```rust
//! Server-only helpers for the Files page. The SSR branch checks the
//! session and, if absent, commits a 303 + Location header so the browser
//! redirects to the login page before the page body is sent.

#![cfg(feature = "server")]

use dioxus::fullstack::FullstackContext;

/// If the current request is unauthenticated, commit a 303 redirect to
/// `/index.php/login?redirect_url=<encoded current path>` and return
/// `true` so the caller can short-circuit page rendering.
///
/// `current_path` is the user-facing absolute path the user requested,
/// e.g. `/apps/files/photos/vacation`.
pub fn redirect_if_anonymous(user_id: &Option<String>, current_path: &str) -> bool {
    if user_id.is_some() {
        return false;
    }
    let Some(fs) = FullstackContext::current() else {
        return false;
    };
    let encoded = url_encode(current_path);
    let location = format!("/index.php/login?redirect_url={encoded}");
    fs.commit_http_status(
        axum::http::StatusCode::SEE_OTHER,
        Some(("location", &location)),
    );
    true
}

/// URL-encode using application/x-www-form-urlencoded rules. We avoid the
/// `url` crate dep for one helper — the input is a path we built ourselves
/// (no NULs, no `?`, no `#`).
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_encode_passthrough_safe() {
        assert_eq!(url_encode("/apps/files/photos"), "/apps/files/photos");
    }

    #[test]
    fn url_encode_escapes_space_and_question() {
        assert_eq!(url_encode("/a b?c"), "/a%20b%3Fc");
    }

    #[test]
    fn url_encode_escapes_unicode() {
        // Multibyte → multiple %XX
        assert_eq!(url_encode("/é"), "/%C3%A9");
    }
}
```

- [ ] **Step 2: Add `axum` to the test dependencies (it's already optional on `server`; cfg-gated tests see it)**

No Cargo change needed — the `#![cfg(feature = "server")]` on the module means the test runs only with `--features server`.

- [ ] **Step 3: Re-export the helper from the page module**

In `crates/crabcloud-ui/src/pages/files/mod.rs`, append:
```rust
#[cfg(feature = "server")]
pub mod ssr;
```

- [ ] **Step 4: Run tests**

```
cargo test -p crabcloud-ui --features server pages::files::ssr::tests
```
Expected: 3 passing.

- [ ] **Step 5: Commit**

```
git add crates/crabcloud-ui/src/pages/files/
git commit -m "sp6(a): SSR redirect helper for anonymous /apps/files/ visitors"
```

### Task A4: Top-bar + sidebar chrome

**Files:**
- Create: `crates/crabcloud-ui/src/pages/files/chrome.rs`
- Modify: `crates/crabcloud-ui/src/pages/files/mod.rs`
- Modify: `crates/crabcloud-ui/assets/app.css`

- [ ] **Step 1: Write the chrome components**

`crates/crabcloud-ui/src/pages/files/chrome.rs`:
```rust
//! Page chrome: top bar (logo, app name, user chip) + left sidebar
//! ("All files" only for MVP). See spec §2 (decision 7).

use crate::context::RequestContext;
use dioxus::prelude::*;

#[component]
pub fn TopBar(ctx: RequestContext) -> Element {
    let display = ctx
        .display_name
        .clone()
        .unwrap_or_else(|| "User".to_string());
    let initial = display.chars().next().unwrap_or('U').to_uppercase().to_string();
    rsx! {
        header { class: "topbar",
            a { class: "topbar-brand", href: "/", "Crabcloud" }
            nav { class: "topbar-nav",
                a { class: "topbar-link active", href: "/apps/files/", "Files" }
            }
            div { class: "topbar-spacer" }
            div { class: "topbar-user", title: "{display}", "{initial}" }
        }
    }
}

#[component]
pub fn Sidebar() -> Element {
    rsx! {
        aside { class: "sidebar",
            ul { class: "sidebar-list",
                li { class: "sidebar-item active",
                    span { class: "sidebar-icon", "📂" }
                    span { "All files" }
                }
            }
        }
    }
}
```

- [ ] **Step 2: Add CSS**

Append to `crates/crabcloud-ui/assets/app.css`:
```css
/* ---- Files page chrome ---- */
.files-page { display: grid; grid-template-rows: auto 1fr; height: 100vh; }
.topbar {
  display: flex; align-items: center; gap: 16px;
  padding: 8px 16px; background: #0082c9; color: white;
  font-size: 14px;
}
.topbar-brand { color: white; font-weight: 600; text-decoration: none; }
.topbar-nav { display: flex; gap: 12px; }
.topbar-link { color: white; text-decoration: none; opacity: 0.85; }
.topbar-link.active { opacity: 1; font-weight: 600; }
.topbar-spacer { flex: 1; }
.topbar-user {
  width: 28px; height: 28px; border-radius: 50%;
  background: rgba(255,255,255,0.2);
  display: inline-flex; align-items: center; justify-content: center;
}
.files-body { display: grid; grid-template-columns: 200px 1fr; min-height: 0; }
.sidebar { background: #fafafa; border-right: 1px solid #eee; padding: 16px 8px; font-size: 13px; }
.sidebar-list { list-style: none; padding: 0; margin: 0; }
.sidebar-item { display: flex; align-items: center; gap: 8px; padding: 8px; border-radius: 4px; }
.sidebar-item.active { background: #e8f3fb; color: #0082c9; }
.sidebar-icon { width: 16px; text-align: center; }
.files-main { padding: 16px 24px; overflow: auto; }
```

- [ ] **Step 3: Re-export**

In `crates/crabcloud-ui/src/pages/files/mod.rs`, append:
```rust
pub mod chrome;
```

- [ ] **Step 4: Build**

```
cargo check -p crabcloud-ui --features server
cargo check -p crabcloud-ui --target wasm32-unknown-unknown --features web
```
Expected: clean.

- [ ] **Step 5: Commit**

```
git add crates/crabcloud-ui/src/pages/files/chrome.rs crates/crabcloud-ui/src/pages/files/mod.rs crates/crabcloud-ui/assets/app.css
git commit -m "sp6(a): top-bar + sidebar chrome for files page"
```

### Task A5: Empty/loading/error/skeleton fragments

**Files:**
- Create: `crates/crabcloud-ui/src/pages/files/states.rs`
- Modify: `crates/crabcloud-ui/src/pages/files/mod.rs`
- Modify: `crates/crabcloud-ui/assets/app.css`

- [ ] **Step 1: Write the state fragments**

`crates/crabcloud-ui/src/pages/files/states.rs`:
```rust
//! Empty / loading / error states for the Files page. See spec §13.

use dioxus::prelude::*;

#[component]
pub fn EmptyFolder() -> Element {
    rsx! {
        div { class: "files-empty",
            div { class: "files-empty-icon", "📂" }
            div { class: "files-empty-title", "This folder is empty" }
            div { class: "files-empty-sub", "Drop files here, or click ", strong { "Upload" }, " above." }
        }
    }
}

#[component]
pub fn LoadError(reason: String, on_retry: EventHandler<()>) -> Element {
    rsx! {
        div { class: "files-error",
            div { class: "files-error-icon", "⚠️" }
            div { class: "files-error-title", "Couldn't load this folder" }
            if !reason.is_empty() {
                div { class: "files-error-sub", "{reason}" }
            }
            button {
                class: "files-error-retry",
                onclick: move |_| on_retry.call(()),
                "Retry"
            }
        }
    }
}

#[component]
pub fn Skeleton() -> Element {
    rsx! {
        div { class: "files-skeleton",
            for _ in 0..4 {
                div { class: "files-skeleton-row",
                    span { class: "files-skel-cell files-skel-check" }
                    span { class: "files-skel-cell files-skel-name" }
                    span { class: "files-skel-cell files-skel-size" }
                    span { class: "files-skel-cell files-skel-mtime" }
                }
            }
        }
    }
}
```

- [ ] **Step 2: Add CSS**

Append to `crates/crabcloud-ui/assets/app.css`:
```css
.files-empty, .files-error {
  display: flex; flex-direction: column; align-items: center; justify-content: center;
  padding: 48px 16px; color: #666;
}
.files-empty-icon, .files-error-icon { font-size: 40px; opacity: 0.6; }
.files-empty-title, .files-error-title { margin-top: 8px; font-weight: 600; color: #333; }
.files-empty-sub, .files-error-sub { margin-top: 4px; font-size: 13px; }
.files-error-retry { margin-top: 14px; padding: 6px 14px; border: 1px solid #0082c9; background: white; color: #0082c9; border-radius: 4px; cursor: pointer; }
.files-skeleton { padding: 8px 0; }
.files-skeleton-row { display: grid; grid-template-columns: 32px 1fr 80px 120px; gap: 8px; padding: 10px 4px; border-bottom: 1px solid #f5f5f5; }
.files-skel-cell { height: 12px; background: #eee; border-radius: 2px; }
.files-skel-check { width: 14px; }
```

- [ ] **Step 3: Re-export**

In `crates/crabcloud-ui/src/pages/files/mod.rs`, append:
```rust
pub mod states;
```

- [ ] **Step 4: Build**

```
cargo check -p crabcloud-ui --features server
cargo check -p crabcloud-ui --target wasm32-unknown-unknown --features web
```
Expected: clean.

- [ ] **Step 5: Commit**

```
git add crates/crabcloud-ui/src/pages/files/states.rs crates/crabcloud-ui/src/pages/files/mod.rs crates/crabcloud-ui/assets/app.css
git commit -m "sp6(a): empty/loading/error fragments"
```

### Task A6: Wire the page end-to-end

**Files:**
- Modify: `crates/crabcloud-ui/src/pages/files/mod.rs`

- [ ] **Step 1: Replace the placeholder `Files` component**

Replace the `Files` component in `crates/crabcloud-ui/src/pages/files/mod.rs` with:
```rust
use crate::context::RequestContext;
use dioxus::prelude::*;

pub mod chrome;
pub mod path;
pub mod states;
#[cfg(feature = "server")]
pub mod ssr;

#[component]
pub fn Files(ctx: RequestContext, path: String) -> Element {
    // SSR-only: redirect anonymous visitors to login with redirect_url.
    #[cfg(feature = "server")]
    {
        let current_path = format!(
            "/apps/files{}",
            if path == "/" { String::new() } else { path.clone() }
        );
        if ssr::redirect_if_anonymous(&ctx.user_id, &current_path) {
            return rsx! { "" };
        }
    }

    let _ = path;
    rsx! {
        div { class: "files-page",
            chrome::TopBar { ctx: ctx.clone() }
            div { class: "files-body",
                chrome::Sidebar {}
                main { class: "files-main",
                    states::EmptyFolder {}
                }
            }
        }
    }
}
```

- [ ] **Step 2: Build server + WASM**

```
cargo check -p crabcloud-ui --features server
cargo check -p crabcloud-ui --target wasm32-unknown-unknown --features web
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```
Expected: clean.

- [ ] **Step 3: Run the full workspace test suite**

```
cargo test --workspace
```
Expected: green.

- [ ] **Step 4: Manual SSR smoke (optional but recommended)**

If a dev server is convenient, start it (the existing `dev/` scripts) and hit `http://localhost:18765/apps/files/` while logged out — expect a 303 to `/index.php/login?redirect_url=/apps/files/`. Logged in: expect the chrome to render with the empty folder placeholder.

- [ ] **Step 5: Commit**

```
git add crates/crabcloud-ui/src/pages/files/mod.rs
git commit -m "sp6(a): wire files page chrome + empty state end-to-end"
```

### Task A7: Open the PR, watch CI, merge

- [ ] **Step 1: Push the branch**

```
git push -u origin sp6/a-chrome
```

- [ ] **Step 2: Open the PR**

```
gh pr create --title "sp6: batch A — routing & chrome" --body "$(cat <<'EOF'
## Summary
- Adds the `/apps/files/:..segments` catch-all route.
- Renders top bar + left sidebar + empty-state placeholder.
- SSR redirects anonymous users to `/index.php/login?redirect_url=/apps/files/...`.

## Test plan
- [ ] CI green (fmt-and-clippy, build-wasm, test-sqlite, test-multidialect, e2e)
- [ ] Logged-out hit of /apps/files/ redirects to login with redirect_url
- [ ] Logged-in hit of /apps/files/ shows chrome + empty-state

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 3: Wait for CI**

```
gh pr checks --watch
```
Expected: 5 non-cosmetic checks green.

- [ ] **Step 4: Merge**

```
gh pr merge --squash --delete-branch
```

---

## Batch B — Browse + download

**Branch:** `sp6/b-browse` off `origin/master`.
**Goal:** The Files page lists the real contents of the user's folder. Folder rows are clickable and update the URL/route; file rows are anchor links to `/dav/files/<user>/<path>`. Skeleton renders while loading. The breadcrumb shows the current path and lets the user navigate up.

### Task B1: `FileEntry` DTO and `list_dir` server fn

**Files:**
- Modify: `crates/crabcloud-ui/src/server_fns.rs`
- Modify: `crates/crabcloud-ui/src/lib.rs` (re-export new items)

- [ ] **Step 1: Add the DTO and the server fn**

Append to `crates/crabcloud-ui/src/server_fns.rs`:
```rust
/// Single entry in a `list_dir` response. Shape is what the UI needs;
/// server-side it's filled from `crabcloud_storage::DirEntry`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileEntry {
    pub name: String,
    /// Full UserPath, e.g. `/photos/cat.jpg`.
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime_ms: i64,
    pub mime: Option<String>,
    pub etag: String,
}

/// `GET /api/files/list?path=...` — list a directory. Returns sorted
/// entries (directories first, then files; alphabetical within each group,
/// case-insensitive).
#[get("/api/files/list")]
pub async fn list_dir(path: String) -> Result<Vec<FileEntry>, ServerFnError> {
    use crabcloud_fs::UserPath;
    use dioxus::fullstack::FullstackContext;

    let fs = FullstackContext::current()
        .ok_or_else(|| ServerFnError::new("not running on server"))?;
    let state = fs
        .extension::<crabcloud_core::AppState>()
        .ok_or_else(|| ServerFnError::new("AppState extension missing"))?;
    let session = fs.extension::<crabcloud_http::SessionHandle>();
    let snapshot = session.and_then(|s| s.try_read_snapshot());
    let uid_str = snapshot
        .as_ref()
        .and_then(|s| s.user_id.clone())
        .ok_or_else(|| ServerFnError::new("unauthorized"))?;
    let uid = crabcloud_users::UserId::new(&uid_str)
        .map_err(|e| ServerFnError::new(format!("invalid uid: {e}")))?;

    let user_path =
        UserPath::new(path).map_err(|e| ServerFnError::new(format!("invalid path: {e}")))?;
    let view = state
        .view_for(&uid)
        .await
        .map_err(|e| ServerFnError::new(format!("view: {e}")))?;
    let raw = view
        .list(&user_path)
        .await
        .map_err(|e| map_fs_err(e))?;

    let mut out: Vec<FileEntry> = raw
        .into_iter()
        .map(|entry| dir_entry_to_dto(&user_path, entry))
        .collect();
    out.sort_by(|a, b| match (b.is_dir, a.is_dir) {
        (true, false) => std::cmp::Ordering::Greater,
        (false, true) => std::cmp::Ordering::Less,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    Ok(out)
}

#[cfg(feature = "server")]
fn dir_entry_to_dto(parent: &crabcloud_fs::UserPath, entry: crabcloud_storage::DirEntry) -> FileEntry {
    use std::time::UNIX_EPOCH;
    let full_path = parent
        .join(&entry.name)
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|_| entry.name.clone());
    let mtime_ms = entry
        .metadata
        .mtime
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let is_dir = matches!(entry.metadata.kind, crabcloud_storage::FileKind::Directory);
    FileEntry {
        name: entry.name,
        path: full_path,
        is_dir,
        size: entry.metadata.size,
        mtime_ms,
        mime: (!is_dir).then(|| entry.metadata.mimetype.as_str().to_string()),
        etag: entry.metadata.etag.as_str().to_string(),
    }
}

#[cfg(feature = "server")]
fn map_fs_err(err: crabcloud_fs::FsError) -> ServerFnError {
    use crabcloud_fs::FsError;
    match err {
        FsError::NotFound => ServerFnError::new("not_found"),
        FsError::InvalidPath(m) => ServerFnError::new(format!("invalid_path: {m}")),
        FsError::CrossMount => ServerFnError::new("cross_mount"),
        FsError::MountNotFound => ServerFnError::new("mount_not_found"),
        FsError::Storage(s) => ServerFnError::new(format!("storage: {s}")),
        FsError::FileCache(c) => ServerFnError::new(format!("filecache: {c}")),
        FsError::Upload(m) => ServerFnError::new(format!("upload: {m}")),
    }
}
```

(`dir_entry_to_dto` and `map_fs_err` are server-only because they reference `crabcloud_fs` and `crabcloud_storage`. The DTO and the `#[get]`-decorated fn itself live in shared code; the server-only body is gated by Dioxus.)

- [ ] **Step 2: Re-export from `lib.rs`**

In `crates/crabcloud-ui/src/lib.rs`, update the existing `pub use server_fns::...` line:
```rust
pub use server_fns::{list_dir, login, status, FileEntry, StatusInfo};
```

- [ ] **Step 3: Build server side**

```
cargo check -p crabcloud-ui --features server
```
Expected: clean. If `crabcloud_storage` isn't already a dependency of `crabcloud-ui`, add it as an optional dep behind the `server` feature (it's reachable via `crabcloud_fs`'s public re-exports too; if so, prefer using `crabcloud_fs::storage::...` or copy the types via `crabcloud_storage::DirEntry` and add the dep).

If the dep is missing, edit `crates/crabcloud-ui/Cargo.toml`:
```toml
crabcloud-storage = { workspace = true, optional = true }
```
And append `"dep:crabcloud-storage"` to `[features].server`.

- [ ] **Step 4: Commit**

```
git add crates/crabcloud-ui/src/server_fns.rs crates/crabcloud-ui/src/lib.rs crates/crabcloud-ui/Cargo.toml
git commit -m "sp6(b): list_dir server fn + FileEntry DTO"
```

### Task B2: Integration test for `list_dir`

**Files:**
- Create: `crates/crabcloud-ui/tests/server_fns_files.rs`

- [ ] **Step 1: Add the test file**

```rust
//! HTTP-level integration tests for the Files server fns. Drives the full
//! `build_router` so requests travel through the real auth + middleware
//! stack. Uses bearer token + `OCS-APIRequest` (matches the test pattern
//! used by `crates/crabcloud-http/tests/dav_*.rs`).

#![allow(unused_crate_dependencies)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_core::{AppState, AppStateBuilder};
use crabcloud_users::{AuthTokenType, BcryptVerifier, PasswordVerifier, User, UserId};
use tempfile::tempdir;
use tower::ServiceExt;

async fn make_state_with_user(db: std::path::PathBuf, data: std::path::PathBuf) -> AppState {
    let mut cfg = minimal_sqlite_config(db);
    cfg.datadirectory = data;
    cfg.filecache.enabled = false; // see Batch A conventions.
    let state = AppStateBuilder::new(cfg).build().await.unwrap();
    let hash = BcryptVerifier::new().hash("hunter2").unwrap();
    state
        .users
        .user_store()
        .create(
            &User {
                uid: UserId::new("alice").unwrap(),
                display_name: "Alice".into(),
                email: None,
                enabled: true,
                last_seen: 0,
            },
            Some(&hash),
        )
        .await
        .unwrap();
    state
}

async fn alice_bearer(state: &AppState) -> String {
    let ap = state.users.app_passwords().unwrap().clone();
    let (_row, raw) = ap
        .mint(
            &UserId::new("alice").unwrap(),
            "alice",
            "UI",
            AuthTokenType::Session,
            false,
        )
        .await
        .unwrap();
    raw.expose().to_string()
}

/// Write a small file via WebDAV PUT so we have content to list.
async fn put_file(app: &axum::Router, token: &str, path: &str, body: &'static str) {
    let req = Request::builder()
        .method("PUT")
        .uri(format!("/dav/files/alice{path}"))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert!(
        resp.status() == StatusCode::CREATED || resp.status() == StatusCode::NO_CONTENT,
        "PUT failed: {}",
        resp.status()
    );
}

#[tokio::test]
async fn list_dir_returns_entries() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("l.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    put_file(&app, &token, "/hello.txt", "hi").await;

    let req = Request::builder()
        .method("GET")
        .uri("/api/files/list?path=%2F")
        .header("authorization", format!("Bearer {token}"))
        .header("ocs-apirequest", "true")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let entries: Vec<crabcloud_ui::FileEntry> = serde_json::from_slice(&body).unwrap();
    assert!(entries.iter().any(|e| e.name == "hello.txt" && !e.is_dir));
}

#[tokio::test]
async fn list_dir_unauthenticated_returns_401() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("u.db"), data.path().to_path_buf()).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("GET")
        .uri("/api/files/list?path=%2F")
        .header("ocs-apirequest", "true")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn list_dir_invalid_path_returns_4xx() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("i.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let req = Request::builder()
        .method("GET")
        .uri("/api/files/list?path=not-absolute")
        .header("authorization", format!("Bearer {token}"))
        .header("ocs-apirequest", "true")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    // Dioxus server fns map errors to 500 by default; we accept any 4xx
    // *or* 500 here since the contract is "non-200 with diagnostic body".
    assert!(resp.status() != StatusCode::OK);
}
```

- [ ] **Step 2: Run tests**

```
cargo test -p crabcloud-ui --test server_fns_files
```
Expected: 3 passing.

If the JSON deserialization fails, inspect the actual response body (Dioxus 0.7's server-fn codec wraps the value — adjust `serde_json::from_slice` to match if needed; expected payload is the bare `Vec<FileEntry>` per Dioxus 0.7 JSON codec).

- [ ] **Step 3: Commit**

```
git add crates/crabcloud-ui/tests/server_fns_files.rs
git commit -m "sp6(b): integration tests for list_dir"
```

### Task B3: `FileRow` and `FileList` components

**Files:**
- Create: `crates/crabcloud-ui/src/pages/files/row.rs`
- Create: `crates/crabcloud-ui/src/pages/files/list.rs`
- Modify: `crates/crabcloud-ui/src/pages/files/mod.rs`
- Modify: `crates/crabcloud-ui/assets/app.css`

- [ ] **Step 1: `FileRow` component**

`crates/crabcloud-ui/src/pages/files/row.rs`:
```rust
//! Single row in the file list. Click on a folder row navigates into it;
//! file rows are anchor links to the WebDAV GET URL so the browser handles
//! the download/inline-view natively. See spec §8.

use crate::server_fns::FileEntry;
use dioxus::prelude::*;

#[component]
pub fn FileRow(
    entry: FileEntry,
    user_id: String,
    on_open_folder: EventHandler<String>,
) -> Element {
    let icon = if entry.is_dir { "📁" } else { "📄" };
    let size = if entry.is_dir {
        "—".to_string()
    } else {
        format_size(entry.size)
    };
    let mtime = format_mtime(entry.mtime_ms);
    let name_cell = if entry.is_dir {
        let path = entry.path.clone();
        rsx! {
            button {
                class: "files-name files-name-folder",
                onclick: move |_| on_open_folder.call(path.clone()),
                span { class: "files-icon", "{icon}" }
                "{entry.name}"
            }
        }
    } else {
        let href = format!("/dav/files/{user_id}{}", entry.path);
        rsx! {
            a {
                class: "files-name files-name-file",
                href: "{href}",
                onclick: move |evt: MouseEvent| evt.stop_propagation(),
                span { class: "files-icon", "{icon}" }
                "{entry.name}"
            }
        }
    };
    rsx! {
        tr { class: "files-row",
            td { class: "files-cell files-check", input { r#type: "checkbox" } }
            td { class: "files-cell", {name_cell} }
            td { class: "files-cell files-size", "{size}" }
            td { class: "files-cell files-mtime", "{mtime}" }
        }
    }
}

fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[0])
    } else {
        format!("{:.1} {}", size, UNITS[unit])
    }
}

fn format_mtime(mtime_ms: i64) -> String {
    // Minimal relative-time formatter. Tests cover the boundaries.
    let now_ms = current_time_ms();
    let delta_secs = ((now_ms - mtime_ms).max(0)) / 1000;
    if delta_secs < 60 { return "just now".into(); }
    if delta_secs < 3_600 { return format!("{} min ago", delta_secs / 60); }
    if delta_secs < 86_400 { return format!("{} hr ago", delta_secs / 3_600); }
    if delta_secs < 7 * 86_400 { return format!("{} days ago", delta_secs / 86_400); }
    if delta_secs < 30 * 86_400 { return format!("{} weeks ago", delta_secs / (7 * 86_400)); }
    format!("{} months ago", delta_secs / (30 * 86_400))
}

#[cfg(target_arch = "wasm32")]
fn current_time_ms() -> i64 {
    js_sys::Date::now() as i64
}

#[cfg(not(target_arch = "wasm32"))]
fn current_time_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_size_bytes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
    }

    #[test]
    fn format_size_kb() {
        assert_eq!(format_size(2048), "2.0 KB");
    }

    #[test]
    fn format_size_mb() {
        assert_eq!(format_size(2 * 1024 * 1024 + 100 * 1024), "2.1 MB");
    }
}
```

Add `js-sys` for the WASM clock:
- In `crates/crabcloud-ui/Cargo.toml` under `[dependencies]`:
  ```toml
  js-sys = { workspace = true }
  ```
  (If `js-sys` is not in the workspace yet, add it to root `Cargo.toml`'s `[workspace.dependencies]` first with the current stable version, e.g., `js-sys = "0.3"`.)

- [ ] **Step 2: `FileList` component**

`crates/crabcloud-ui/src/pages/files/list.rs`:
```rust
//! Tabular list of files in the current folder. Renders skeleton/empty/error
//! states based on the `Resource` state passed in from the page.

use crate::pages::files::row::FileRow;
use crate::pages::files::states::{EmptyFolder, LoadError, Skeleton};
use crate::server_fns::FileEntry;
use dioxus::prelude::*;

#[component]
pub fn FileList(
    entries: Option<Result<Vec<FileEntry>, String>>,
    user_id: String,
    on_open_folder: EventHandler<String>,
    on_retry: EventHandler<()>,
) -> Element {
    match entries {
        None => rsx! { Skeleton {} },
        Some(Err(msg)) => rsx! { LoadError { reason: msg, on_retry } },
        Some(Ok(es)) if es.is_empty() => rsx! { EmptyFolder {} },
        Some(Ok(es)) => rsx! {
            table { class: "files-table",
                thead {
                    tr {
                        th { class: "files-th files-check" }
                        th { class: "files-th", "Name" }
                        th { class: "files-th files-size", "Size" }
                        th { class: "files-th files-mtime", "Modified" }
                    }
                }
                tbody {
                    for e in es {
                        FileRow {
                            entry: e.clone(),
                            user_id: user_id.clone(),
                            on_open_folder,
                        }
                    }
                }
            }
        },
    }
}
```

- [ ] **Step 3: CSS for table + row**

Append to `crates/crabcloud-ui/assets/app.css`:
```css
.files-table { width: 100%; border-collapse: collapse; font-size: 13px; }
.files-th { text-align: left; padding: 8px 4px; color: #666; border-bottom: 1px solid #eee; background: #fafafa; }
.files-th.files-size, .files-cell.files-size { width: 80px; text-align: right; }
.files-th.files-mtime, .files-cell.files-mtime { width: 140px; text-align: right; color: #888; }
.files-th.files-check, .files-cell.files-check { width: 32px; }
.files-row:hover { background: #f5fafd; }
.files-cell { padding: 10px 4px; border-bottom: 1px solid #f5f5f5; }
.files-icon { margin-right: 8px; }
.files-name { display: inline-flex; align-items: center; gap: 4px; color: inherit; text-decoration: none; background: none; border: 0; padding: 0; font: inherit; cursor: pointer; }
.files-name-folder { font-weight: 600; }
.files-name-file:hover { text-decoration: underline; }
```

- [ ] **Step 4: Re-export**

In `crates/crabcloud-ui/src/pages/files/mod.rs`, append:
```rust
pub mod list;
pub mod row;
```

- [ ] **Step 5: Build + tests**

```
cargo check -p crabcloud-ui --features server
cargo check -p crabcloud-ui --target wasm32-unknown-unknown --features web
cargo test -p crabcloud-ui pages::files::row::tests
```
Expected: clean + 3 tests passing.

- [ ] **Step 6: Commit**

```
git add crates/crabcloud-ui/Cargo.toml Cargo.toml crates/crabcloud-ui/src/pages/files/ crates/crabcloud-ui/assets/app.css
git commit -m "sp6(b): FileRow + FileList components"
```

### Task B4: Breadcrumb

**Files:**
- Create: `crates/crabcloud-ui/src/pages/files/breadcrumb.rs`
- Modify: `crates/crabcloud-ui/src/pages/files/mod.rs`
- Modify: `crates/crabcloud-ui/assets/app.css`

- [ ] **Step 1: Write the component**

`crates/crabcloud-ui/src/pages/files/breadcrumb.rs`:
```rust
//! Breadcrumb showing the path from Home to the current folder. Each
//! segment is clickable and navigates back up the tree.

use crate::pages::files::path::path_to_segments;
use dioxus::prelude::*;

#[component]
pub fn Breadcrumb(path: String, on_navigate: EventHandler<String>) -> Element {
    let segments = path_to_segments(&path);
    let mut cumulative = String::from("/");
    let mut crumbs: Vec<(String, String)> = vec![("Home".to_string(), "/".to_string())];
    for seg in segments {
        if cumulative != "/" {
            cumulative.push('/');
        } else {
            cumulative.clear();
            cumulative.push('/');
        }
        if cumulative == "/" {
            cumulative = format!("/{seg}");
        } else {
            cumulative.push_str(&seg);
        }
        crumbs.push((seg, cumulative.clone()));
    }
    let last_index = crumbs.len().saturating_sub(1);
    rsx! {
        nav { class: "files-breadcrumb",
            for (i, (label, target)) in crumbs.iter().enumerate() {
                if i > 0 {
                    span { class: "files-breadcrumb-sep", "›" }
                }
                if i == last_index {
                    span { class: "files-breadcrumb-here", "{label}" }
                } else {
                    button {
                        class: "files-breadcrumb-link",
                        onclick: {
                            let t = target.clone();
                            move |_| on_navigate.call(t.clone())
                        },
                        "{label}"
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::files::path::path_to_segments;

    #[test]
    fn root_yields_single_home() {
        let segs = path_to_segments("/");
        assert!(segs.is_empty());
    }

    #[test]
    fn one_level_path() {
        let segs = path_to_segments("/photos");
        assert_eq!(segs, vec!["photos".to_string()]);
    }

    #[test]
    fn two_level_path() {
        let segs = path_to_segments("/photos/vacation");
        assert_eq!(segs, vec!["photos".to_string(), "vacation".to_string()]);
    }
}
```

- [ ] **Step 2: CSS**

Append to `crates/crabcloud-ui/assets/app.css`:
```css
.files-breadcrumb { display: flex; align-items: center; gap: 6px; font-size: 13px; margin-bottom: 12px; color: #666; }
.files-breadcrumb-link { background: none; border: 0; padding: 2px 4px; font: inherit; color: #0082c9; cursor: pointer; }
.files-breadcrumb-link:hover { text-decoration: underline; }
.files-breadcrumb-here { font-weight: 600; color: #333; }
.files-breadcrumb-sep { color: #aaa; }
```

- [ ] **Step 3: Re-export**

In `crates/crabcloud-ui/src/pages/files/mod.rs`, append:
```rust
pub mod breadcrumb;
```

- [ ] **Step 4: Test + build**

```
cargo test -p crabcloud-ui pages::files::breadcrumb::tests
cargo check -p crabcloud-ui --features server
cargo check -p crabcloud-ui --target wasm32-unknown-unknown --features web
```
Expected: 3 tests + clean builds.

- [ ] **Step 5: Commit**

```
git add crates/crabcloud-ui/src/pages/files/ crates/crabcloud-ui/assets/app.css
git commit -m "sp6(b): breadcrumb component"
```

### Task B5: Wire the page — data, navigation, downloads

**Files:**
- Modify: `crates/crabcloud-ui/src/pages/files/mod.rs`

- [ ] **Step 1: Replace `Files` with the live version**

Replace the `Files` component body in `crates/crabcloud-ui/src/pages/files/mod.rs` with:
```rust
use crate::context::RequestContext;
use crate::pages::files::breadcrumb::Breadcrumb;
use crate::pages::files::list::FileList;
use crate::server_fns::{list_dir, FileEntry};
use dioxus::prelude::*;

#[component]
pub fn Files(ctx: RequestContext, path: String) -> Element {
    #[cfg(feature = "server")]
    {
        let current_path = format!(
            "/apps/files{}",
            if path == "/" { String::new() } else { path.clone() }
        );
        if ssr::redirect_if_anonymous(&ctx.user_id, &current_path) {
            return rsx! { "" };
        }
    }

    let user_id = ctx.user_id.clone().unwrap_or_default();
    let mut path_sig = use_signal(|| path.clone());
    let mut refresh = use_signal(|| 0u64);

    // Keep the route's `path` prop in sync with the signal. If the user
    // hits back/forward, the prop changes; reflect it into the signal.
    use_effect(use_reactive((&path,), move |(p,)| {
        path_sig.set(p);
    }));

    let entries = use_resource(move || {
        let p = path_sig();
        let _ = refresh();
        async move {
            list_dir(p)
                .await
                .map_err(|e| format!("{e}"))
        }
    });

    let nav = use_navigator();
    let on_open_folder = move |target: String| {
        use crate::pages::files::path::path_to_segments;
        let segs = path_to_segments(&target);
        nav.push(crate::Route::FilesRoute { segments: segs });
    };
    let on_navigate_breadcrumb = on_open_folder;
    let on_retry = move |_| refresh.set(refresh() + 1);

    let entries_view = entries.read().clone();

    rsx! {
        div { class: "files-page",
            chrome::TopBar { ctx: ctx.clone() }
            div { class: "files-body",
                chrome::Sidebar {}
                main { class: "files-main",
                    Breadcrumb {
                        path: path_sig(),
                        on_navigate: on_navigate_breadcrumb,
                    }
                    FileList {
                        entries: entries_view,
                        user_id: user_id,
                        on_open_folder,
                        on_retry,
                    }
                }
            }
        }
    }
}
```

(`use_reactive` is the Dioxus 0.7 helper for capturing changing component props inside an effect; if the exact name differs in this codebase, use the existing pattern from `login_v2_flow.rs` or another page that reads a route prop.)

- [ ] **Step 2: Build server + WASM**

```
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo check -p crabcloud-ui --target wasm32-unknown-unknown --features web
cargo test --workspace
```
Expected: clean, all tests passing.

- [ ] **Step 3: Commit**

```
git add crates/crabcloud-ui/src/pages/files/mod.rs
git commit -m "sp6(b): wire files page to list_dir + breadcrumb + click-to-navigate"
```

### Task B6: PR + merge

- [ ] **Step 1:** `git push -u origin sp6/b-browse`
- [ ] **Step 2:** Open the PR (use the conventions template; summary bullets: "list_dir server fn", "FileList/FileRow/Breadcrumb components", "click-to-navigate folders + anchor-based downloads").
- [ ] **Step 3:** `gh pr checks --watch` → wait for 5 green.
- [ ] **Step 4:** `gh pr merge --squash --delete-branch`.

---

## Batch C — Mkdir, rename, delete

**Branch:** `sp6/c-mutations` off `origin/master`.
**Goal:** Inline "New folder" row creates a directory. Inline rename swaps the row's name cell for an input on demand. Delete pops a modal that confirms then removes one or many items. All three operations refresh the list on success.

### Task C1: `mkdir`, `rename`, `delete` server fns + tests

**Files:**
- Modify: `crates/crabcloud-ui/src/server_fns.rs`
- Modify: `crates/crabcloud-ui/src/lib.rs`
- Modify: `crates/crabcloud-ui/tests/server_fns_files.rs`

- [ ] **Step 1: Add the server fns**

Append to `crates/crabcloud-ui/src/server_fns.rs`:
```rust
#[cfg(feature = "server")]
async fn require_user() -> Result<(crabcloud_core::AppState, crabcloud_users::UserId), ServerFnError> {
    use dioxus::fullstack::FullstackContext;
    let fs = FullstackContext::current()
        .ok_or_else(|| ServerFnError::new("not running on server"))?;
    let state = fs
        .extension::<crabcloud_core::AppState>()
        .ok_or_else(|| ServerFnError::new("AppState extension missing"))?;
    let session = fs.extension::<crabcloud_http::SessionHandle>();
    let snapshot = session.and_then(|s| s.try_read_snapshot());
    let uid_str = snapshot
        .as_ref()
        .and_then(|s| s.user_id.clone())
        .ok_or_else(|| ServerFnError::new("unauthorized"))?;
    let uid = crabcloud_users::UserId::new(&uid_str)
        .map_err(|e| ServerFnError::new(format!("invalid uid: {e}")))?;
    Ok((state, uid))
}

#[server(endpoint = "api/files/mkdir", prefix = "")]
pub async fn mkdir(path: String) -> Result<FileEntry, ServerFnError> {
    use crabcloud_fs::UserPath;
    let (state, uid) = require_user().await?;
    let user_path =
        UserPath::new(&path).map_err(|e| ServerFnError::new(format!("invalid_path: {e}")))?;
    let view = state.view_for(&uid).await.map_err(map_fs_err)?;
    let meta = view.mkdir(&user_path).await.map_err(map_fs_err)?;
    Ok(metadata_to_entry(&user_path, meta))
}

#[server(endpoint = "api/files/rename", prefix = "")]
pub async fn rename(from: String, to: String) -> Result<FileEntry, ServerFnError> {
    use crabcloud_fs::UserPath;
    let (state, uid) = require_user().await?;
    let from_path =
        UserPath::new(&from).map_err(|e| ServerFnError::new(format!("invalid_path: {e}")))?;
    let to_path =
        UserPath::new(&to).map_err(|e| ServerFnError::new(format!("invalid_path: {e}")))?;
    let view = state.view_for(&uid).await.map_err(map_fs_err)?;
    view.rename(&from_path, &to_path).await.map_err(map_fs_err)?;
    let meta = view.stat(&to_path).await.map_err(map_fs_err)?;
    Ok(metadata_to_entry(&to_path, meta))
}

#[server(endpoint = "api/files/delete", prefix = "")]
pub async fn delete(paths: Vec<String>) -> Result<(), ServerFnError> {
    use crabcloud_fs::UserPath;
    let (state, uid) = require_user().await?;
    let view = state.view_for(&uid).await.map_err(map_fs_err)?;
    for path in paths {
        let user_path =
            UserPath::new(&path).map_err(|e| ServerFnError::new(format!("invalid_path: {e}")))?;
        view.delete(&user_path).await.map_err(map_fs_err)?;
    }
    Ok(())
}

#[cfg(feature = "server")]
fn metadata_to_entry(user_path: &crabcloud_fs::UserPath, meta: crabcloud_storage::FileMetadata) -> FileEntry {
    use std::time::UNIX_EPOCH;
    let is_dir = matches!(meta.kind, crabcloud_storage::FileKind::Directory);
    let mtime_ms = meta
        .mtime
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    FileEntry {
        name: user_path
            .as_str()
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_string(),
        path: user_path.as_str().to_string(),
        is_dir,
        size: meta.size,
        mtime_ms,
        mime: (!is_dir).then(|| meta.mimetype.as_str().to_string()),
        etag: meta.etag.as_str().to_string(),
    }
}
```

- [ ] **Step 2: Re-export**

In `crates/crabcloud-ui/src/lib.rs`:
```rust
pub use server_fns::{delete, list_dir, login, mkdir, rename, status, FileEntry, StatusInfo};
```

- [ ] **Step 3: Add integration tests**

Append to `crates/crabcloud-ui/tests/server_fns_files.rs`:
```rust
async fn post_json(
    app: &axum::Router,
    token: &str,
    uri: &str,
    body: serde_json::Value,
) -> axum::response::Response {
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("ocs-apirequest", "true")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap()
}

#[tokio::test]
async fn mkdir_creates_directory() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("mk.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());
    let resp = post_json(
        &app,
        &token,
        "/api/files/mkdir",
        serde_json::json!({ "path": "/newdir" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn rename_moves_file() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("rn.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());
    put_file(&app, &token, "/old.txt", "hi").await;
    let resp = post_json(
        &app,
        &token,
        "/api/files/rename",
        serde_json::json!({ "from": "/old.txt", "to": "/new.txt" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    // Verify via DAV GET that the file moved.
    let get = Request::builder()
        .method("GET")
        .uri("/dav/files/alice/new.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let r = app.clone().oneshot(get).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
}

#[tokio::test]
async fn delete_removes_files() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("dl.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());
    put_file(&app, &token, "/a.txt", "a").await;
    put_file(&app, &token, "/b.txt", "b").await;
    let resp = post_json(
        &app,
        &token,
        "/api/files/delete",
        serde_json::json!({ "paths": ["/a.txt", "/b.txt"] }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let get = Request::builder()
        .method("GET")
        .uri("/dav/files/alice/a.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let r = app.oneshot(get).await.unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}
```

- [ ] **Step 4: Run tests**

```
cargo test -p crabcloud-ui --test server_fns_files
```
Expected: 6 passing (3 existing + 3 new).

- [ ] **Step 5: Commit**

```
git add crates/crabcloud-ui/src/server_fns.rs crates/crabcloud-ui/src/lib.rs crates/crabcloud-ui/tests/server_fns_files.rs
git commit -m "sp6(c): mkdir/rename/delete server fns + tests"
```

### Task C2: Inline `MkdirRow`

**Files:**
- Create: `crates/crabcloud-ui/src/pages/files/mkdir_row.rs`
- Modify: `crates/crabcloud-ui/src/pages/files/mod.rs`
- Modify: `crates/crabcloud-ui/assets/app.css`

- [ ] **Step 1: Component**

```rust
//! Inline "New folder" row. Appears at the top of the file list while the
//! user is creating a directory. Enter commits, Escape cancels.

use dioxus::prelude::*;

#[component]
pub fn MkdirRow(on_commit: EventHandler<String>, on_cancel: EventHandler<()>) -> Element {
    let mut name = use_signal(|| "New folder".to_string());

    let on_keydown = move |evt: KeyboardEvent| {
        match evt.key() {
            Key::Enter => {
                evt.prevent_default();
                let v = name().trim().to_string();
                if !v.is_empty() {
                    on_commit.call(v);
                }
            }
            Key::Escape => {
                evt.prevent_default();
                on_cancel.call(());
            }
            _ => {}
        }
    };

    rsx! {
        tr { class: "files-row files-row-mkdir",
            td { class: "files-cell files-check" }
            td { class: "files-cell",
                span { class: "files-icon", "📁" }
                input {
                    class: "files-mkdir-input",
                    value: "{name}",
                    autofocus: true,
                    oninput: move |e| name.set(e.value()),
                    onkeydown: on_keydown,
                    onblur: move |_| {
                        let v = name().trim().to_string();
                        if v.is_empty() {
                            on_cancel.call(());
                        } else {
                            on_commit.call(v);
                        }
                    },
                }
            }
            td { class: "files-cell files-size", "—" }
            td { class: "files-cell files-mtime", "just now" }
        }
    }
}
```

- [ ] **Step 2: CSS**

Append:
```css
.files-row-mkdir { background: #fffbe6; }
.files-mkdir-input { padding: 4px 6px; font: inherit; border: 1px solid #d0d0d0; border-radius: 3px; width: 240px; }
```

- [ ] **Step 3: Re-export**

In `pages/files/mod.rs`:
```rust
pub mod mkdir_row;
```

- [ ] **Step 4: Build**

```
cargo check -p crabcloud-ui --target wasm32-unknown-unknown --features web
```
Expected: clean.

- [ ] **Step 5: Commit**

```
git add crates/crabcloud-ui/src/pages/files/ crates/crabcloud-ui/assets/app.css
git commit -m "sp6(c): inline MkdirRow component"
```

### Task C3: Inline rename input in `FileRow`

**Files:**
- Modify: `crates/crabcloud-ui/src/pages/files/row.rs`
- Modify: `crates/crabcloud-ui/assets/app.css`

- [ ] **Step 1: Add rename mode + ⋯ menu**

Replace the body of `crates/crabcloud-ui/src/pages/files/row.rs` with:
```rust
//! Single row in the file list. Renders either the standard view or an
//! inline rename input when `rename_active == true`. ⋯ menu emits events
//! for rename/delete (cut is added in Batch D).

use crate::server_fns::FileEntry;
use dioxus::prelude::*;

#[derive(Props, Clone, PartialEq)]
pub struct FileRowProps {
    pub entry: FileEntry,
    pub user_id: String,
    pub rename_active: bool,
    pub selected: bool,
    pub on_open_folder: EventHandler<String>,
    pub on_toggle_select: EventHandler<String>,
    pub on_rename_start: EventHandler<String>,
    pub on_rename_commit: EventHandler<(String, String)>, // (from_path, new_name)
    pub on_rename_cancel: EventHandler<()>,
    pub on_delete: EventHandler<String>,
}

#[component]
pub fn FileRow(props: FileRowProps) -> Element {
    let FileRowProps {
        entry,
        user_id,
        rename_active,
        selected,
        on_open_folder,
        on_toggle_select,
        on_rename_start,
        on_rename_commit,
        on_rename_cancel,
        on_delete,
    } = props;

    let icon = if entry.is_dir { "📁" } else { "📄" };
    let size = if entry.is_dir { "—".into() } else { format_size(entry.size) };
    let mtime = format_mtime(entry.mtime_ms);
    let path_for_open = entry.path.clone();
    let path_for_toggle = entry.path.clone();
    let path_for_rename_start = entry.path.clone();
    let path_for_delete = entry.path.clone();
    let path_for_commit = entry.path.clone();

    let mut menu_open = use_signal(|| false);
    let mut rename_value = use_signal(|| entry.name.clone());

    let name_cell = if rename_active {
        let on_commit = move || {
            let new_name = rename_value().trim().to_string();
            if new_name.is_empty() || new_name == entry.name {
                on_rename_cancel.call(());
            } else {
                on_rename_commit.call((path_for_commit.clone(), new_name));
            }
        };
        rsx! {
            span { class: "files-icon", "{icon}" }
            input {
                class: "files-rename-input",
                value: "{rename_value}",
                autofocus: true,
                oninput: move |e| rename_value.set(e.value()),
                onkeydown: move |e: KeyboardEvent| {
                    match e.key() {
                        Key::Enter => { e.prevent_default(); on_commit(); }
                        Key::Escape => { e.prevent_default(); on_rename_cancel.call(()); }
                        _ => {}
                    }
                },
                onblur: move |_| on_commit(),
            }
        }
    } else if entry.is_dir {
        rsx! {
            button {
                class: "files-name files-name-folder",
                onclick: move |_| on_open_folder.call(path_for_open.clone()),
                span { class: "files-icon", "{icon}" }
                "{entry.name}"
            }
        }
    } else {
        let href = format!("/dav/files/{user_id}{}", entry.path);
        rsx! {
            a {
                class: "files-name files-name-file",
                href: "{href}",
                onclick: move |evt: MouseEvent| evt.stop_propagation(),
                span { class: "files-icon", "{icon}" }
                "{entry.name}"
            }
        }
    };

    rsx! {
        tr { class: if selected { "files-row files-row-selected" } else { "files-row" },
            td { class: "files-cell files-check",
                input {
                    r#type: "checkbox",
                    checked: selected,
                    onchange: move |_| on_toggle_select.call(path_for_toggle.clone()),
                }
            }
            td { class: "files-cell", {name_cell} }
            td { class: "files-cell files-size", "{size}" }
            td { class: "files-cell files-mtime", "{mtime}" }
            td { class: "files-cell files-actions",
                button {
                    class: "files-overflow-btn",
                    onclick: move |_| menu_open.set(!menu_open()),
                    "⋯"
                }
                if menu_open() {
                    div { class: "files-overflow-menu",
                        button {
                            class: "files-overflow-item",
                            onclick: move |_| {
                                menu_open.set(false);
                                on_rename_start.call(path_for_rename_start.clone());
                            },
                            "Rename"
                        }
                        button {
                            class: "files-overflow-item files-overflow-danger",
                            onclick: move |_| {
                                menu_open.set(false);
                                on_delete.call(path_for_delete.clone());
                            },
                            "Delete"
                        }
                    }
                }
            }
        }
    }
}

// `format_size` + `format_mtime` + `current_time_ms` + tests unchanged from
// the Batch B version below this comment.
```
Keep the existing `format_size`, `format_mtime`, `current_time_ms`, and `mod tests` definitions from Batch B. They are unchanged.

- [ ] **Step 2: Update `FileList` to thread the new props**

Modify `crates/crabcloud-ui/src/pages/files/list.rs`:
```rust
use crate::pages::files::row::FileRow;
use crate::pages::files::states::{EmptyFolder, LoadError, Skeleton};
use crate::server_fns::FileEntry;
use dioxus::prelude::*;
use std::collections::HashSet;

#[derive(Props, Clone, PartialEq)]
pub struct FileListProps {
    pub entries: Option<Result<Vec<FileEntry>, String>>,
    pub user_id: String,
    pub selection: HashSet<String>,
    pub rename_target: Option<String>,
    pub on_open_folder: EventHandler<String>,
    pub on_toggle_select: EventHandler<String>,
    pub on_rename_start: EventHandler<String>,
    pub on_rename_commit: EventHandler<(String, String)>,
    pub on_rename_cancel: EventHandler<()>,
    pub on_delete: EventHandler<String>,
    pub on_retry: EventHandler<()>,
}

#[component]
pub fn FileList(props: FileListProps) -> Element {
    let FileListProps {
        entries,
        user_id,
        selection,
        rename_target,
        on_open_folder,
        on_toggle_select,
        on_rename_start,
        on_rename_commit,
        on_rename_cancel,
        on_delete,
        on_retry,
    } = props;

    match entries {
        None => rsx! { Skeleton {} },
        Some(Err(msg)) => rsx! { LoadError { reason: msg, on_retry } },
        Some(Ok(es)) if es.is_empty() => rsx! { EmptyFolder {} },
        Some(Ok(es)) => rsx! {
            table { class: "files-table",
                thead {
                    tr {
                        th { class: "files-th files-check" }
                        th { class: "files-th", "Name" }
                        th { class: "files-th files-size", "Size" }
                        th { class: "files-th files-mtime", "Modified" }
                        th { class: "files-th files-actions" }
                    }
                }
                tbody {
                    for e in es {
                        FileRow {
                            entry: e.clone(),
                            user_id: user_id.clone(),
                            rename_active: rename_target.as_deref() == Some(&e.path),
                            selected: selection.contains(&e.path),
                            on_open_folder,
                            on_toggle_select,
                            on_rename_start,
                            on_rename_commit,
                            on_rename_cancel,
                            on_delete,
                        }
                    }
                }
            }
        },
    }
}
```

- [ ] **Step 3: CSS**

Append:
```css
.files-rename-input { padding: 4px 6px; font: inherit; border: 1px solid #d0d0d0; border-radius: 3px; width: 240px; }
.files-row-selected { background: #f5fafd; }
.files-actions { width: 40px; text-align: right; position: relative; }
.files-overflow-btn { background: none; border: 0; padding: 4px 8px; font: inherit; cursor: pointer; color: #666; }
.files-overflow-btn:hover { color: #0082c9; }
.files-overflow-menu { position: absolute; right: 4px; top: 28px; background: white; border: 1px solid #ddd; border-radius: 4px; box-shadow: 0 4px 12px rgba(0,0,0,0.08); z-index: 10; min-width: 140px; padding: 4px 0; }
.files-overflow-item { display: block; width: 100%; background: none; border: 0; padding: 6px 12px; text-align: left; font: inherit; cursor: pointer; }
.files-overflow-item:hover { background: #f5fafd; }
.files-overflow-danger { color: #d33; }
```

- [ ] **Step 4: Build**

```
cargo check -p crabcloud-ui --target wasm32-unknown-unknown --features web
cargo test --workspace
```
Expected: clean. (Some existing tests reference the old `FileRow` signature — there shouldn't be any in unit tests yet; if there are, update them.)

- [ ] **Step 5: Commit**

```
git add crates/crabcloud-ui/src/pages/files/ crates/crabcloud-ui/assets/app.css
git commit -m "sp6(c): inline rename + ⋯ menu in FileRow"
```

### Task C4: `DeleteModal` component

**Files:**
- Create: `crates/crabcloud-ui/src/pages/files/delete_modal.rs`
- Modify: `crates/crabcloud-ui/src/pages/files/mod.rs`
- Modify: `crates/crabcloud-ui/assets/app.css`

- [ ] **Step 1: Component**

```rust
//! Centered modal that confirms a destructive delete. Lists the items (up
//! to 5 explicitly, then "and N more") and requires explicit click to
//! confirm.

use dioxus::prelude::*;

#[component]
pub fn DeleteModal(paths: Vec<String>, on_cancel: EventHandler<()>, on_confirm: EventHandler<()>) -> Element {
    let count = paths.len();
    let preview: Vec<String> = paths
        .iter()
        .take(5)
        .map(|p| p.rsplit('/').next().unwrap_or(p).to_string())
        .collect();
    let extra = count.saturating_sub(preview.len());
    rsx! {
        div { class: "files-modal-backdrop", onclick: move |_| on_cancel.call(()),
            div {
                class: "files-modal",
                onclick: move |e: MouseEvent| e.stop_propagation(),
                div { class: "files-modal-title",
                    if count == 1 { "Delete {preview[0]}?" } else { "Delete {count} items?" }
                }
                div { class: "files-modal-body",
                    {preview.join(", ")}
                    if extra > 0 { " and {extra} more" }
                }
                div { class: "files-modal-actions",
                    button {
                        class: "files-modal-cancel",
                        onclick: move |_| on_cancel.call(()),
                        "Cancel"
                    }
                    button {
                        class: "files-modal-confirm",
                        onclick: move |_| on_confirm.call(()),
                        "Delete"
                    }
                }
            }
        }
    }
}
```

- [ ] **Step 2: CSS**

Append:
```css
.files-modal-backdrop { position: fixed; inset: 0; background: rgba(0,0,0,0.3); display: flex; align-items: center; justify-content: center; z-index: 100; }
.files-modal { background: white; border-radius: 6px; padding: 20px; max-width: 360px; width: 100%; box-shadow: 0 12px 36px rgba(0,0,0,0.2); }
.files-modal-title { font-weight: 600; margin-bottom: 8px; }
.files-modal-body { font-size: 13px; color: #666; margin-bottom: 16px; }
.files-modal-actions { display: flex; justify-content: flex-end; gap: 8px; }
.files-modal-cancel, .files-modal-confirm { padding: 6px 14px; border-radius: 4px; border: 0; font: inherit; cursor: pointer; }
.files-modal-cancel { background: #f5f5f5; color: #333; }
.files-modal-confirm { background: #d33; color: white; }
```

- [ ] **Step 3: Re-export**

```rust
pub mod delete_modal;
```

- [ ] **Step 4: Build**

```
cargo check -p crabcloud-ui --target wasm32-unknown-unknown --features web
```
Expected: clean.

- [ ] **Step 5: Commit**

```
git add crates/crabcloud-ui/src/pages/files/ crates/crabcloud-ui/assets/app.css
git commit -m "sp6(c): DeleteModal component"
```

### Task C5: Wire mkdir/rename/delete into the page

**Files:**
- Modify: `crates/crabcloud-ui/src/pages/files/mod.rs`

- [ ] **Step 1: Replace the page body**

Update the `Files` component's body to include the new pieces. The full replacement:
```rust
use crate::context::RequestContext;
use crate::pages::files::breadcrumb::Breadcrumb;
use crate::pages::files::delete_modal::DeleteModal;
use crate::pages::files::list::FileList;
use crate::pages::files::mkdir_row::MkdirRow;
use crate::server_fns::{delete, list_dir, mkdir, rename, FileEntry};
use dioxus::prelude::*;
use std::collections::HashSet;

pub mod breadcrumb;
pub mod chrome;
pub mod delete_modal;
pub mod list;
pub mod mkdir_row;
pub mod path;
pub mod row;
#[cfg(feature = "server")]
pub mod ssr;
pub mod states;

#[component]
pub fn Files(ctx: RequestContext, path: String) -> Element {
    #[cfg(feature = "server")]
    {
        let current_path = format!(
            "/apps/files{}",
            if path == "/" { String::new() } else { path.clone() }
        );
        if ssr::redirect_if_anonymous(&ctx.user_id, &current_path) {
            return rsx! { "" };
        }
    }

    let user_id = ctx.user_id.clone().unwrap_or_default();
    let mut path_sig = use_signal(|| path.clone());
    let mut refresh = use_signal(|| 0u64);
    let mut rename_target: Signal<Option<String>> = use_signal(|| None);
    let mut delete_target: Signal<Option<Vec<String>>> = use_signal(|| None);
    let mut mkdir_active = use_signal(|| false);
    let selection: Signal<HashSet<String>> = use_signal(HashSet::new);

    use_effect(use_reactive((&path,), move |(p,)| {
        path_sig.set(p);
    }));

    let entries = use_resource(move || {
        let p = path_sig();
        let _ = refresh();
        async move { list_dir(p).await.map_err(|e| format!("{e}")) }
    });

    let nav = use_navigator();
    let on_open_folder = move |target: String| {
        let segs = path::path_to_segments(&target);
        nav.push(crate::Route::FilesRoute { segments: segs });
    };
    let on_navigate_breadcrumb = on_open_folder;
    let on_retry = move |_| refresh.set(refresh() + 1);
    let bump = move || refresh.set(refresh() + 1);

    let on_toggle_select = move |path: String| {
        let mut s = selection();
        if !s.insert(path.clone()) {
            s.remove(&path);
        }
        selection.set(s);
    };
    let on_rename_start = move |path: String| rename_target.set(Some(path));
    let on_rename_cancel = move |_| rename_target.set(None);
    let on_rename_commit = move |(from, new_name): (String, String)| {
        spawn(async move {
            let to = match from.rsplit_once('/') {
                Some((parent, _)) if parent.is_empty() => format!("/{new_name}"),
                Some((parent, _)) => format!("{parent}/{new_name}"),
                None => format!("/{new_name}"),
            };
            let _ = rename(from, to).await;
            rename_target.set(None);
            bump();
        });
    };
    let on_delete = move |path: String| delete_target.set(Some(vec![path]));
    let on_delete_confirm = move |_| {
        if let Some(paths) = delete_target() {
            spawn(async move {
                let _ = delete(paths).await;
                delete_target.set(None);
                bump();
            });
        }
    };
    let on_delete_cancel = move |_| delete_target.set(None);

    let on_mkdir_start = move |_| mkdir_active.set(true);
    let on_mkdir_cancel = move |_| mkdir_active.set(false);
    let on_mkdir_commit = move |name: String| {
        let parent = path_sig();
        let new_path = if parent == "/" {
            format!("/{name}")
        } else {
            format!("{parent}/{name}")
        };
        spawn(async move {
            let _ = mkdir(new_path).await;
            mkdir_active.set(false);
            bump();
        });
    };

    let entries_view = entries.read().clone();
    let sel = selection();
    let rn_target = rename_target();

    rsx! {
        div { class: "files-page",
            chrome::TopBar { ctx: ctx.clone() }
            div { class: "files-body",
                chrome::Sidebar {}
                main { class: "files-main",
                    div { class: "files-toolbar",
                        button {
                            class: "files-tb-btn files-tb-primary",
                            onclick: on_mkdir_start,
                            "+ New folder"
                        }
                    }
                    Breadcrumb { path: path_sig(), on_navigate: on_navigate_breadcrumb }
                    if mkdir_active() {
                        table { class: "files-table",
                            tbody {
                                MkdirRow { on_commit: on_mkdir_commit, on_cancel: on_mkdir_cancel }
                            }
                        }
                    }
                    FileList {
                        entries: entries_view,
                        user_id: user_id.clone(),
                        selection: sel,
                        rename_target: rn_target,
                        on_open_folder,
                        on_toggle_select,
                        on_rename_start,
                        on_rename_commit,
                        on_rename_cancel,
                        on_delete,
                        on_retry,
                    }
                }
            }
            if let Some(paths) = delete_target() {
                DeleteModal {
                    paths,
                    on_cancel: on_delete_cancel,
                    on_confirm: on_delete_confirm,
                }
            }
        }
    }
}
```

CSS for the toolbar buttons (append):
```css
.files-toolbar { display: flex; gap: 8px; margin-bottom: 12px; }
.files-tb-btn { padding: 6px 14px; border-radius: 4px; border: 1px solid #d0d0d0; background: white; font: inherit; cursor: pointer; }
.files-tb-primary { background: #0082c9; color: white; border-color: #0082c9; }
```

- [ ] **Step 2: Build**

```
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo check -p crabcloud-ui --target wasm32-unknown-unknown --features web
cargo test --workspace
```
Expected: clean, all tests passing.

- [ ] **Step 3: Commit**

```
git add crates/crabcloud-ui/src/pages/files/mod.rs crates/crabcloud-ui/assets/app.css
git commit -m "sp6(c): wire mkdir + rename + delete into the page"
```

### Task C6: PR + merge

- [ ] **Step 1:** `git push -u origin sp6/c-mutations`
- [ ] **Step 2:** Open the PR (summary bullets: "mkdir/rename/delete server fns", "inline MkdirRow + rename in FileRow", "DeleteModal with confirmation").
- [ ] **Step 3:** `gh pr checks --watch`.
- [ ] **Step 4:** `gh pr merge --squash --delete-branch`.


---

## Batch D — Multi-select + cut/paste move

**Branch:** `sp6/d-clipboard` off `origin/master`.
**Goal:** Multi-select via row checkboxes, a header "select all" checkbox, a "N selected" toolbar chip with bulk-delete and cut, a "N on clipboard" chip that persists across folder navigation, and a paste action that moves the cut items into the current folder.

### Task D1: `move_paths` server fn + integration test

**Files:**
- Modify: `crates/crabcloud-ui/src/server_fns.rs`
- Modify: `crates/crabcloud-ui/src/lib.rs`
- Modify: `crates/crabcloud-ui/tests/server_fns_files.rs`

- [ ] **Step 1: Add the server fn**

Append to `crates/crabcloud-ui/src/server_fns.rs`:
```rust
#[server(endpoint = "api/files/move", prefix = "")]
pub async fn move_paths(paths: Vec<String>, dest_dir: String) -> Result<Vec<FileEntry>, ServerFnError> {
    use crabcloud_fs::UserPath;
    let (state, uid) = require_user().await?;
    let dest =
        UserPath::new(&dest_dir).map_err(|e| ServerFnError::new(format!("invalid_path: {e}")))?;
    let view = state.view_for(&uid).await.map_err(map_fs_err)?;
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        let from = UserPath::new(&path)
            .map_err(|e| ServerFnError::new(format!("invalid_path: {e}")))?;
        let leaf = path.rsplit('/').next().unwrap_or("");
        if leaf.is_empty() {
            return Err(ServerFnError::new("invalid_path: empty leaf"));
        }
        let to = dest
            .join(leaf)
            .map_err(|e| ServerFnError::new(format!("invalid_path: {e}")))?;
        view.rename(&from, &to).await.map_err(map_fs_err)?;
        let meta = view.stat(&to).await.map_err(map_fs_err)?;
        out.push(metadata_to_entry(&to, meta));
    }
    Ok(out)
}
```

- [ ] **Step 2: Re-export**

```rust
pub use server_fns::{delete, list_dir, login, mkdir, move_paths, rename, status, FileEntry, StatusInfo};
```

- [ ] **Step 3: Integration test**

Append to `crates/crabcloud-ui/tests/server_fns_files.rs`:
```rust
#[tokio::test]
async fn move_paths_moves_files_into_destination() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("mv.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    // Create dest dir + two source files.
    let _ = post_json(&app, &token, "/api/files/mkdir", serde_json::json!({ "path": "/dest" })).await;
    put_file(&app, &token, "/a.txt", "a").await;
    put_file(&app, &token, "/b.txt", "b").await;

    let resp = post_json(
        &app,
        &token,
        "/api/files/move",
        serde_json::json!({ "paths": ["/a.txt", "/b.txt"], "dest_dir": "/dest" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Sources should be gone, destinations present.
    let g_old = Request::builder()
        .method("GET")
        .uri("/dav/files/alice/a.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.clone().oneshot(g_old).await.unwrap().status(), StatusCode::NOT_FOUND);

    let g_new = Request::builder()
        .method("GET")
        .uri("/dav/files/alice/dest/a.txt")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.oneshot(g_new).await.unwrap().status(), StatusCode::OK);
}
```

- [ ] **Step 4: Run + commit**

```
cargo test -p crabcloud-ui --test server_fns_files
git add crates/crabcloud-ui/src/server_fns.rs crates/crabcloud-ui/src/lib.rs crates/crabcloud-ui/tests/server_fns_files.rs
git commit -m "sp6(d): move_paths server fn + test"
```

### Task D2: Toolbar with selection + clipboard chips

**Files:**
- Create: `crates/crabcloud-ui/src/pages/files/toolbar.rs`
- Modify: `crates/crabcloud-ui/src/pages/files/mod.rs`
- Modify: `crates/crabcloud-ui/assets/app.css`

- [ ] **Step 1: Component**

```rust
//! Toolbar: New / Upload buttons plus chips for current selection and the
//! cut-clipboard. Chips are compact and live in the same row as the
//! buttons (spec §11/decision 11).

use dioxus::prelude::*;

#[derive(Props, Clone, PartialEq)]
pub struct ToolbarProps {
    pub selection_count: usize,
    pub clipboard_count: usize,
    pub clipboard_source: Option<String>,
    pub can_paste: bool,
    pub on_new_folder: EventHandler<()>,
    pub on_upload: EventHandler<()>,
    pub on_cut: EventHandler<()>,
    pub on_delete_selection: EventHandler<()>,
    pub on_clear_selection: EventHandler<()>,
    pub on_paste: EventHandler<()>,
    pub on_clear_clipboard: EventHandler<()>,
}

#[component]
pub fn Toolbar(props: ToolbarProps) -> Element {
    rsx! {
        div { class: "files-toolbar",
            button { class: "files-tb-btn files-tb-primary", onclick: move |_| props.on_new_folder.call(()), "+ New folder" }
            button { class: "files-tb-btn", onclick: move |_| props.on_upload.call(()), "⬆ Upload" }
            if props.selection_count > 0 {
                div { class: "files-chip files-chip-selection",
                    span { "{props.selection_count} selected" }
                    button { class: "files-chip-action", onclick: move |_| props.on_cut.call(()), "✂ Cut" }
                    button { class: "files-chip-action files-chip-danger", onclick: move |_| props.on_delete_selection.call(()), "🗑 Delete" }
                    button { class: "files-chip-close", onclick: move |_| props.on_clear_selection.call(()), "✕" }
                }
            }
            if props.clipboard_count > 0 {
                div { class: "files-chip files-chip-clipboard",
                    {
                        match &props.clipboard_source {
                            Some(src) => rsx!(span { "✂ {props.clipboard_count} on clipboard from {src}" }),
                            None => rsx!(span { "✂ {props.clipboard_count} on clipboard" }),
                        }
                    }
                    button {
                        class: "files-chip-action",
                        disabled: !props.can_paste,
                        onclick: move |_| props.on_paste.call(()),
                        "Paste here"
                    }
                    button { class: "files-chip-close", onclick: move |_| props.on_clear_clipboard.call(()), "✕" }
                }
            }
        }
    }
}
```

- [ ] **Step 2: CSS**

Append:
```css
.files-chip { display: inline-flex; align-items: center; gap: 8px; padding: 4px 10px; border-radius: 14px; font-size: 12px; }
.files-chip-selection { background: #e8f3fb; color: #0082c9; }
.files-chip-clipboard { background: #fff3d6; color: #a07300; }
.files-chip-action { background: none; border: 0; padding: 2px 6px; font: inherit; cursor: pointer; color: inherit; }
.files-chip-action[disabled] { opacity: 0.4; cursor: not-allowed; }
.files-chip-danger { color: #d33; }
.files-chip-close { background: none; border: 0; padding: 2px 6px; cursor: pointer; color: inherit; opacity: 0.6; }
.files-chip-close:hover { opacity: 1; }
```

- [ ] **Step 3: Re-export**

```rust
pub mod toolbar;
```

- [ ] **Step 4: Build + commit**

```
cargo check -p crabcloud-ui --target wasm32-unknown-unknown --features web
git add crates/crabcloud-ui/src/pages/files/ crates/crabcloud-ui/assets/app.css
git commit -m "sp6(d): toolbar with selection + clipboard chips"
```

### Task D3: Wire multi-select + cut/paste into the page

**Files:**
- Modify: `crates/crabcloud-ui/src/pages/files/mod.rs`

- [ ] **Step 1: Add clipboard state and wire the toolbar**

Modify the `Files` component:
- Add a `Clipboard` struct and a `Signal<Option<Clipboard>>`.
- Replace the inline toolbar markup with `Toolbar { ... }`.
- Use `move_paths` for the Paste action.

```rust
#[derive(Clone, PartialEq)]
pub struct Clipboard {
    pub source_dir: String,
    pub paths: Vec<String>,
}
```

Add inside `Files`:
```rust
let mut clipboard: Signal<Option<Clipboard>> = use_signal(|| None);

let on_cut = move |_| {
    let s = selection();
    if !s.is_empty() {
        clipboard.set(Some(Clipboard {
            source_dir: path_sig(),
            paths: s.into_iter().collect(),
        }));
        selection.set(HashSet::new());
    }
};
let on_clear_selection = move |_| selection.set(HashSet::new());
let on_clear_clipboard = move |_| clipboard.set(None);
let on_delete_selection = move |_| {
    let s = selection();
    if !s.is_empty() {
        delete_target.set(Some(s.into_iter().collect()));
    }
};
let on_paste = move |_| {
    if let Some(cb) = clipboard() {
        let dest = path_sig();
        if dest == cb.source_dir {
            return;
        }
        spawn(async move {
            let _ = move_paths(cb.paths, dest).await;
            clipboard.set(None);
            bump();
        });
    }
};
```

Replace the previous toolbar div with:
```rust
Toolbar {
    selection_count: selection().len(),
    clipboard_count: clipboard().as_ref().map(|c| c.paths.len()).unwrap_or(0),
    clipboard_source: clipboard().as_ref().map(|c| c.source_dir.clone()),
    can_paste: clipboard()
        .as_ref()
        .map(|c| c.source_dir != path_sig())
        .unwrap_or(false),
    on_new_folder: on_mkdir_start,
    on_upload: move |_| {}, // wired in Batch E
    on_cut,
    on_delete_selection,
    on_clear_selection,
    on_paste,
    on_clear_clipboard,
}
```

Also: clear selection when the path changes. Add inside the existing `use_effect` that syncs path:
```rust
use_effect(use_reactive((&path,), move |(p,)| {
    path_sig.set(p);
    selection.set(HashSet::new());
}));
```

(Clipboard should NOT reset — that's the whole point of cut/paste persisting.)

Also: header "select all" checkbox. Update `FileList` and its `Props` to accept `on_toggle_select_all: EventHandler<bool>` and render `<input type="checkbox" checked=... onchange=...>` in the header `th.files-check`. The "checked" state is `selection.len() > 0 && selection.len() == entries.len()`. Pass the right values from `Files`.

- [ ] **Step 2: Build + commit**

```
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo check -p crabcloud-ui --target wasm32-unknown-unknown --features web
cargo test --workspace
git add crates/crabcloud-ui/src/pages/files/
git commit -m "sp6(d): multi-select + cut/paste flow"
```

### Task D4: PR + merge

- [ ] **Step 1:** `git push -u origin sp6/d-clipboard`
- [ ] **Step 2:** Open PR.
- [ ] **Step 3:** Watch CI; merge.

---

## Batch E — Uploads

**Branch:** `sp6/e-uploads` off `origin/master`.
**Goal:** Upload button (file picker) and drop-anywhere-on-the-file-list drag-drop. Small files (`< 8 MiB`) use a single `PUT /dav/files/<user>/<path>`. Larger files use the chunked-upload protocol against `/dav/uploads/<user>/<id>/<n>` + `MOVE … .file`. Progress strip below the toolbar shows in-flight + queued uploads with cancel and retry.

### Task E1: `upload_begin` server fn + tests

**Files:**
- Modify: `crates/crabcloud-ui/src/server_fns.rs`
- Modify: `crates/crabcloud-ui/src/lib.rs`
- Modify: `crates/crabcloud-ui/tests/server_fns_files.rs`

- [ ] **Step 1: Add the server fn**

Append to `crates/crabcloud-ui/src/server_fns.rs`:
```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UploadBeginResponse {
    pub upload_id: String,
}

#[server(endpoint = "api/files/upload_begin", prefix = "")]
pub async fn upload_begin(dest_path: String) -> Result<UploadBeginResponse, ServerFnError> {
    use crabcloud_fs::UserPath;
    let (state, uid) = require_user().await?;
    let dest =
        UserPath::new(&dest_path).map_err(|e| ServerFnError::new(format!("invalid_path: {e}")))?;
    let uploads = state.uploads_for(&uid).await.map_err(map_fs_err)?;
    let handle = uploads.begin(&dest).await.map_err(map_fs_err)?;
    Ok(UploadBeginResponse { upload_id: handle.upload_id })
}
```

- [ ] **Step 2: Re-export from `lib.rs`**

```rust
pub use server_fns::{
    delete, list_dir, login, mkdir, move_paths, rename, status, upload_begin,
    FileEntry, StatusInfo, UploadBeginResponse,
};
```

- [ ] **Step 3: Integration test**

Append to `crates/crabcloud-ui/tests/server_fns_files.rs`:
```rust
#[tokio::test]
async fn upload_begin_returns_opaque_id() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let state = make_state_with_user(dir.path().join("ub.db"), data.path().to_path_buf()).await;
    let token = alice_bearer(&state).await;
    let app = crabcloud_http::build_router(state, axum::Router::new());

    let resp = post_json(
        &app,
        &token,
        "/api/files/upload_begin",
        serde_json::json!({ "dest_path": "/big.bin" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 8 * 1024).await.unwrap();
    let parsed: crabcloud_ui::UploadBeginResponse = serde_json::from_slice(&body).unwrap();
    assert!(parsed.upload_id.contains(':'));
}
```

- [ ] **Step 4: Run + commit**

```
cargo test -p crabcloud-ui --test server_fns_files
git add crates/crabcloud-ui/src/server_fns.rs crates/crabcloud-ui/src/lib.rs crates/crabcloud-ui/tests/server_fns_files.rs
git commit -m "sp6(e): upload_begin server fn"
```

### Task E2: Upload state types + chunked-upload state machine

**Files:**
- Create: `crates/crabcloud-ui/src/pages/files/upload.rs`
- Modify: `crates/crabcloud-ui/Cargo.toml`
- Modify: `crates/crabcloud-ui/src/pages/files/mod.rs`
- Modify: `crates/crabcloud-ui/assets/app.css`

- [ ] **Step 1: Web-platform deps**

In `crates/crabcloud-ui/Cargo.toml` under `[dependencies]`, add (and add the matching workspace entries to root `Cargo.toml` if missing):
```toml
gloo-file = { workspace = true }
gloo-net = { workspace = true }
wasm-bindgen-futures = { workspace = true }
wasm-bindgen = { workspace = true }
web-sys = { workspace = true, features = [
    "Blob", "File", "FileList", "FormData", "HtmlInputElement",
    "Request", "RequestInit", "Response", "DragEvent", "DataTransfer",
] }
```

- [ ] **Step 2: Create the module**

`crates/crabcloud-ui/src/pages/files/upload.rs`:
```rust
//! Upload state machine. Small files PUT to /dav/files/...; larger files use
//! the chunked protocol against /dav/uploads/<user>/<id>/<n> + MOVE. See
//! spec §7.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

pub const SINGLE_PUT_MAX: u64 = 8 * 1024 * 1024;
pub const CHUNK_SIZE: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobState {
    Queued,
    InProgress { percent: u8 },
    Completed,
    Failed { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadJob {
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub dest_path: String,
    pub state: JobState,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UploadQueue {
    next_id: u64,
    pub jobs: Vec<UploadJob>,
}

impl UploadQueue {
    pub fn enqueue(&mut self, name: String, size: u64, dest_path: String) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.jobs.push(UploadJob { id, name, size, dest_path, state: JobState::Queued });
        id
    }
    pub fn update<F: FnOnce(&mut UploadJob)>(&mut self, id: u64, f: F) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) { f(job); }
    }
    pub fn remove(&mut self, id: u64) { self.jobs.retain(|j| j.id != id); }
}

#[component]
pub fn DropOverlay(visible: bool, current_folder: String) -> Element {
    if !visible { return rsx! { "" }; }
    rsx! {
        div { class: "files-drop-overlay",
            div { class: "files-drop-target",
                div { class: "files-drop-icon", "⬇" }
                div { class: "files-drop-title", "Drop to upload to ", em { "{current_folder}" } }
            }
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct PartTagJson { part_number: u32, etag: String }

#[cfg(target_arch = "wasm32")]
pub async fn upload_one(
    user_id: String,
    dest_path: String,
    file: gloo_file::File,
    on_progress: impl Fn(u8) + 'static,
) -> Result<(), String> {
    use gloo_net::http::Request;
    use web_sys::Blob;

    let size = file.size();
    let dest_url = format!("/dav/files/{user_id}{dest_path}");

    if size <= SINGLE_PUT_MAX {
        let blob: &Blob = file.as_ref();
        let resp = Request::put(&dest_url)
            .header("ocs-apirequest", "true")
            .body(blob).map_err(|e| format!("build: {e}"))?
            .send().await.map_err(|e| format!("net: {e}"))?;
        if !resp.ok() { return Err(format!("PUT {} → {}", dest_url, resp.status())); }
        on_progress(100);
        return Ok(());
    }

    let begin = crate::server_fns::upload_begin(dest_path.clone())
        .await.map_err(|e| format!("upload_begin: {e}"))?;
    let upload_id = begin.upload_id;
    let part_count = ((size + CHUNK_SIZE - 1) / CHUNK_SIZE) as u32;
    let mut tags: Vec<PartTagJson> = Vec::with_capacity(part_count as usize);
    let blob: &Blob = file.as_ref();
    for n in 1..=part_count {
        let start = (n as u64 - 1) * CHUNK_SIZE;
        let end = (start + CHUNK_SIZE).min(size);
        let slice = blob.slice_with_f64_and_f64(start as f64, end as f64)
            .map_err(|e| format!("slice: {e:?}"))?;
        let url = format!("/dav/uploads/{user_id}/{upload_id}/{n}");
        let resp = Request::put(&url)
            .header("ocs-apirequest", "true")
            .body(&slice).map_err(|e| format!("build: {e}"))?
            .send().await.map_err(|e| format!("net: {e}"))?;
        if !resp.ok() { return Err(format!("PUT part {n} → {}", resp.status())); }
        let etag = resp.headers().get("etag")
            .ok_or_else(|| "missing etag".to_string())?
            .trim_matches('"').to_string();
        tags.push(PartTagJson { part_number: n, etag });
        on_progress(((end as f64 / size as f64) * 100.0) as u8);
    }
    let tags_json = serde_json::to_string(&tags).map_err(|e| format!("tags: {e}"))?;
    let commit_url = format!("/dav/uploads/{user_id}/{upload_id}/.file");
    let resp = gloo_net::http::Request::new(&commit_url)
        .method(http::Method::from_bytes(b"MOVE").unwrap())
        .header("ocs-apirequest", "true")
        .header("destination", &format!("/dav/files/{user_id}{dest_path}"))
        .header("x-crabcloud-part-tags", &tags_json)
        .send().await.map_err(|e| format!("net: {e}"))?;
    if !resp.ok() { return Err(format!("MOVE commit → {}", resp.status())); }
    on_progress(100);
    Ok(())
}
```

(If `gloo_net::http::Request::method(...)` isn't available, fall back to the lower-level `web_sys::Request` / `wasm_bindgen_futures::JsFuture::from(window.fetch_with_request(...))`. The shape is "issue `MOVE` with `destination` + `x-crabcloud-part-tags` headers".)

- [ ] **Step 3: CSS for the drop overlay**

Append:
```css
.files-main { position: relative; }
.files-drop-overlay { position: absolute; inset: 0; background: rgba(0,130,201,0.08); display: flex; align-items: center; justify-content: center; pointer-events: none; z-index: 5; }
.files-drop-target { border: 3px dashed #0082c9; border-radius: 8px; padding: 32px 48px; color: #0082c9; background: white; text-align: center; }
.files-drop-icon { font-size: 32px; }
.files-drop-title { font-weight: 600; margin-top: 8px; }
```

- [ ] **Step 4: Re-export + build + commit**

In `pages/files/mod.rs`:
```rust
pub mod upload;
```
```
cargo check -p crabcloud-ui --target wasm32-unknown-unknown --features web
git add crates/crabcloud-ui/Cargo.toml Cargo.toml crates/crabcloud-ui/src/pages/files/upload.rs crates/crabcloud-ui/src/pages/files/mod.rs crates/crabcloud-ui/assets/app.css
git commit -m "sp6(e): upload state machine + DropOverlay"
```

### Task E3: `UploadProgressStrip`

**Files:**
- Create: `crates/crabcloud-ui/src/pages/files/progress_strip.rs`
- Modify: `crates/crabcloud-ui/src/pages/files/mod.rs`
- Modify: `crates/crabcloud-ui/assets/app.css`

- [ ] **Step 1: Component**

```rust
//! Inline progress strip. Shows in-progress jobs with their percent + a
//! cancel button, queued-count summary, and failed jobs with Retry.

use crate::pages::files::upload::{JobState, UploadJob};
use dioxus::prelude::*;

#[component]
pub fn UploadProgressStrip(
    jobs: Vec<UploadJob>,
    on_cancel: EventHandler<u64>,
    on_retry: EventHandler<u64>,
) -> Element {
    if jobs.is_empty() { return rsx! { "" }; }
    let queued = jobs.iter().filter(|j| matches!(j.state, JobState::Queued)).count();
    rsx! {
        div { class: "files-progress",
            for job in jobs.iter().filter(|j| matches!(j.state, JobState::InProgress { .. })) {
                {
                    let percent = match job.state { JobState::InProgress { percent } => percent, _ => 0 };
                    let id = job.id;
                    rsx! {
                        div { class: "files-progress-row",
                            span { class: "files-progress-icon", "⬆" }
                            div { class: "files-progress-body",
                                div { class: "files-progress-name", "{job.name} · {percent}%" }
                                div { class: "files-progress-bar",
                                    div { class: "files-progress-fill", style: "width: {percent}%" }
                                }
                            }
                            button { class: "files-progress-cancel", onclick: move |_| on_cancel.call(id), "Cancel" }
                        }
                    }
                }
            }
            if queued > 0 {
                div { class: "files-progress-queued", "+ {queued} queued" }
            }
            for job in jobs.iter().filter(|j| matches!(j.state, JobState::Failed { .. })) {
                {
                    let reason = match &job.state { JobState::Failed { reason } => reason.clone(), _ => String::new() };
                    let id = job.id;
                    rsx! {
                        div { class: "files-progress-row files-progress-failed",
                            span { class: "files-progress-icon", "⚠" }
                            div { class: "files-progress-body",
                                div { class: "files-progress-name", "{job.name}" }
                                div { class: "files-progress-reason", "{reason}" }
                            }
                            button { class: "files-progress-retry", onclick: move |_| on_retry.call(id), "Retry" }
                        }
                    }
                }
            }
        }
    }
}
```

- [ ] **Step 2: CSS**

Append:
```css
.files-progress { background: #fffbe6; border: 1px solid #f5e7a3; border-radius: 4px; padding: 8px 12px; margin-bottom: 12px; font-size: 12px; display: flex; flex-direction: column; gap: 6px; }
.files-progress-row { display: flex; align-items: center; gap: 12px; }
.files-progress-body { flex: 1; }
.files-progress-name { font-weight: 600; }
.files-progress-bar { height: 4px; background: #eee; border-radius: 2px; margin-top: 3px; overflow: hidden; }
.files-progress-fill { height: 100%; background: #0082c9; }
.files-progress-cancel, .files-progress-retry { background: none; border: 1px solid #d0d0d0; padding: 4px 10px; border-radius: 3px; font: inherit; cursor: pointer; }
.files-progress-queued { color: #888; }
.files-progress-failed { color: #d33; }
.files-progress-reason { color: #d33; font-size: 11px; }
```

- [ ] **Step 3: Re-export + commit**

```rust
pub mod progress_strip;
```
```
cargo check -p crabcloud-ui --target wasm32-unknown-unknown --features web
git add crates/crabcloud-ui/src/pages/files/progress_strip.rs crates/crabcloud-ui/src/pages/files/mod.rs crates/crabcloud-ui/assets/app.css
git commit -m "sp6(e): UploadProgressStrip component"
```

### Task E4: Wire uploads into the page

**Files:**
- Modify: `crates/crabcloud-ui/src/pages/files/mod.rs`

- [ ] **Step 1: Add upload signals + handlers**

Inside `Files`, after the other state signals:
```rust
use crate::pages::files::progress_strip::UploadProgressStrip;
use crate::pages::files::upload::{upload_one, DropOverlay, JobState, UploadQueue};

let mut upload_queue: Signal<UploadQueue> = use_signal(UploadQueue::default);
let mut drag_active = use_signal(|| false);

let kick_upload = move |files: Vec<gloo_file::File>| {
    let dest_dir = path_sig();
    let uid = user_id.clone();
    for f in files {
        let name = f.name();
        let size = f.size();
        let dest_path = if dest_dir == "/" {
            format!("/{name}")
        } else { format!("{dest_dir}/{name}") };
        let id = upload_queue.write().enqueue(name.clone(), size, dest_path.clone());
        let uid = uid.clone();
        let dp = dest_path.clone();
        let f = f.clone();
        spawn(async move {
            upload_queue.write().update(id, |j| j.state = JobState::InProgress { percent: 0 });
            let on_progress = move |percent: u8| {
                upload_queue.write().update(id, |j| j.state = JobState::InProgress { percent });
            };
            #[cfg(target_arch = "wasm32")]
            match upload_one(uid, dp, f, on_progress).await {
                Ok(()) => {
                    upload_queue.write().update(id, |j| j.state = JobState::Completed);
                    bump();
                }
                Err(reason) => {
                    upload_queue.write().update(id, |j| j.state = JobState::Failed { reason });
                }
            }
        });
    }
};

let on_upload_click = move |_| {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        if let Some(input) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("files-file-input"))
            .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
        {
            input.click();
        }
    }
};
```

Add WASM helpers at the bottom of the module:
```rust
#[cfg(target_arch = "wasm32")]
fn collect_files_from_input() -> Vec<gloo_file::File> {
    use wasm_bindgen::JsCast;
    let mut out = Vec::new();
    let input = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("files-file-input"))
        .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok());
    if let Some(input) = input {
        if let Some(list) = input.files() {
            for i in 0..list.length() {
                if let Some(file) = list.item(i) {
                    out.push(gloo_file::File::from(file));
                }
            }
        }
    }
    out
}

#[cfg(target_arch = "wasm32")]
fn collect_files_from_drag(evt: &Event<DragData>) -> Vec<gloo_file::File> {
    use wasm_bindgen::JsCast;
    let mut out = Vec::new();
    if let Some(raw) = evt.try_as_web_event() {
        if let Some(dt) = raw.dyn_ref::<web_sys::DragEvent>().and_then(|e| e.data_transfer()) {
            if let Some(list) = dt.files() {
                for i in 0..list.length() {
                    if let Some(file) = list.item(i) {
                        out.push(gloo_file::File::from(file));
                    }
                }
            }
        }
    }
    out
}
```

(The exact accessor for the native event from Dioxus 0.7 events is `evt.try_as_web_event()` or similar. If the API differs, mirror whatever pattern the existing pages use to reach the native event.)

- [ ] **Step 2: Update markup**

In the `Files` rsx tree, the `main { class: "files-main" }` block becomes:
```rust
main { class: "files-main",
    ondragover: move |evt| { evt.prevent_default(); drag_active.set(true); },
    ondragleave: move |_| drag_active.set(false),
    ondrop: move |evt| {
        evt.prevent_default();
        drag_active.set(false);
        #[cfg(target_arch = "wasm32")]
        kick_upload(collect_files_from_drag(&evt));
        let _ = evt;
    },

    input {
        r#type: "file",
        id: "files-file-input",
        multiple: true,
        style: "display: none",
        onchange: move |_| {
            #[cfg(target_arch = "wasm32")]
            kick_upload(collect_files_from_input());
        },
    }

    Toolbar { /* … with on_upload: on_upload_click … */ }
    UploadProgressStrip {
        jobs: upload_queue().jobs.clone(),
        on_cancel: move |id: u64| { upload_queue.write().remove(id); },
        on_retry: move |id: u64| {
            upload_queue.write().update(id, |j| j.state = JobState::Queued);
        },
    }
    Breadcrumb { /* … */ }
    if mkdir_active() {
        table { class: "files-table", tbody { MkdirRow { /* … */ } } }
    }
    FileList { /* … */ }
    DropOverlay { visible: drag_active(), current_folder: path_sig() }
}
```

- [ ] **Step 3: Build + commit**

```
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo check -p crabcloud-ui --target wasm32-unknown-unknown --features web
cargo test --workspace
git add crates/crabcloud-ui/src/pages/files/mod.rs
git commit -m "sp6(e): wire upload button + drop zone + progress strip"
```

### Task E5: PR + merge

- [ ] **Step 1:** `git push -u origin sp6/e-uploads`
- [ ] **Step 2:** Open PR (summary: "upload_begin server fn", "single-PUT + chunked-upload state machine", "drop overlay + progress strip").
- [ ] **Step 3:** Watch CI; merge.

---

## Batch F — Tests, acceptance docs, polish

**Branch:** `sp6/f-tests-polish` off `origin/master`.
**Goal:** Playwright e2e covering spec §18 scenarios + README/changelog/memory updates + SP7 follow-up notes. After this lands, SP6 is done.

### Task F1: Playwright e2e

**Files:**
- Create: `e2e/tests/files.spec.ts`

- [ ] **Step 1: Write the spec**

```ts
import { test, expect } from "@playwright/test";

const BASE_URL = process.env.CRABCLOUD_E2E_URL ?? "http://127.0.0.1:18765";

async function login(request: any): Promise<string> {
    const r = await request.post("/index.php/login", {
        data: { username: "admin", password: "hunter2" },
        headers: { "content-type": "application/json" },
        maxRedirects: 0,
    });
    expect(r.status()).toBe(200);
    const setCookie = r.headers()["set-cookie"];
    const m = /oc_sessionPassphrase=([^;\n]+)/.exec(setCookie!);
    expect(m).not.toBeNull();
    return `oc_sessionPassphrase=${m![1]}`;
}

async function loginInBrowser(page: any) {
    const cookie = await login(page.request);
    const value = /oc_sessionPassphrase=([^;]+)/.exec(cookie)![1];
    await page.context().addCookies([
        { name: "oc_sessionPassphrase", value, url: BASE_URL },
    ]);
}

test.describe("Files web UI", () => {
    test("anonymous /apps/files redirects to login with redirect_url", async ({ page }) => {
        const r = await page.goto("/apps/files/", { waitUntil: "domcontentloaded" });
        expect(r!.url()).toContain("/index.php/login");
        expect(r!.url()).toContain("redirect_url");
        expect(decodeURIComponent(r!.url())).toContain("/apps/files/");
    });

    test("authenticated /apps/files renders chrome + folder list", async ({ page }) => {
        await loginInBrowser(page);
        const r = await page.goto("/apps/files/");
        expect(r!.status()).toBe(200);
        await expect(page.locator(".files-page")).toBeVisible();
        await expect(page.locator(".sidebar-item.active")).toContainText("All files");
    });

    test("mkdir + rename + delete round-trip", async ({ page }) => {
        await loginInBrowser(page);
        await page.goto("/apps/files/");

        await page.click('.files-tb-primary');
        await page.fill('.files-mkdir-input', 'e2e-folder');
        await page.keyboard.press('Enter');
        await expect(page.locator('.files-name', { hasText: 'e2e-folder' })).toBeVisible();

        await page.click('.files-row:has-text("e2e-folder") .files-overflow-btn');
        await page.click('.files-overflow-item:has-text("Rename")');
        await page.fill('.files-rename-input', 'renamed');
        await page.keyboard.press('Enter');
        await expect(page.locator('.files-name', { hasText: 'renamed' })).toBeVisible();

        await page.click('.files-row:has-text("renamed") .files-overflow-btn');
        await page.click('.files-overflow-item:has-text("Delete")');
        await page.click('.files-modal-confirm');
        await expect(page.locator('.files-name', { hasText: 'renamed' })).toHaveCount(0);
    });

    test("listing shows file uploaded via WebDAV", async ({ page, request }) => {
        await loginInBrowser(page);
        const cookie = await login(request);
        const r = await request.fetch('/dav/files/admin/e2e-upload.txt', {
            method: 'PUT', headers: { cookie }, data: 'hello e2e',
        });
        expect(r.status()).toBe(201);
        await page.goto('/apps/files/');
        await expect(page.locator('.files-name', { hasText: 'e2e-upload.txt' })).toBeVisible();
        await request.fetch('/dav/files/admin/e2e-upload.txt', { method: 'DELETE', headers: { cookie } });
    });

    test("clicking a folder updates the URL", async ({ page, request }) => {
        await loginInBrowser(page);
        const cookie = await login(request);
        await request.fetch('/dav/files/admin/clickable/', { method: 'MKCOL', headers: { cookie } });
        await page.goto('/apps/files/');
        await page.click('.files-name-folder:has-text("clickable")');
        await expect(page).toHaveURL(/\/apps\/files\/clickable$/);
        await request.fetch('/dav/files/admin/clickable', { method: 'DELETE', headers: { cookie } });
    });

    test("multi-select + cut/paste moves files across folders", async ({ page, request }) => {
        await loginInBrowser(page);
        const cookie = await login(request);
        await request.fetch('/dav/files/admin/src-file.txt', { method: 'PUT', headers: { cookie }, data: 'x' });
        await request.fetch('/dav/files/admin/dest-dir/', { method: 'MKCOL', headers: { cookie } });
        await page.goto('/apps/files/');
        await page.click('.files-row:has-text("src-file.txt") input[type=checkbox]');
        await page.click('.files-chip-selection .files-chip-action:has-text("Cut")');
        await page.click('.files-name-folder:has-text("dest-dir")');
        await page.click('.files-chip-clipboard .files-chip-action:has-text("Paste here")');
        await expect(page.locator('.files-name', { hasText: 'src-file.txt' })).toBeVisible();
        await request.fetch('/dav/files/admin/dest-dir/src-file.txt', { method: 'DELETE', headers: { cookie } });
        await request.fetch('/dav/files/admin/dest-dir', { method: 'DELETE', headers: { cookie } });
    });

    test("reload preserves folder", async ({ page, request }) => {
        await loginInBrowser(page);
        const cookie = await login(request);
        await request.fetch('/dav/files/admin/persist-dir/', { method: 'MKCOL', headers: { cookie } });
        await page.goto('/apps/files/persist-dir');
        await page.reload();
        await expect(page).toHaveURL(/\/apps\/files\/persist-dir$/);
        await request.fetch('/dav/files/admin/persist-dir', { method: 'DELETE', headers: { cookie } });
    });
});
```

- [ ] **Step 2: Commit**

```
git add e2e/tests/files.spec.ts
git commit -m "sp6(f): playwright e2e for the files web UI"
```

### Task F2: README + changelog + memory + SP7 follow-up

**Files:**
- Modify: `README.md`
- Modify: `memory/project_rustcloud_program.md`
- Create: `docs/superpowers/specs/2026-05-12-files-web-ui-design.followup-sp7.md`

- [ ] **Step 1: README bullet**

In `README.md`, add a one-line entry under the workspace crate list for the files web UI: "Files web UI at `/apps/files/<path>` — browse / download / upload / rename / delete / cut-paste move. Server fns at `/api/files/*`."

- [ ] **Step 2: Memory update**

Edit `C:\Users\Matt Stone\.claude\projects\C--Users-Matt-Stone-git-rustcloud\memory\project_rustcloud_program.md` to mark SP6 done. Add a bullet under "## Status as of 2026-05-12" mirroring the SP5 entry:
```
- **Sub-project 6 (Files web UI):** done. New Dioxus catch-all
  route `/apps/files/<path>` rendering a top-bar + sidebar chrome with a
  tabular file list. Click-to-navigate folders, anchor-based downloads via
  the existing /dav/files/... WebDAV GET surface. Inline mkdir + rename;
  modal delete; multi-select via checkboxes; cut/paste move via a sticky
  clipboard chip. Uploads: single-PUT path for small files, chunked
  upload via the existing /dav/uploads/... endpoints for larger files,
  with a progress strip + Cancel/Retry. New `#[server]` fns at
  `/api/files/{list,mkdir,rename,delete,move,upload_begin}`. Playwright
  e2e covers the spec §18 scenarios. Next: SP4b-S3 (deferred) or SP7
  (sharing).
```

- [ ] **Step 3: Follow-up notes**

`docs/superpowers/specs/2026-05-12-files-web-ui-design.followup-sp7.md`:
```markdown
# Sub-project 6 follow-up — open notes feeding SP7 (Sharing)

Surfaced during SP6 implementation.

## Seams left open for sharing
- Sidebar reserves space for "Favorites", "Recent", "Shared with you",
  "Trash". Strings + routing land in SP7+.
- `FileEntry` DTO has no sharing fields yet. When sharing lands, extend
  with `shared_with: Option<Vec<String>>` + indicator flags rather than
  changing existing fields.
- The row ⋯ menu has two fixed entries (Rename, Delete). A "Share" entry
  plugs in as a third item; the menu component already supports a vector.

## Working primitives sharing can reuse
- The drop overlay's positioning + activation pattern can drive a
  "drag-to-share" flow.
- The clipboard chip pattern (sticky, navigation-preserving toolbar chip)
  can be reused as a "share clipboard" for bulk-share UX.
- The server-fn extractor `require_user()` already gates per-user
  authorization; share endpoints can reuse it.

## Known limitations
- No client-side pagination — list_dir returns everything. Shared folders
  with many entries may need pagination in SP7.
- Retry on a failed upload re-queues but cannot re-supply the file blob
  (browser security). Acceptable for MVP; revisit in SP7 if needed.
```

- [ ] **Step 4: Commit**

```
git add README.md docs/superpowers/specs/2026-05-12-files-web-ui-design.followup-sp7.md
git commit -m "sp6(f): README + followup-sp7 notes"
```

(Memory file is outside the repo — update it via the auto-memory system as a separate operation, not in this commit.)

### Task F3: PR + merge + memory flip

- [ ] **Step 1:** `git push -u origin sp6/f-tests-polish`
- [ ] **Step 2:** Open PR (summary: "Playwright e2e for files UI", "README + followup-sp7 notes").
- [ ] **Step 3:** Watch CI carefully — the e2e job is the meaningful one for this PR.
- [ ] **Step 4:** `gh pr merge --squash --delete-branch`.
- [ ] **Step 5:** Update `memory/project_rustcloud_program.md` per Task F2 Step 2 (this is outside the repo and must be persisted via the memory system).

---

## Self-review checklist

After Batch F merges, verify against the spec:

1. **Spec coverage** — each of spec §20's 10 acceptance criteria has a covering Playwright scenario or batch task. Notably:
   - §20.1-§20.3 covered by F1's auth-redirect + chrome tests.
   - §20.4 covered by F1's click-to-navigate + listing-shows-uploaded tests.
   - §20.5 covered by F1's mkdir/rename/delete round-trip.
   - §20.6 covered by Batch E + the e2e listing-shows-uploaded test.
   - §20.7 covered by F1's cut/paste test.
   - §20.8 covered by Batch B's skeleton/empty/error fragments + the load-error retry path.
   - §20.9: Playwright `files.spec.ts` lands in F1.
   - §20.10: workspace + multi-dialect tests stay green across all 6 batches.
2. **Placeholders** — none.
3. **Type consistency** — `FileEntry` is consistent across server fns, list, row, and tests.
4. **Frequent commits** — every task ends with a commit step.
5. **TDD-ish** — server fns get integration tests before being consumed by the UI; UI components are validated via WASM build + Playwright (Dioxus components don't have a unit-test culture in this codebase).

