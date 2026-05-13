# Files Web UI — Design (Sub-project 6)

**Status:** spec — design only, no implementation.
**Date:** 2026-05-12
**Sub-project:** 6 of ~13 (see `memory/project_rustcloud_program.md`).

## 1. Goal

Ship the first browser-facing app in Crabcloud: a Dioxus Files page that lets a signed-in user browse, read, and write their files in the home storage. After this sub-project the web UI reaches feature parity with desktop/iOS/Android clients for the common-case "I want to look at and edit my files" workflow — no sharing, no favorites, no trash recovery (those are post-MVP).

MVP scope is **Browse + read + write only**:

- Single-column folder list with breadcrumb navigation.
- Click folder → navigate into it. Click file → browser downloads it.
- Drag-drop *and* Upload button (chunked uploads for large files).
- Inline rename, inline new-folder.
- Delete with confirmation modal (single or multi).
- Cut/paste move (clipboard persists across folder navigation).
- Multi-select via checkboxes.

Explicit out-of-scope for SP6: sharing UI, public links, favorites, recent view, shared-with-you view, trash/restore, file versions, file preview pane, file editor integration, search, drag-to-move, public upload pages.

## 2. Load-bearing decisions (brainstormed)

| # | Decision | Rationale |
|---|---|---|
| 1 | **Backend RPC = Dioxus `#[server]` functions** wrapping `View`/`Uploads` for metadata ops (list, mkdir, rename, delete, move). | Mirrors existing login pattern. Type-safe Rust-to-Rust. The `View` façade is what the WebDAV layer also calls, so the underlying surface is identical. |
| 2 | **Downloads use a plain anchor link** to `/dav/files/<user>/<path>` (browser-native streaming, cookie auth). | Avoids forcing file bytes through the server-fn channel. Reuses SP5's GET handler verbatim. Triggers browser download/inline-view by content-type. |
| 3 | **Uploads use existing `/dav/uploads/<user>/<id>/...` chunked endpoints via `fetch()`** from the client. | Same protocol the Nextcloud desktop client uses (SP5 already implements it). Keeps server-fn channels out of the bulk-byte path. PUT-then-MOVE state machine is in the browser, but it's the same one the desktop client runs against the same endpoints. |
| 4 | **URL prefix = `/apps/files/<path>`** (catch-all Dioxus route under `Route::FilesRoute`). | Matches Nextcloud's `apps/<appid>/` convention. Sets the pattern future apps (calendar, contacts) follow. Folder URLs are bookmarkable, browser back/forward works, copy-paste links work. |
| 5 | **Server-side auth redirect** to `/index.php/login?redirect_url=<original>` when the session cookie is missing. | SSR check before render. Matches Nextcloud. Lands the user back on the folder they wanted after they log in. |
| 6 | **Data fetching = `use_resource` on a path signal** with a refresh trigger after mutations. | Dioxus-idiomatic. No client-side cache layer for MVP. Mutations call the server fn then bump the refresh signal. |
| 7 | **Page chrome = top bar + left sidebar** (Nextcloud-style). | Sidebar is mostly empty in MVP (just "All files") but reserves space for Favorites / Recent / Shared / Trash that all land post-MVP. |
| 8 | **File row anatomy = checkbox + name + size + modified + hover-revealed action icons + ⋯ menu** (Nextcloud-style). | Multi-select via checkbox column is the discoverable, touch-friendly path. Hover icons surface common actions (rename, delete, cut); ⋯ menu has the full set. |
| 9 | **Upload UX = Upload button + drop-anywhere overlay + inline progress strip** below the toolbar. | Drop overlay covers the file-list area while the user is dragging files over the window. Progress strip persists below the toolbar across navigation. |
| 10 | **Inline editing for rename + new folder; modal confirm for delete.** | Rename/mkdir match Finder/Explorer feel (click name → input replaces it; Enter commits, Escape cancels). Delete keeps a modal because it's destructive and MVP has no trash. |
| 11 | **Persistent toolbar with selection chip + clipboard chip.** | Default toolbar always visible. When ≥1 item is checked, a compact "N selected · cut · delete · ✕" chip joins the toolbar. After a cut, a "✂ N on clipboard · Paste · ✕" chip persists across folder navigation until paste/clear. |
| 12 | **Empty / loading / error states = skeleton rows for loading, illustrated empty state, retry-button error state.** | Skeleton rows match the SSR-then-hydrate model cleanly (SSR can render skeleton, client swaps real rows in). |

## 3. Architecture

```
Browser
 ├─ /apps/files/<path>                                  ← Dioxus page (SSR + hydrate)
 │   ├─ <a href="/dav/files/<user>/<path>">name</a>     ← download (cookie auth)
 │   ├─ POST /api/files/list, /mkdir, /rename, ...      ← #[server] fns
 │   │   wrap AppState::view_for(uid).list/mkdir/...
 │   └─ fetch() PUT /dav/uploads/<user>/<id>/<n>        ← chunked upload
 │           MOVE  /dav/uploads/<user>/<id>/.file       ← commit
 │
Server
 ├─ axum router
 │   ├─ /dav/files/*        ← SP5 WebDAV (existing)
 │   ├─ /dav/uploads/*      ← SP5 chunked uploads (existing)
 │   ├─ /apps/files/*       ← new Dioxus catch-all
 │   └─ /api/files/...      ← new server-fn endpoints
 ├─ AuthLayer (SP2b) gates everything
 └─ AppState::view_for(uid) → View(façade over storage/filecache from SP4)
```

The Files page is the only consumer of the new `/api/files/...` server fns. The server fns are thin: each one extracts the session user via `FullstackContext`, calls `AppState::view_for(uid)` or `uploads_for(uid)`, and translates the result into a serde-friendly DTO. The `View` façade already enforces user-relative paths and authorization, so the server fn doesn't add policy.

## 4. Routes & wiring

### 4.1 Dioxus route

Add to `crates/crabcloud-ui/src/app.rs`:

```rust
#[route("/apps/files/:..segments")]
FilesRoute { segments: Vec<String> },
```

`segments` is collected, joined by `/`, and rebuilt into a `UserPath` (leading `/`, empty → "/"). The `FilesRoute` component reads the segments, normalizes them, and constructs the page.

### 4.2 SSR auth check

`server::current_request_context()` already exposes the authenticated user (or `None`). The Files page's server-only branch checks: if anonymous, call `FullstackContext::commit_http_status(303, Some(("location", "/index.php/login?redirect_url=<encoded>")))` and return an empty fragment. Encoding uses `url::form_urlencoded`. The browser never sees the page shell for anonymous requests.

### 4.3 Server functions

All under `crates/crabcloud-ui/src/server_fns.rs` (or split into `server_fns/files.rs` if file grows past ~400 lines). Endpoints land at `/api/files/<op>` (POST except `list` which is GET for cacheability of headers/CORS, though it is not cached). CSRF: same `requesttoken` meta + `OCS-APIRequest` header pattern the rest of the UI already uses.

```rust
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FileEntry {
    pub name: String,           // leaf name only
    pub path: String,           // full UserPath, e.g. "/photos/cat.jpg"
    pub is_dir: bool,
    pub size: u64,              // 0 for dirs (or recursive size? — see §10)
    pub mtime_ms: i64,
    pub mime: Option<String>,   // None for dirs
    pub etag: String,
}

#[get("/api/files/list")]
pub async fn list_dir(path: String) -> Result<Vec<FileEntry>, ServerFnError>;

#[server(endpoint = "api/files/mkdir", prefix = "")]
pub async fn mkdir(path: String) -> Result<FileEntry, ServerFnError>;

#[server(endpoint = "api/files/rename", prefix = "")]
pub async fn rename(from: String, to: String) -> Result<FileEntry, ServerFnError>;

#[server(endpoint = "api/files/delete", prefix = "")]
pub async fn delete(paths: Vec<String>) -> Result<(), ServerFnError>;

#[server(endpoint = "api/files/move", prefix = "")]
pub async fn move_paths(paths: Vec<String>, dest_dir: String)
    -> Result<Vec<FileEntry>, ServerFnError>;   // returns new entries
```

Each fn:

1. Pulls `AppState` + session user from `FullstackContext`.
2. If anonymous → `ServerFnError::new("unauthorized")` (maps to 401 in the response).
3. Calls the matching `View` method on `state.view_for(&uid)`.
4. Errors map: `NotFound` → 404, `AlreadyExists` → 409, `PermissionDenied` → 403, others → 500.

Uploads do **not** go through server fns. The browser calls the WebDAV chunked endpoints directly (§7).

## 5. UI module layout

```
crates/crabcloud-ui/src/pages/files/
├── mod.rs              — FilesRoute + Files page + signal definitions
├── path.rs             — UserPath parsing helpers (segments ↔ "/a/b")
├── toolbar.rs          — New / Upload buttons + Selection chip + Clipboard chip
├── breadcrumb.rs       — Home › photos › vacation
├── list.rs             — FileList: header row + body + skeleton + empty
├── row.rs              — FileRow (checkbox, icon, name/RenameInput, size, mtime, ⋯ menu)
├── mkdir_row.rs        — Inline "New folder" row (input + commit/cancel)
├── delete_modal.rs     — Centered modal, single-or-many
├── upload.rs           — Drop-zone overlay + Upload button + chunked uploader
├── progress_strip.rs   — In-progress + queued uploads bar
└── states.rs           — EmptyFolder / LoadError / Skeleton fragments
```

Existing `pages/` is flat (`login.rs`, `login_v2_flow.rs`); SP6 introduces the first subdirectory page. We follow this pattern for future apps.

## 6. State model

State lives on the `Files` page component, exposed to children via `use_context`.

```rust
#[derive(Clone)]
struct FilesState {
    path:        Signal<String>,         // current folder, e.g. "/photos"
    refresh:     Signal<u64>,            // bump after mutations
    entries:     Resource<Result<Vec<FileEntry>, ServerFnError>>,
    selection:   Signal<HashSet<String>>,// selected full paths (leaf only in current view)
    clipboard:   Signal<Option<Clipboard>>,
    rename_target: Signal<Option<String>>, // path being inline-edited (None=no edit)
    delete_target: Signal<Option<Vec<String>>>, // paths to delete (drives modal)
    mkdir_active: Signal<bool>,          // inline new-folder row shown?
    uploads:     Signal<UploadQueue>,
    drag_active: Signal<bool>,           // drop-overlay visibility
}

#[derive(Clone)]
struct Clipboard {
    source_dir: String,     // for the chip's "from X" label
    paths:      Vec<String>,
}

#[derive(Clone, Default)]
struct UploadQueue {
    in_progress: Vec<UploadJob>,
    queued:      Vec<UploadJob>,
    completed:   Vec<UploadJob>,    // shown briefly then GC'd
}
```

`entries` is a `use_resource` keyed on `(path(), refresh())`. Mutations bump `refresh` so the list re-fetches. The mutation server fns also return the new/updated entry, so the UI can optimistically update before the refresh lands (optional polish — MVP can rely on the refresh).

Changing folder clears `selection` but **not** `clipboard` (the cut-paste pattern depends on the clipboard surviving navigation).

## 7. Uploads

### 7.1 Picking files

`<input type="file" multiple hidden>` triggered by the Upload button + drop event handler on the `FileList` container. Both paths produce a `FileList` JS handle which we iterate.

### 7.2 Small files (single PUT)

If `file.size <= cfg.upload.single_put_max_bytes` (default 8 MiB), the browser does a single:

```
PUT /dav/files/<user>/<dest>/<filename>
Content-Type: application/octet-stream
X-CSRFToken: <token>          ← already injected for cookie-auth requests
body: file bytes (streamed via fetch's Request body)
```

On 201/204 we bump `refresh`. On 409 (overwrite + If-Match mismatch) we surface a toast.

### 7.3 Large files (chunked)

For files above the threshold:

1. **Begin**: `MKCOL /dav/uploads/<user>/<id>` where `<id>` is the opaque `upload_id` returned by `Uploads::begin` — the browser calls a new tiny server fn `upload_begin(dest_path)` to get this id (the id encodes `dest_path` itself; storage-side state is just whatever the multipart backend tracks). Smaller surface than asking the browser to construct the opaque id.
2. **Parts**: `PUT /dav/uploads/<user>/<id>/<n>` for n = 1..N, with body = a slice of the file. The `ETag` returned by each PUT is the `PartTag` value the commit needs. Default part size = 16 MiB. Parts run with concurrency cap (default 4).
3. **Commit**: `MOVE /dav/uploads/<user>/<id>/.file` with `Destination: /dav/files/<user>/<dest>` and `X-Crabcloud-Part-Tags: ["etag1","etag2",...]`.
4. **Cancel**: `DELETE /dav/uploads/<user>/<id>`.

The browser also has the option to call `MKCOL` directly with the id we returned, but going through `upload_begin` lets us evolve the opaque format without breaking the client.

`fetch()` reports byte progress via the request body stream's `ReadableStream` (or, where unsupported, per-part granularity which is good enough). Progress strip shows percentage per active job + count of queued.

### 7.4 Drop zone

The Files page registers `dragenter`/`dragover`/`dragleave`/`drop` on the `FileList` container. While `drag_active`, an overlay covers the list with "Drop to upload to <folder>". Dropping queues uploads to the current folder.

### 7.5 Failures

A failed upload (any HTTP error or network drop) moves the job to a "failed" lane in the progress strip with a Retry button. No automatic retry in MVP.

## 8. Downloads

The file row's name cell is an `<a>` with `href="/dav/files/<user>/<path>"` and **no** `target="_blank"`. Click-through navigates the browser to that URL; the existing `Content-Disposition: attachment` header from SP5 (or the inferred inline disposition for images/PDFs) tells the browser whether to save or render. Cookie auth flows automatically.

The anchor's click handler stops propagation so the row's row-level click handler (folder navigation) doesn't fire for files.

Folder rows are also clickable but as `<button>` (no href) that pushes the new path into the route via Dioxus's `navigator()`.

## 9. Inline rename & inline mkdir

### 9.1 Rename

`rename_target` holds the path being edited. The file row renders the name as either a `<span>` or an `<input>` based on whether `rename_target == row.path`. Triggers:

- ⋯ menu → "Rename" sets `rename_target = Some(row.path)` and focuses the input.
- F2 key while the row is hovered (nice-to-have, post-MVP).

Commit (Enter / blur): call `rename(from, to)` server fn. On success: clear `rename_target`, bump `refresh`. On 409 (name conflict): keep the input open, show inline red helper text.

Cancel (Escape): clear `rename_target`.

### 9.2 Mkdir

`mkdir_active` shows a synthetic row at the top of the file list with `<input value="New folder">` focused and selected. Commit / cancel mirrors rename. Server fn returns the new `FileEntry`, which we splice into the list as an optimistic update.

## 10. PROPFIND vs server-fn data shape

`View::list` already returns the metadata we need. The `FileEntry` DTO maps:

| FileEntry field | View source |
|---|---|
| `name` | basename of `DirEntry.path` |
| `path` | `DirEntry.path` (full UserPath) |
| `is_dir` | `DirEntry.metadata.is_dir` |
| `size` | `DirEntry.metadata.size` (already accumulated for dirs by filecache) |
| `mtime_ms` | `DirEntry.metadata.mtime` → ms |
| `mime` | `DirEntry.metadata.mime` (None for dirs) |
| `etag` | `DirEntry.metadata.etag` |

Sort order: directories first, then files, both alphabetic (case-insensitive). MVP has no column sort UI.

## 11. Multi-select & clipboard

### 11.1 Multi-select

Selection state is `HashSet<String>` keyed on full path. Triggers:

- Checkbox click → toggle.
- Row click (anywhere outside name/checkbox) → no-op in MVP (no single-click-to-select).
- Header checkbox → toggle all in current view.
- Shift-click on a row's checkbox → range select between last-clicked and this one (nice-to-have).

Selection clears on path change.

### 11.2 Cut/paste

Cut (selection chip's ✂ button, or Ctrl/Cmd+X):

```rust
clipboard.set(Some(Clipboard {
    source_dir: path(),
    paths: selection().iter().cloned().collect(),
}));
selection.set(Default::default());
```

The clipboard chip ("✂ 3 on clipboard from photos · Paste · ✕") joins the toolbar. It persists across folder navigation.

Paste (clipboard chip's Paste button, or Ctrl/Cmd+V): call `move_paths(clipboard.paths, current_dir)`, clear clipboard, bump refresh. If the destination is the same as the source, the chip's Paste button is disabled (no-op).

There is no copy/duplicate in MVP — only cut/move.

## 12. Delete

Triggers:

- Row ⋯ → Delete → `delete_target = Some(vec![row.path])`.
- Selection chip → Delete → `delete_target = Some(selection().iter().cloned().collect())`.

The modal shows the count + first few paths, with a destructive button. Confirm calls `delete(paths)`, clears `delete_target` + `selection`, bumps refresh.

Errors (any path fails — e.g. permission denied) surface as a toast with the failing path; partial successes are kept (the server-side delete is per-path and non-transactional).

## 13. Empty / loading / error

- **Loading**: skeleton rows (4 of them) inside the file list while `entries.read()` is `None`. Same row height as real rows.
- **Empty folder**: 📂 icon + "This folder is empty" + "Drop files here, or click Upload above." Drop overlay still activates on dragenter.
- **Load error**: ⚠️ + "Couldn't load this folder" + a one-line server-supplied reason + Retry button. Retry bumps `refresh`.
- **Drop overlay**: covers the file-list area only (not the sidebar/toolbar) with a dashed blue border and "Drop to upload to <folder>".

## 14. Auth & CSRF

- The `/apps/files/...` SSR path is gated by the same `AuthLayer` (SP2b) that protects every cookie-auth route.
- Server fns are gated by `AuthLayer` *and* by the existing CSRF check (request token meta + `OCS-APIRequest` header). The Files page emits `<meta name="requesttoken">` like other pages.
- WebDAV calls from the browser (`/dav/files/...`, `/dav/uploads/...`) carry both the session cookie and `OCS-APIRequest: true` so the CSRF check passes. SP5 already accepts those.

## 15. Routing & navigation details

- `FilesRoute { segments: vec![] }` → folder `/`.
- `FilesRoute { segments: vec!["photos", "vacation"] }` → folder `/photos/vacation`.
- Inside the page, navigating into a subfolder uses `navigator().push(Route::FilesRoute { segments: <new> })`. The breadcrumb's "Home" link goes to `Route::FilesRoute { segments: vec![] }`.
- Browser back/forward works via Dioxus's history integration.
- Reload-keeps-folder: comes for free from path-in-URL.

## 16. Internationalization

All user-visible strings go through the existing `t!` macro (set up in SP1's i18n crate). No new locale files are added in SP6 — strings ship in English; translations land later. The strings are enumerated in `crates/crabcloud-ui/src/pages/files/strings.rs` so the i18n extractor can find them.

## 17. Telemetry / errors

- Each server fn emits a `tracing` span at INFO with `uid` + `op` + `path` (no body), plus an ERROR on storage failures (the lower layer already logs; we add the request-side context).
- Frontend errors that can't recover (e.g. "rename failed: name exists") surface as toasts. Toast infrastructure ships in SP6 as a small `toast.rs` module under `pages/files/`; we'll generalize and move it out of `pages/files/` if and when another page needs toasts.

## 18. Testing strategy

**Unit / integration (Rust):**

- `crates/crabcloud-ui/tests/server_fns_files.rs` — drives each server fn against a `make_state_with_user`-built `AppState` (sqlite, scanner disabled per the established workaround). Covers list, mkdir, rename, delete, move, error mapping (NotFound/AlreadyExists/PermissionDenied → status codes).
- `crates/crabcloud-ui/src/pages/files/path.rs` — unit tests for segments↔UserPath round-trip.

**Playwright e2e** (`tests/playwright/files.spec.ts`):

1. Login, hit `/apps/files/` → see file list seeded by the fixture.
2. Navigate into a subfolder via click; URL updates.
3. Reload → land back on the same folder.
4. Mkdir inline; new folder appears.
5. Rename inline; row updates.
6. Upload a small file (<8 MiB) via the Upload button; appears in the list.
7. Upload a large file (>16 MiB; fixture generates one) via drag-drop; chunked endpoints used; appears in the list.
8. Multi-select two files; chip appears; cut; navigate to subfolder; paste; files appear in dest, gone from source.
9. Delete a file via modal; row gone.
10. Hit `/apps/files/` while logged out → redirected to `/index.php/login?redirect_url=/apps/files/`; logging in lands back on `/apps/files/`.

E2e runs in the existing CI `e2e` job. Test data is seeded by the existing fixture-bootstrap step.

**Out-of-scope for SP6 tests:** dragging files into the browser (Playwright supports `setInputFiles` for the button path; drag-drop is exercised manually).

## 19. Batches (estimate ~6)

This is the brainstorm-time estimate; the implementation plan locks the exact split.

- **A — Routing & chrome.** Add `FilesRoute` catch-all, SSR auth redirect, top-bar + sidebar shell, empty/loading/error fragments. No server fns yet (page renders a hardcoded "coming soon" list).
- **B — Browse + download.** `list_dir` server fn + `FileList`/`FileRow`/`Breadcrumb` + click-to-navigate folders + anchor-based downloads + skeleton/empty/error states. URL drives folder.
- **C — Mkdir + rename + delete.** Inline `mkdir_row`, inline rename input on `FileRow`, delete modal, server fns for each, error mapping.
- **D — Multi-select + cut/paste move.** Selection state, selection chip, clipboard chip, `move_paths` server fn, navigation-preserving clipboard.
- **E — Uploads.** `upload_begin` thin server fn, drop overlay, Upload button + hidden file input, single-PUT path, chunked-upload state machine, progress strip with cancel/retry.
- **F — Tests + acceptance + polish.** Playwright e2e (§18 scenarios), `tests/server_fns_files.rs` integration tests, README workspace bullet for the files UI, sub-project changelog, screenshots in the spec follow-up.

## 20. Acceptance criteria

After SP6, all of the following are true:

1. Signed-in user can navigate to `/apps/files/` and see their home folder.
2. URL reflects the current folder; reload preserves it; browser back/forward works.
3. Anonymous user is redirected to `/index.php/login?redirect_url=...` with the redirect honored after login.
4. Clicking a folder navigates into it; clicking a file downloads it (or renders inline per content-type) via the WebDAV GET path.
5. Inline "New folder" creates a directory; inline rename renames a file or folder; delete-with-confirm removes one or many items.
6. Drag-drop and Upload button both accept small and large files; chunked uploads use `/dav/uploads/...`; progress shows for each job.
7. Multi-select via checkboxes; cut+paste moves items across folders; clipboard persists across navigation until pasted or cleared.
8. Empty, loading, and error states render per §13.
9. All e2e scenarios in §18 pass on the CI `e2e` job.
10. `cargo test --workspace` stays green on SQLite, and the multi-dialect migration suite stays green on MySQL/Postgres (no new migrations in SP6).

## 21. Out of scope (post-MVP, tracked for SP7+)

Sharing UI, public link shares, federated shares, favorites + favorites view, recent view, shared-with-you view, trash + restore, file versions, file preview pane, in-browser editor integration, search, drag-to-move (within UI), public upload pages, file activity feed, conflict resolution UI for sync, custom column visibility, column sort UI, file-tag UI.

A separate follow-up note will land at `docs/superpowers/specs/2026-05-12-files-web-ui-design.followup-sp7.md` capturing anything we surface during SP6 that should inform sharing's design.
