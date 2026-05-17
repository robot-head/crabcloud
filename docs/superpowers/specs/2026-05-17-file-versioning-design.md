# File versioning — Design (Sub-project 13)

**Status:** spec — design only, no implementation.
**Date:** 2026-05-17
**Sub-project:** 13. Picks up after SP12 trash bin landed (PRs #168–#175). Touches `crabcloud-fs::View` (the write paths), `crabcloud-trash` (cascade-purge on hard delete), `crabcloud-http::routes::dav` + `routes::ocs`, the Dioxus files UI, and adds a new `crabcloud-versions` crate plus an `oc_files_versions` table.

## 1. Goal

Ship Nextcloud-compatible file versioning: every byte-changing write (PUT / MOVE-overwrite) of a non-empty file snapshots the prior contents to the owner's `files_versions/` tree, with a tiered-retention background sweeper, restore (lossless — snapshot-then-replace), and DAV + OCS + UI surfaces.

In MVP scope:

- New `crabcloud-versions` crate: `Versions::{snapshot_if_needed, list_for, restore, delete, sweep_tiered, purge_for_fileid}`.
- New `oc_files_versions` table + migration `0010_files_versions` (sqlite + mysql + postgres triplet).
- Hooks in `View::write_file` and `View::move_with_overwrite`: before bytes change, snapshot the existing file if size > 0, throttle window has elapsed, and size cap not exceeded.
- Tiered retention sweeper (every / hourly / daily / weekly), daily cadence, mirrors the `TrashSweeper` shape.
- DAV `/dav/versions/{uid}/{fileid}/...` endpoint (and `/remote.php/dav/versions/...` alias): PROPFIND lists, GET downloads, COPY restores.
- OCS REST: `/ocs/v2.php/apps/files_versions/api/v1/versions/{fileid}` list / restore / delete.
- Server fns: `list_versions(fileid)`, `restore_version(version_id)`, `delete_version(version_id)`.
- Files UI: per-row "Versions" action opens a panel/modal with restore + delete per version.
- Trash cascade: `Trash::purge_entry` calls `Versions::purge_for_fileid(fileid_legacy)` so versions of hard-deleted files are reclaimed immediately. Public-link DELETE / sweeper-expired purges also cascade.
- E2E + unit tests on every layer.

Explicitly out of scope (deferred):

- Editor attribution per version (who wrote v5). Add a `created_by` column later when we have a clear UX for it.
- Content-addressed dedup. Storage cost = sum of all versions; the tiered sweeper + size cap keep growth bounded.
- Delta storage (xdelta / etc).
- Cross-storage versioning for incoming shares whose owner uses a non-local backend (S3-only owner storage). Local-first per the SP design.
- Group-folder version semantics.
- "All versions" global sidebar view (the per-file panel covers MVP).
- Versions-count toward quota (no quota system yet).
- Operator override of retention buckets (the schedule is hardcoded in MVP; one `versions_retention_disabled: bool` escape hatch only).
- Restore-into-a-different-path. Restore always replaces in place.
- Version naming / tagging / pinning ("don't auto-purge this version").

## 2. Load-bearing decisions (brainstormed)

| # | Decision | Rationale |
|---|---|---|
| 1 | **Physical copy on disk** to `<datadir>/<uid>/files_versions/<relative path>.v<mtime_unix_secs>`. Files only (directories aren't versioned). Same directory structure inside `files_versions/` as inside `files/`. | Matches Nextcloud byte-for-byte; desktop clients see versions at the standard DAV endpoint with no translation. Avoids the complexity of a content store + ref-counting. |
| 2 | **New crate `crabcloud-versions`** holding `Versions` (CRUD over `oc_files_versions` + on-disk operations). Depends on `crabcloud-db`, `crabcloud-storage`, `crabcloud-filecache`. Same shape as `crabcloud-trash`. | Single-responsibility; multidialect SQL lives next to the table that owns it; `crabcloud-fs::View` calls in. |
| 3 | **`oc_files_versions`** columns: `(id, storage_id, fileid, user, path, version_mtime, size)`. Indexes on `(user, fileid)` (per-file lookup), `(user, version_mtime)` (sweeper sort), `(storage_id, fileid)` (purge-on-trash cascade). | `fileid` is the join key for "all versions of this file"; `path` is for display + on-disk lookup; `version_mtime` is the suffix; `user` is the owner uid (redundant with storage_id, but cheaper than joining to `oc_storages` on every list). |
| 4 | **Snapshot trigger in `View`**: before any byte-changing write (`write_file`, `move_with_overwrite`), call `Versions::snapshot_if_needed(uid, path, current_row_in_filecache)`. Skip if: source is missing, size is 0, size > `versions_max_bytes` (default 1 GiB), or last version row for the same `(storage_id, fileid)` has `version_mtime > now - versions_min_interval_secs` (default 2s). Otherwise copy on-disk bytes to versions tree, INSERT row, return. | Same single point of policy as the trash reroute. Throttle catches autosave-heavy editors without losing meaningful diffs. Size cap prevents one 50 GiB write from blowing the trash + versions budget simultaneously. |
| 5 | **Restore is snapshot-then-replace**: `restore(version_id)` snapshots current to a NEW version row, then copies the target version's bytes over current. Lossless — undoing a restore is itself a restore. The version being restored stays in the versions list. | Matches Nextcloud's `Storage::restoreVersion` behavior. The "restore by accident" footgun is closed by the implicit pre-restore snapshot. |
| 6 | **Shared-file edit lands in the OWNER's versions table**. Bob editing a file Alice shared with him triggers `snapshot_if_needed` against Alice's storage; row written with `user='alice'`. Only Alice's versions panel surfaces the history. | Matches Nextcloud's `Storage_Versions::getVersionsList` ownership model. Avoids fan-out and "Bob's versions of Alice's file" confusion. Bob still benefits because his own next edit creates a version Alice can roll back. |
| 7 | **Public-link writes also create owner-side versions**. An anonymous upload that overwrites a shared file snapshots the prior contents into the share owner's versions table. No editor attribution recorded in MVP. | Same logic — bytes change on the owner's storage; the owner's history records the change. The "who did it" question gets answered when we add `created_by` later. |
| 8 | **Tiered retention sweeper** runs daily. For each `(user, fileid)` group, walks versions newest-first and keeps one per bucket: 0–24h every version, 24h–30d one per hour, 30d–180d one per day, 180d+ one per week. Implementation: SQL `ORDER BY version_mtime DESC`, walk, classify into a (bucket, slot) pair, drop if that slot is already filled. | Matches Nextcloud's `getExpireList`. Daily cadence (versus hourly) is fine because the trigger throttle already coalesces rapid writes; oldest-bucket eviction is the slow path. |
| 9 | **`versions_retention_disabled: bool` escape hatch** (default false). When true the sweeper short-circuits (returns Ok(0)) for compliance retain-forever deployments. The bucket thresholds are NOT operator-configurable in MVP. | Bucket-tunable retention is a niche feature; add when a real operator asks. The disable knob is the standard compliance pattern. |
| 10 | **Trash cascade**: `Trash::purge_entry` (in `crabcloud-trash`) calls `Versions::purge_for_fileid(fileid_legacy)` so versions of hard-deleted files are reclaimed immediately. This adds a `crabcloud-versions` dep on `crabcloud-trash` (dep direction: `trash → versions`; versions doesn't need trash, so no cycle). The Trash sweeper's `sweep_expired` path goes through the same `purge_entry` so age-expired trash also cascades. | Predictable storage cleanup. An alternative orphan-versions sweep (loosely coupled) was considered but rejected: it leaves stale bytes lying around for up to a day after the underlying file is purged. Tight coupling is the better trade for an MVP. |
| 11 | **MOVE rename (no overwrite) does NOT touch versions on disk** — versions stay attached to `fileid`. The on-disk versions path includes the relative path *at snapshot time*. After a rename, the database lookup is still by `fileid` so the panel keeps working; the on-disk path is just a historical artefact, never user-visible. | Avoids a recursive rename-versions-too operation. Trade-off: a long rename history leaves a maze of `files_versions/old/path/...` directories. Acceptable; the daily sweeper still expires them; future cleanup can prune empty dirs. |
| 12 | **DAV surface `/dav/versions/{uid}/{fileid}/...`** mounted alongside `/dav/trashbin/{uid}/...`. Inside the versions namespace: PROPFIND lists `oc_files_versions` rows for `fileid` as DAV resources; GET on a specific `<version_mtime>` returns the bytes; COPY with `Destination: /dav/files/{uid}/<current_path>` restores (`MOVE` would imply moving the version out of history — wrong shape; use COPY to mean "copy this version into the current path"). 405 otherwise. | Nextcloud-compatible spelling. The `{fileid}` segment is the same DB-allocated id used everywhere else (filecache, OCS); clients first resolve a path to a fileid via PROPFIND on `/dav/files/...`, then use it here. |
| 13 | **OCS surface** `/ocs/v2.php/apps/files_versions/api/v1/versions/{fileid}` (Nextcloud spelling). `GET` lists, `POST /restore/{version_id}` restores, `DELETE /version/{version_id}` deletes. JSON shape mirrors Nextcloud's. | Standard apps-API namespacing. Lets third-party Nextcloud clients keep working. |
| 14 | **Dioxus UI** adds a per-row "Versions" item to the file row's `…` menu. Clicking opens a `VersionsPanel` modal (reuses the `.files-modal-*` chrome) showing the version list with Restore + Delete buttons per row + the file size + the relative timestamp. Bulk actions deferred. | Minimal UI surface; matches the visual rhythm of trash + share modals. Side panel was considered but adds a layout primitive the page doesn't have yet. |
| 15 | **Server-fn API**: `list_versions(fileid) -> Vec<VersionDto>`, `restore_version(version_id) -> ()`, `delete_version(version_id) -> ()`. All gated by `AuthenticatedUser`; the service checks that the authed uid owns the underlying file (or has update permission via a share). | Server fns share the auth path with the rest of the UI; no per-request DAV round-trip from the browser. |

## 3. Architecture

```
Dioxus UI / DAV / OCS
 │
 ├─ PUT /dav/files/<uid>/<path>            ┐
 ├─ PUT /s/<token>/<path>                  │ writes mutate bytes
 ├─ MOVE …/foo → …/bar (overwrite)         │
 ├─ files-page row drag/upload             │
 │                                         ▼
 │                                  View::write_file / View::move_with_overwrite
 │                                         │
 │                                         ├─ Versions::snapshot_if_needed(owner_uid, owner_path, current_row)
 │                                         │     ├─ skip if size 0 / size > cap / throttle window not elapsed
 │                                         │     ├─ LocalStorage.copy(<owner>/files/<path>, <owner>/files_versions/<path>.v<mtime>)
 │                                         │     └─ INSERT oc_files_versions
 │                                         │
 │                                         └─ proceed with the actual write
 │
 ├─ /dav/versions/<uid>/<fileid>/…
 │     ├─ PROPFIND → Versions::list_for(fileid)
 │     ├─ GET      → stream bytes from on-disk version file
 │     └─ COPY     → Versions::restore(version_id)
 │
 ├─ /ocs/v2.php/apps/files_versions/api/v1/versions/{fileid}
 │     ├─ GET → list
 │     ├─ POST /restore/{vid} → restore
 │     └─ DELETE /version/{vid} → delete
 │
 └─ Files page row × menu × "Versions"
       └─ VersionsPanel (server-fn driven) → list / restore / delete

crabcloud-versions  (NEW crate)
 ├─ VersionEntry { id, storage_id, fileid, user, path, version_mtime, size }
 ├─ Versions::{snapshot_if_needed, list_for, restore, delete, sweep_tiered, purge_for_fileid}
 ├─ Multidialect SQL via match self.pool.as_ref() (sqlite/mysql/postgres)
 └─ Depends on: crabcloud-db, crabcloud-storage, crabcloud-filecache

VersionsSweeper  (crates/crabcloud-core/src/versions_sweeper.rs)
 ├─ run(): 24h cooperative loop
 ├─ sweep_once() -> Result<u64>  (for tests)
 └─ Spawned unconditionally in AppStateBuilder::build()

AppState additions
 ├─ versions: Arc<crabcloud_versions::Versions>
 └─ versions_sweeper_shutdown: Arc<tokio::sync::Notify>
```

## 4. Schema

```sql
CREATE TABLE oc_files_versions (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,  -- BIGSERIAL on pg, BIGINT AUTO_INCREMENT on mysql
    storage_id     BIGINT       NOT NULL,              -- oc_storages.numeric_id of the OWNER's home storage
    fileid         BIGINT       NOT NULL,              -- oc_filecache.fileid of the CURRENT file (or last-known)
    "user"         VARCHAR(64)  NOT NULL,              -- owner uid; redundant with storage_id but cheap to index
    path           VARCHAR(512) NOT NULL,              -- owner-relative path at snapshot time, e.g. "/projects/q1/report.docx"
    version_mtime  BIGINT       NOT NULL,              -- unix seconds; matches the "v<mtime>" on-disk suffix
    size           BIGINT       NOT NULL
);

CREATE INDEX idx_versions_user_fileid    ON oc_files_versions ("user", fileid);
CREATE INDEX idx_versions_user_mtime     ON oc_files_versions ("user", version_mtime);
CREATE INDEX idx_versions_storage_fileid ON oc_files_versions (storage_id, fileid);
```

The on-disk layout under `<datadirectory>/<uid>/files_versions/` mirrors the source path one-for-one: `files_versions/projects/q1/report.docx.v1716123456`. Multiple versions of the same file coexist as multiple `.v<n>` siblings.

A separate `oc_storages` row is **not** added for versions (the source storage_id is reused; we don't surface versions through filecache scanning). This is the one deliberate divergence from the trash layout, which DID add a `trash::<uid>` storage row — versions are accessed by direct DB lookup, not by traversing the storage hierarchy, so a separate storage row buys nothing and would just duplicate scanner work.

## 5. Surface contracts

### 5.1 DAV — `/dav/versions/{uid}/{fileid}/...` (and `/remote.php/dav/versions/...` alias)

| Method | Behavior |
|---|---|
| PROPFIND `/dav/versions/{uid}/{fileid}/` (Depth: 0, 1) | Lists the versions for `{fileid}`. Each entry's `<d:href>` is `/dav/versions/{uid}/{fileid}/{version_mtime}` (the mtime IS the resource identifier). Props: `displayname` (= original basename), `getlastmodified` (= `version_mtime`), `getcontentlength` (= `size`), `getcontenttype`, `resourcetype` (empty), `{http://nextcloud.org/ns}version-author` (omitted in MVP). |
| PROPFIND `/dav/versions/{uid}/{fileid}/{version_mtime}` (Depth: 0) | Single-entry detail. |
| GET `/dav/versions/{uid}/{fileid}/{version_mtime}` | Streams the on-disk version bytes. `Content-Type` from the filecache mime of the current file; `Content-Length` from `size`. |
| COPY `/dav/versions/{uid}/{fileid}/{version_mtime}` with `Destination: /dav/files/{uid}/<current_path>` | Restores: snapshots current, copies version bytes over current. 204 on success. 404 if version row missing or destination doesn't match the current file's owner path. 412 if destination's etag doesn't match the latest filecache row (optimistic-concurrency check). |
| Anything else (PUT, POST, DELETE, MOVE, MKCOL, PROPPATCH, LOCK, UNLOCK) | 405. |

`{uid}` must match the authed user OR the authed user must have update permission on a share that covers the file. Otherwise 403.

### 5.2 OCS — `/ocs/v2.php/apps/files_versions/api/v1/versions/{fileid}`

| Method | Path | Behavior |
|---|---|---|
| GET | `/versions/{fileid}` | JSON list of `VersionDto` for `fileid` (auth-gated by ownership / share). |
| POST | `/restore/{version_id}` | Restore (snapshot-then-replace). |
| DELETE | `/version/{version_id}` | Hard-delete one version row + on-disk file. |

Note the `version_id` here is the row PK of `oc_files_versions`, not the `version_mtime`. DAV uses `version_mtime` because it's the natural href segment; OCS uses the row id because clients pass it back from list responses verbatim.

### 5.3 Server-fn API (Dioxus)

```rust
#[server]
pub async fn list_versions(fileid: i64) -> Result<Vec<VersionDto>, ServerFnError>;
#[server]
pub async fn restore_version(version_id: i64) -> Result<(), ServerFnError>;
#[server]
pub async fn delete_version(version_id: i64) -> Result<(), ServerFnError>;
```

`VersionDto`: `{ id: i64, version_mtime: i64, size: i64 }`. The UI needs the size for display and the mtime for the "5 minutes ago" label; the basename + path are already known from the row the user is acting on.

All three are gated by the same `require_user()` extractor as the trash server fns. Each fn additionally verifies the authed user is the owner of `fileid` OR has update permission on a share that covers it.

## 6. Edge cases

| Case | Behavior |
|---|---|
| **Restore** | Snapshot current as a new version row, then copy version's bytes over current. Lossless. The version being restored stays in the versions list. |
| **Shared file, Bob edits** | Versions row's `user` = file owner (Alice). On-disk bytes land under `<alice>/files_versions/...`. Only Alice's panel surfaces the history. |
| **Public-link upload overwrites a shared file** | Same as above. Owner's history records the change. No editor attribution in MVP. |
| **Throttle window** | Within `versions_min_interval_secs` of the last version, no new snapshot. Default 2s. The "missed" intermediate write still happens — only the version is skipped. |
| **Size cap exceeded** | Skip versioning + `tracing::warn!` with path + size. The write itself still proceeds. |
| **Zero-byte source / source missing** | Skip snapshot (nothing to back up). |
| **Trash hard-delete** | `Trash::purge_entry` calls `Versions::purge_for_fileid(fileid_legacy)` so all versions of that fileid are removed. Public-link DELETE bypasses trash → no version cascade needed for that path (versions cascade only on hard delete from trash). |
| **Restore-on-trash-restore** | When a file is restored from the trash bin, versions for the original fileid were already purged at hard-delete time (see above). They do NOT come back. If versions need to survive trash, that's a future feature. |
| **MOVE rename (no overwrite)** | Source's `fileid` is preserved; existing versions stay attached. On-disk versions tree is NOT moved/renamed (cheap; the DB lookup is by fileid, not path). |
| **MOVE overwrite** | Counts as a write of the destination. Snapshot destination's current bytes before the overwrite. The source side's `fileid` survives the move; no source-side version is created (source isn't changing — it's leaving). |
| **Read-only share write attempt** | Permission denied at storage layer; no versions side-effect. |
| **Restore target's etag changed since list** | DAV COPY returns 412 if destination's etag doesn't match the latest filecache row at restore time (optimistic concurrency). OCS POST /restore returns the same. UI hides the race by always refetching the list immediately after a restore. |
| **Version's on-disk file missing** | The version row exists but the bytes don't (manual operator intervention, partial filesystem failure). GET/COPY returns 500 with a `tracing::error!`. List still surfaces the row so the operator sees something to clean up. `delete_version` removes the row regardless. |
| **Disk full during snapshot** | `Versions::snapshot_if_needed` returns `Err(VersionsError::Io)`. `View::write_file` decides: fail the write (safer — user sees the failure and can choose to retry) vs. log + continue (loses versioning silently). **Pick: fail the write.** Versions are part of the contract; failing visibly is better than losing data silently. |

## 7. Testing

- **`crabcloud-versions` unit/e2e (sqlite)**: round-trip via a fake storage + sqlite pool. Coverage: `snapshot_if_needed` writes row + copies bytes; throttle window blocks within `versions_min_interval_secs`; size cap blocks; zero-byte source skips; `list_for` returns rows in `version_mtime DESC`; `restore` snapshots current + replaces; `delete` removes row + on-disk file; `sweep_tiered` keeps newest-per-bucket only; `purge_for_fileid` removes everything for a fileid.
- **`crabcloud-core::versions_sweeper` e2e (sqlite)**: insert rows with stale + fresh `version_mtime`, call `sweep_once()`, assert correct keep/drop based on bucket boundaries. Confirm `versions_retention_disabled = true` short-circuits.
- **`crabcloud-fs::view` integration**: `View::write_file` creates a version on overwrite; throttle prevents a second version within the window; size cap is honored; zero-byte writes don't snapshot; `View::move_with_overwrite` snapshots the destination; read-only share write attempt is permission-denied without a versions side-effect.
- **`crabcloud-fs::view` cross-user**: Alice shares /report.docx with Bob; Bob's `View::write_file` lands a version with `user='alice'`; Alice's `list_for` sees it; Bob's `list_for` does NOT see it (unless Bob has owner-visibility — he doesn't in MVP).
- **`crabcloud-http` DAV e2e**: PROPFIND lists per-file versions, GET downloads bytes, COPY-with-Destination restores. 412 on etag mismatch.
- **`crabcloud-http` OCS e2e**: GET/POST/DELETE shapes against a seeded user with versions.
- **`crabcloud-app` server-fn integration**: round-trip list → restore → file content matches the chosen version.
- **`crabcloud-trash` integration**: `Trash::purge_entry` cascade-purges `Versions::purge_for_fileid`. Soft-delete does NOT trigger the cascade (versions survive soft-delete).
- **Public-link write cascade**: anonymous PUT overwrites a shared file → owner's versions table gets a row.

## 8. Batches (implementation order)

A. **Core + triggers + sweeper** — new `crabcloud-versions` crate, migration `0010_files_versions`, `Versions::{snapshot_if_needed, list_for, restore, delete, sweep_tiered, purge_for_fileid}`, `View::write_file` + `View::move_with_overwrite` hooks (and the symmetric share-mount write paths), `VersionsSweeper` with `versions_min_interval_secs` / `versions_max_bytes` / `versions_retention_disabled` config knobs, AppState wiring, `Trash::purge_entry` cascade. Unit + e2e + integration tests.
B. **DAV** — `/dav/versions/{uid}/{fileid}/...` router covering PROPFIND, GET, COPY. Optimistic-concurrency etag check on COPY. 405 on other methods. Plus `/remote.php/dav/versions/...` alias.
C. **OCS + server fns** — `/ocs/v2.php/apps/files_versions/api/v1/versions/{fileid}` GET/POST/DELETE; `list_versions` / `restore_version` / `delete_version` server fns. Cross-crate parity test for the JSON shape vs Nextcloud.
D. **UI** — per-row "Versions" item in the file row × menu opens a `VersionsPanel` modal (reuses `.files-modal-*` chrome) listing entries with Restore + Delete actions per version. SSR snapshot test.

Each batch ships as one PR through subagent-driven-development with the standard two-stage review (spec compliance → code quality).
