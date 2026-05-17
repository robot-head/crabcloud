# Trash bin — Design (Sub-project 12)

**Status:** spec — design only, no implementation.
**Date:** 2026-05-16
**Sub-project:** 12. Picks up after the SP9–SP11 polish sweep landed (PRs #163–#167). Touches `crabcloud-fs::View`, `crabcloud-http::routes::dav`, OCS, the Dioxus files UI, and adds a new `crabcloud-trash` crate plus an `oc_files_trash` table.

## 1. Goal

Ship a Nextcloud-compatible trash bin so deleted files are recoverable for 30 days by default.

In MVP scope:

- New `crabcloud-trash` crate: `Trash::{soft_delete, list, restore, purge}`.
- New `oc_files_trash` table + migration (sqlite + mysql + postgres).
- `View::delete` reroutes to `Trash::soft_delete` for every authed surface (UI, DAV, OCS). Public-link DELETE explicitly opts out and hard-deletes.
- DAV `/dav/trashbin/{uid}/...` endpoint: PROPFIND (list), DELETE (purge), MOVE (restore). Plus `/remote.php/dav/trashbin/...` alias.
- OCS REST: `/ocs/v2.php/apps/files_trashbin/api/v1/trashbin` (list + restore + purge actions). Plus `#[server]` fn mirrors for the Dioxus UI.
- Files UI: "Deleted files" sidebar entry → trash view reusing the existing file-list chrome with `Restore` and `Delete permanently` actions.
- Age-based background sweeper (daily, default 30d retention, new `trash_retention_days` config knob).
- E2E + unit tests on every layer.

Explicitly out of scope (deferred):

- Size-pressure / quota-fraction purge policies (no quota system yet).
- Per-folder retention overrides.
- "Trashbin app" client-app config knobs (Nextcloud's `files_trashbin` admin settings).
- Restore-collision UI: server picks ` (restored)` / ` (restored 2)` suffixes; no client prompt.
- Group-folder / external-storage trash (need their own trash semantics — defer with the backends).
- Trash for incoming-shares-of-a-share (3rd-degree sharing).
- "Show originally-shared-by" attribution in the UI (the row stores it but the view doesn't surface it yet).
- Cross-user un-delete (admin recovering Alice's trash from Bob's bin).

## 2. Load-bearing decisions (brainstormed)

| # | Decision | Rationale |
|---|---|---|
| 1 | **Physical move on disk** to `<datadirectory>/<uid>/files_trashbin/files/<basename>.<suffix>` where `suffix = "d<unix_seconds>"`. Files and directories preserved as-is (directories restored recursively). | Matches Nextcloud byte-for-byte; desktop clients see the trashbin DAV surface and "just work". A DB-only soft-delete would require gating every storage `list` / `read` on a `deleted_at IS NULL` filter, which leaks the trash concept into every backend. |
| 2 | **New crate `crabcloud-trash`** holding the core `Trash` service. Same shape as `crabcloud-sharing`: depends on `crabcloud-db`, `crabcloud-storage`, `crabcloud-filecache`; no dependency on `crabcloud-fs` (`View::delete` reaches into `Trash` instead — same dependency direction `View → Trash`). | Keeps the FS crate thin. Multidialect SQL lives next to the table that owns it. |
| 3 | **`oc_files_trash` table** holds the metadata: `(id, user, basename, suffix, location, deleted_at, type, fileid_legacy)`. Indexed on `(user, deleted_at)` for list-by-most-recent-first. | Mirrors Nextcloud's columns; the (basename, suffix) pair gives the on-disk filename, location gives the original parent path, type lets the UI render folder vs file icons. |
| 4 | **Trashbin gets its own `oc_storages` row** keyed `trash::<uid>`, mounted at `<datadirectory>/<uid>/files_trashbin/`. So the filecache stays consistent and trash content is rescannable via the same scanner that handles user homes. | Avoids a "trash is invisible to the filecache" hole; trash entries have real fileids and storage paths, just under a different storage id. |
| 5 | **Suffix format `d<unix_seconds>`** (e.g. `report.pdf.d1716000000`). Two deletes of the same basename within one second collide; resolve by appending `_<n>` (`report.pdf.d1716000000_2`). | Nextcloud-compatible. Sub-second collisions are rare enough that the linear `_n` probe is fine. |
| 6 | **`View::delete(uid, path)` reroutes to `Trash::soft_delete`** for every authed surface. The existing `View::delete` signature returns `Result<(), FsError>` so callers don't notice the change. Public-link DELETE handlers call a new `View::hard_delete` (or pass an opt-out flag) that bypasses trash. | Single point of policy. Tests against existing `View::delete` callers automatically pick up the new behavior. Public-link surfaces explicitly opt out because anonymous users have no trash bin. |
| 7 | **Shared-with-me delete lands in the deleter's bin**: if Alice shares `/photos` with Bob and Bob deletes `/photos/cat.jpg`, the trash row's `user` column is `bob` (the deleter, derived from the authenticated `View`'s uid) and the on-disk file moves to `<datadir>/bob/files_trashbin/files/cat.jpg.d…`. Alice loses the file from her storage but can't restore it from her own trash. | Matches Nextcloud's `OC\Files\Trashbin::move2trash` behavior. Each user controls their own bin. |
| 8 | **Restore is MOVE-like**: take the trash row, locate the file on disk, move it back to `location/basename`. If `location/` doesn't exist, auto-create the parent chain via `View::mkdir`. If the destination already exists, suffix the restored name with ` (restored)`, then ` (restored 2)`, etc. The trash row is deleted on success; the on-disk file is gone (was renamed). | Auto-create matches user expectation ("undo the delete fully"). The ` (restored N)` collision strategy avoids silently overwriting work the user did after deleting. |
| 9 | **Background sweeper** `TrashSweeper` runs daily (24h sleep + cooperative shutdown via `Arc<Notify>`, matching the existing `ExpirationWarningSweeper` / `MailQueueCleanup` / `PreviewCacheCleanup` pattern). Selects rows where `deleted_at < now() - trash_retention_days * 86400`, deletes the on-disk files, deletes the rows. Always spawned (independent of mail transport). | Same shape as the recently-landed cleanup tasks; reuses the test ergonomics (`sweep_once()` for synchronous test drive). |
| 10 | **`trash_retention_days: u32`** config knob on `FileConfig`, default `30`. `0` disables retention sweeping (manual purge only). | Standard Nextcloud default. `0` is the operator escape hatch for compliance-driven retain-forever; explicitly documented. |
| 11 | **DAV surface `/dav/trashbin/{uid}/...`** mounted alongside `/dav/files/{uid}/...`. Inside the trashbin namespace: PROPFIND lists `oc_files_trash` rows as DAV resources; DELETE permanently purges; MOVE with `Destination: /dav/files/{uid}/<restore_path>` restores. POST/PUT/MKCOL/COPY return 405. | Nextcloud-compatible. Desktop clients (Nextcloud's, KIO) already speak this. The MOVE-with-Destination shape lets the client pick a non-default restore path if desired. |
| 12 | **OCS surface** `/ocs/v2.php/apps/files_trashbin/api/v1/trashbin` (Nextcloud spelling). `GET` lists, `POST /restore/{id}` restores, `DELETE /trash/{id}` purges, `DELETE /trash` purges all. JSON shape mirrors Nextcloud's. | Standard apps-API namespacing. Matching Nextcloud's URL shape means existing third-party clients keep working. |
| 13 | **Dioxus UI** adds a new sidebar entry "Deleted files" wired via the existing `chrome.rs` sidebar. Clicking renders a new `pages/trash.rs` that reuses the `files/list.rs` row/list components in read-mostly mode (no upload zone, no inline rename). Per-row actions: `Restore`, `Delete permanently`. Bulk action: `Empty trash`. | Reuses the chrome / row components so look-and-feel matches the main file view. The trash view is a constrained variant rather than a parallel implementation. |
| 14 | **Server-fn API** for the UI: `list_trash() -> Vec<TrashEntry>`, `restore_trash(id) -> Result<RestoredTo, …>`, `purge_trash(id) -> Result<()>`, `empty_trash() -> Result<u64>`. All gated by `AuthenticatedUser`. Implementation simply forwards to `Trash::*` after looking up the per-uid handle. | Server fns share the auth path with the rest of the UI; no per-request DAV round-trip from the browser. |

## 3. Architecture

```
DAV / OCS / Dioxus UI
 │
 ├─ DELETE /dav/files/<uid>/<path>           ┐
 ├─ DELETE /ocs/.../files/<path>             │  authed surfaces:
 ├─ files-page row × menu × delete           │  hit View::delete
 │   (server fn → View::delete)              ┘    │
 │                                                 ▼
 ├─ DELETE /s/<token>/<path>                  Trash::soft_delete(uid, path)
 │   public-link → View::hard_delete  ───┐         │
 │                                       │         ├─ LocalStorage.rename(home, trashbin)
 │                                       │         ├─ INSERT oc_files_trash
 │                                       │         └─ filecache rescan (trashbin storage)
 │                                       │
 └─ /dav/trashbin/<uid>/...               │
     ├─ PROPFIND → Trash::list(uid)       │
     ├─ DELETE   → Trash::purge(uid, id)  │
     └─ MOVE     → Trash::restore(uid, id, optional dest)
                                          │
                                          └─ hard delete: bytes gone, no trash row.

crabcloud-trash  (NEW crate)
 ├─ TrashEntry { id, user, basename, suffix, location, deleted_at, type, fileid_legacy }
 ├─ Trash::{soft_delete, list, restore, purge, sweep_expired}
 ├─ Multidialect SQL via match self.pool.as_ref() (sqlite/mysql/postgres)
 └─ Depends on: crabcloud-db, crabcloud-storage, crabcloud-filecache

TrashSweeper  (crates/crabcloud-core/src/trash_sweeper.rs)
 ├─ run(): 24h cooperative loop
 ├─ sweep_once() -> Result<u64>  (for tests)
 └─ Spawned unconditionally in AppStateBuilder::build()

AppState additions
 ├─ trash: Arc<crabcloud_trash::Trash>
 └─ trash_sweeper_shutdown: Arc<tokio::sync::Notify>
```

## 4. Schema

```sql
CREATE TABLE oc_files_trash (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,  -- BIGSERIAL on pg, BIGINT AUTO_INCREMENT on mysql
    "user"         VARCHAR(64)  NOT NULL,              -- deleter's uid; "user" is reserved in pg/mysql so always quoted
    basename       VARCHAR(255) NOT NULL,              -- "report.pdf"
    suffix         VARCHAR(32)  NOT NULL,              -- "d1716000000" (or "d1716000000_2" on collision)
    location       VARCHAR(512) NOT NULL,              -- original parent dir, e.g. "/projects/q1"; "/" for root
    deleted_at     BIGINT       NOT NULL,              -- unix seconds; matches the numeric portion of suffix
    type           VARCHAR(16)  NOT NULL,              -- "file" | "dir"
    fileid_legacy  BIGINT       NULL                   -- pre-delete oc_filecache.fileid (best-effort, for audit)
);

CREATE INDEX idx_trash_user_deleted ON oc_files_trash ("user", deleted_at);
CREATE UNIQUE INDEX idx_trash_user_name ON oc_files_trash ("user", basename, suffix);
```

The on-disk trashbin layout under `<datadirectory>/<uid>/files_trashbin/files/` is a flat namespace of `<basename>.<suffix>` entries — directories are stored as directories at the top level (so restoring a folder is a single rename, not a recursive replay).

A separate `oc_storages` row keyed `trash::<uid>` is registered for each user on first trash interaction; its `numeric_id` is what the trashbin's `oc_filecache` rows reference.

## 5. Surface contracts

### 5.1 DAV — `/dav/trashbin/{uid}/...` (and `/remote.php/dav/trashbin/...` alias)

Resources inside the trashbin namespace use the suffix-encoded filename as their href, e.g. `/dav/trashbin/{uid}/trash/report.pdf.d1716000000`. This matches Nextcloud's wire shape so desktop / KIO clients work without translation; internally the handler looks up the row by `(user, basename, suffix)` (the unique index).

| Method | Behavior |
|---|---|
| PROPFIND `/dav/trashbin/{uid}/` (Depth: 0, 1) | Lists trash root + entries. Each entry exposes `displayname` (= original basename, suffix stripped), `getlastmodified` (= `deleted_at`), `getcontentlength`, `resourcetype`, and the custom `{http://nextcloud.org/ns}trashbin-original-location` property holding `location/basename`. |
| PROPFIND `/dav/trashbin/{uid}/trash/<basename>.<suffix>` | Single entry detail. |
| DELETE `/dav/trashbin/{uid}/trash/<basename>.<suffix>` | Purges (deletes row + on-disk file). 204 on success, 404 if not found, 403 if not the deleter. |
| MOVE `/dav/trashbin/{uid}/trash/<basename>.<suffix>` with `Destination: /dav/files/{uid}/<path>` | Restores to the given destination (parent auto-created). Without an explicit destination, restores to original `location/basename`. 201 on restore, 409 on collision-after-suffix-exhaustion. |
| Anything else (PUT, POST, COPY, MKCOL, PROPPATCH, LOCK, UNLOCK) | 405. |

### 5.2 OCS — `/ocs/v2.php/apps/files_trashbin/api/v1/trashbin`

The OCS surface uses the row `id` as the resource handle (JSON-internal — easier than URL-encoding the suffix). DAV uses the suffix-encoded filename; OCS uses `id`. Both lookups go through the same `(user, id)` or `(user, basename, suffix)` paths in `Trash::*`.

| Method | Path | Behavior |
|---|---|---|
| GET | `/trashbin` | JSON list of `TrashEntry` for the authed user. |
| POST | `/restore/{id}` | Restore. Returns the restored full path. |
| DELETE | `/trash/{id}` | Purge a single entry. |
| DELETE | `/trash` | Empty the bin. Returns count purged. |

### 5.3 Server-fn API (Dioxus)

```rust
#[server]
pub async fn list_trash() -> Result<Vec<TrashEntryDto>, ServerFnError>;
#[server]
pub async fn restore_trash(id: i64) -> Result<RestoredTo, ServerFnError>;
#[server]
pub async fn purge_trash(id: i64) -> Result<(), ServerFnError>;
#[server]
pub async fn empty_trash() -> Result<u64, ServerFnError>;  // returns count purged
```

All four are gated by the existing `AuthenticatedUser` extractor / pattern.

## 6. Edge cases

| Case | Behavior |
|---|---|
| **Public-link DELETE** (anonymous) | Hard-delete; no trash row. Implemented by adding `View::hard_delete(uid, path)` and routing the public-link DAV/REST handlers through it. |
| **Shared-with-me delete** | Trash row's `user` = deleter (per the authed `View`). Original file moves from the share owner's storage to the deleter's trashbin. |
| **Restore with missing parent dir** | `View::mkdir_p`-equivalent recreates the chain. |
| **Restore with destination collision** | Suffix the restored name with ` (restored)`, then ` (restored 2)`, ... until the destination is free. Cap at 99; fail with 409 beyond that (effectively unreachable). |
| **Sub-second double-delete of same basename** | Trash suffix `_n` probe (`d…_2`, `d…_3`, ...) on disk; the `id` PK keeps DB rows unique. |
| **Trash retention = 0** | Sweeper skips its scan entirely. Operator escape hatch for compliance-driven retain-forever; logged at startup. |
| **Trash directory restore** | Single on-disk rename moves the whole tree back. Filecache rescan re-indexes the restored subtree under the user's home storage. |
| **Trashbin storage row missing** | Lazily created on first soft-delete (`Trash::ensure_storage(uid)` upserts the `oc_storages` row before the rename). |
| **Disk full during soft-delete** | Surface the rename `io::Error` as `FsError::Storage`; no trash row written; the original file stays in place. (Atomic: rename either succeeds fully or not at all.) |
| **Cross-storage soft-delete** (e.g. a file in an incoming share where the share owner uses a different backend) | MVP rejects with `FsError::CrossStorage` — clients see a 412 or 501 depending on surface. Out-of-scope to ship cross-storage trash semantics in this SP. |

## 7. Testing

- **`crabcloud-trash` unit**: round-trip via a fake storage + sqlite pool. Coverage: soft-delete writes row + moves bytes, list returns rows in `deleted_at DESC`, restore picks correct destination + suffixes on collision, purge deletes row + bytes, sub-second collision exercises `_n` suffix.
- **`crabcloud-core::trash_sweeper` e2e (sqlite)**: insert rows with stale + fresh `deleted_at`, call `sweep_once()`, assert stale rows + on-disk files gone, fresh rows + files remain.
- **`crabcloud-fs::view` e2e**: `View::delete` rerouted — soft-delete creates a trash row; `View::hard_delete` does not.
- **`crabcloud-http` DAV e2e**: PROPFIND lists, DELETE purges, MOVE restores. Test the `Destination: /dav/files/...` shape and the implicit-original-location fallback.
- **`crabcloud-http` OCS e2e**: GET/POST/DELETE shapes against a seeded user.
- **`crabcloud-app` server-fn integration test**: round-trip `list_trash` → `restore_trash` → file reappears under `/dav/files/{uid}/...`.
- **Shared-with-me cross-user test (`crabcloud-fs`)**: Alice shares `/photos` with Bob, Bob deletes `/photos/cat.jpg`, assert trash row belongs to `bob`, on-disk file is under `bob/files_trashbin/`.
- **Public-link DELETE bypass (`crabcloud-http`)**: anonymous DELETE via `/s/{token}/<path>` triggers hard delete; no trash row created.

## 8. Batches (implementation order)

A. **Core + storage** — new `crabcloud-trash` crate, migration `0009_files_trash`, `Trash::{soft_delete, list, restore, purge}`, `View::delete` rerouted + `View::hard_delete` added, `TrashSweeper` with `trash_retention_days` config knob, AppState wiring, unit + sweeper tests.
B. **DAV** — `/dav/trashbin/{uid}/...` (and `/remote.php/dav/trashbin/...`) router covering PROPFIND, DELETE, MOVE. Public-link DELETE handlers switched to `View::hard_delete`.
C. **OCS + server fns** — `/ocs/v2.php/apps/files_trashbin/api/v1/trashbin` REST endpoints; `list_trash` / `restore_trash` / `purge_trash` / `empty_trash` server fns; cross-crate parity test for the JSON shape vs Nextcloud.
D. **UI** — sidebar entry, `pages/trash.rs` view reusing `files/list.rs` components, restore + purge actions, empty-trash bulk action, SSR snapshot test.

Each batch ships as one PR through subagent-driven development with the standard two-stage review (spec compliance → code quality).
