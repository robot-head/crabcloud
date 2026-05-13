# Sharing (user + group) and virtual mount — Design (Sub-project 7)

**Status:** spec — design only, no implementation.
**Date:** 2026-05-13
**Sub-project:** 7 of ~13. SP8 will extend with public links / anonymous viewer / file-drop.

## 1. Goal

Ship outgoing user-and-group shares in Crabcloud, with the recipient seeing each accepted share as a virtual mount at their filesystem root (visible in both the web UI and over WebDAV / desktop clients). Owners create and manage shares via the Nextcloud-compatible OCS Sharing API and via a share modal in the Files UI.

**In scope:**

- New `oc_share` schema (full Nextcloud columns, so SP8's public-link work needs no migration).
- New `crabcloud-sharing` crate (CRUD + group-aware lookup).
- `SharedSubrootStorage` wrapper in `crabcloud-fs`.
- `ShareMountResolver` returning home mount + a synthesized mount per accepted incoming share.
- OCS endpoints under `/ocs/v2.php/apps/files_sharing/api/v1/shares` — `POST`, `GET` (list-for-path + shared-with-me), `GET /{id}`, `PUT /{id}`, `DELETE /{id}`. `share_type` `0` (user) and `1` (group) implemented; `share_type` `3` (link) wired into the schema but the create path returns `501 not implemented`, reserved for SP8.
- Files UI: Share button as a third entry in the row `⋯` menu, share modal (recipient picker, permission toggles, current-shares list), "Shared with you" sidebar entry that's just a navigation chip — the items themselves render through the normal Files page because they're real mounts.
- Re-share rejected (permission bit `16` stripped on create; documented and enforced by storage-id comparison).

**Explicitly out of scope (deferred):**

- Public links, anonymous viewer page, public WebDAV, file-drop (SP8).
- Re-sharing (cascade logic).
- Share notifications / activity stream.
- Recipient-side share rename (the recipient always sees the owner's `basename`; collision suffix `(N)` is computed at resolve time, not stored).
- Pending / accept / decline flow (auto-accept).
- Federated shares (separate SP).
- Expiration enforcement on user/group shares (`expiration` column lands now so SP8's public-link work can drive it; the SP7 UI doesn't set it).

## 2. Load-bearing decisions (brainstormed)

| # | Decision | Rationale |
|---|---|---|
| 1 | **Schema mirrors Nextcloud `oc_share`** (id, share_type, share_with, uid_owner, uid_initiator, parent, item_type, item_source, file_source, file_target, permissions, stime, accepted, expiration, token, password, mail_send). | Existing clients query this shape via OCS. All columns up-front means SP8 reuses the table without a migration. |
| 2 | **Permission storage = full Nextcloud bitmask** stored as `INTEGER`; SP7 strips bit `16` (share) on create. | Forward-compat: re-share work is a logic change, not a schema change. SP7 invariant: every row has `permissions & 16 == 0`. |
| 3 | **Sharing = `SharedSubrootStorage(owner_storage, owner_path, permissions)`**, returned by a new `ShareMountResolver` that wraps the existing `HomeMountResolver`. | View unchanged. File-id continuity automatic via owner's filecache + `storage.id()` namespace. |
| 4 | **Mount path for an accepted share = `/<basename(owner_path)>`** at the recipient's root, with `(N)` suffix on collision with home contents or another share. Computed at resolve time, not stored. | Nextcloud-default UX. Desktop clients see shares where they expect. Collision suffix isn't persisted so renames of the owner's source don't shift unrelated entries. |
| 5 | **OCS endpoint paths and JSON envelopes are Nextcloud-verbatim** under `/ocs/v2.php/apps/files_sharing/api/v1/...`. | Maximizes existing-client compatibility, which is the SP7 scope's explicit goal. |
| 6 | **Owner-only sharing**: only the file's owner can create a share for it. Enforced at the `Shares` service via storage-id comparison. Bit `16` is stripped on create. | SP7 invariant. Re-share is SP-later. |
| 7 | **Share creation auto-accepts**. No pending state, no separate accept endpoint. | Matches the per-question answer; UX simpler. Schema's `accepted` column is reserved so a later SP can add the flow without a migration. |
| 8 | **Share lookup at request time** walks the recipient's group memberships via the existing `crabcloud-users::Groups`, then unions `(share_type=0 AND share_with=uid) OR (share_type=1 AND share_with IN user_groups)`. No cross-request caching in MVP. | Group membership rarely changes mid-request; cross-request caching adds invalidation complexity not justified for MVP. |
| 9 | **Share modal launched from the row `⋯` menu** — third entry after Rename, Delete. Modal: recipient picker (autocomplete over users + groups), permission toggles (Can edit / Can delete; read is implicit), current-shares list with revoke buttons. | Reuses the row `⋯` infrastructure SP6 left open per the followup notes; reuses the `DeleteModal` chrome. |
| 10 | **"Shared with you" sidebar entry is a navigation chip**, not a separate filesystem route. Click → navigate to `/apps/files/` (root). The mounts themselves are first-class entries at root. | Mirrors the actual filesystem semantics; avoids a parallel view that would diverge from what WebDAV clients see. |
| 11 | **`Mount` gains an optional `metadata: Option<MountMetadata>`** field carrying `owner_uid` + `permissions` for share mounts. | Required so `View::list` can decorate entries with "shared by alice" badges without a second `Shares::list_incoming` query and without `list_dir` becoming a second source of truth. Additive — existing call sites pass `None` and read no metadata. |

## 3. Architecture

```
Web UI (Files)
 ├─ Share button in row ⋯ menu                                ← new
 ├─ ShareModal (recipient picker + permission toggles)        ← new
 │   └─ POST /ocs/v2.php/apps/files_sharing/api/v1/shares
 └─ Sidebar nav: "Shared with you"                            ← new (chip)
     └─ /apps/files/ (mounts already at root)

OCS API surface (new in SP7)
 /ocs/v2.php/apps/files_sharing/api/v1/shares
   ├─ POST            create share
   ├─ GET ?path=…     shares I created on this path
   ├─ GET ?shared_with_me=true   shares received by me
   ├─ GET /{id}       fetch one
   ├─ PUT /{id}       update perms/expire (link-only fields → 501 in SP7)
   └─ DELETE /{id}    revoke

Server
 ├─ axum router  (build_router merges into existing OCS subrouter)
 │
 ├─ Shares service (crabcloud-sharing — new crate)
 │   ├─ create(req)   — owner-only, strips bit 16, resolves recipient
 │   ├─ get(id)
 │   ├─ list_for_owner_path(owner, path) | list_outgoing(owner)
 │   ├─ list_incoming(recipient)         — joins on group membership
 │   ├─ update(id, fields)               — owner only
 │   └─ delete(id, requester)            — owner → row gone; recipient → accepted=0
 │   uses crabcloud-users for display-name + Groups lookups
 │   uses crabcloud-filecache for source-path → fileid + storage-id ownership check
 │
 ├─ ShareMountResolver (crabcloud-fs — new)
 │   wraps HomeMountResolver:
 │     - returns the home mount (delegated)
 │     - PLUS one mount per incoming share:
 │         path_prefix = "/<collision-suffixed basename>"
 │         storage     = SharedSubrootStorage(owner_home, owner_path, permissions)
 │         metadata    = Some(MountMetadata{ owner_uid, permissions })
 │
 ├─ SharedSubrootStorage (crabcloud-fs — new)
 │   delegates to inner; refuses write/delete/move when permissions disallow
 │   storage.id() == inner.id()  (filecache rows live in owner's namespace)
 │
 └─ existing layers unchanged: View, AuthLayer, SessionLayer, CSRF, dav, dx SSR
```

### 3.1 Data flow — create

alice shares `/Vacation Photos` with bob (read + update):

1. Browser `POST /ocs/v2.php/apps/files_sharing/api/v1/shares` with `path=/Vacation Photos&shareType=0&shareWith=bob&permissions=3`.
2. OCS handler resolves alice via `AuthContext`, calls `Shares::create(CreateShareRequest{ ... })`.
3. Service: resolve `path` → filecache row → record `item_source`, `item_type`. Verify `storage_id` belongs to requester (re-share rejection). Strip bit 16. Verify `bob` exists. Insert row with `accepted=1`, `stime=now`, `file_target="/Vacation Photos"`.
4. Response: Nextcloud-shaped JSON.

### 3.2 Data flow — bob lists his root

1. `GET /apps/files/`.
2. SSR's `FilesRoute` calls `View::list(bob, "/")`.
3. View asks `ShareMountResolver::mounts_for(bob)` → `[home_mount, share_mount("/Vacation Photos")]`.
4. View enumerates root via the home mount, then appends a synthetic entry per share-mount whose `path_prefix` lives one level deep at root. Each share-mount entry's metadata (size, mtime, fileid) comes from alice's filecache for `(alice_storage_id, /Vacation Photos)`. The entry is decorated with `shared_by="alice"` from the mount's `metadata`.

### 3.3 Data flow — bob opens `/Vacation Photos/x.jpg`

1. Browser navigates to `/apps/files/Vacation%20Photos/`.
2. View resolves: path matches `share_mount.path_prefix = "/Vacation Photos"`. Strip prefix → `recipient_relative="/"`.
3. View calls `SharedSubrootStorage::list("/")` → translates to `alice_home.list("/Vacation Photos/")` (the `owner_path` the resolver baked in from looking up `item_source` in alice's filecache) → returns alice's entries verbatim. File IDs are alice's. Mtimes are alice's.
4. Bob clicks `x.jpg` → `<a href="/dav/files/bob/Vacation%20Photos/x.jpg">`. WebDAV resolves the same way: `ShareMountResolver` finds the mount, View routes the read through `SharedSubrootStorage`, which reads from `alice_home`.

If alice has renamed her source folder since the share was created (`/Vacation Photos` → `/Holiday`), `item_source` still points to the same filecache row, the row's path is now `/Holiday`, and the resolver constructs the share mount with `owner_path="/Holiday"`. Bob still sees the share at `/Vacation Photos` (his mount name, derived from the original `file_target`). The share survives owner-side renames cleanly.

### 3.4 Mount permission enforcement

`SharedSubrootStorage` checks the permission bitmask before mutating ops:

- `write` (existing path) → bit `2` (update).
- `write` (new path) / `mkdir` → bit `4` (create).
- `delete` → bit `8`.
- `move` (always within-mount; cross-mount moves decompose to read+write at the View layer) → bit `2`.
- `read` / `list` / `head` → always allowed (bit `1` implied; SP7 has no read-denied shares).

Disallowed ops return `StorageError::PermissionDenied`. View surfaces this through the existing error path; WebDAV maps it to `403`; server fns map it to their existing forbidden response.

## 4. Schema (migration `0006_shares`)

```sql
-- sqlite
CREATE TABLE oc_share (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    share_type    SMALLINT     NOT NULL,
    share_with    VARCHAR(255) NULL,
    uid_owner     VARCHAR(64)  NOT NULL,
    uid_initiator VARCHAR(64)  NOT NULL,
    parent        BIGINT       NULL,
    item_type     VARCHAR(64)  NOT NULL,
    item_source   BIGINT       NOT NULL,
    file_source   BIGINT       NOT NULL,
    file_target   VARCHAR(512) NOT NULL,
    permissions   INTEGER      NOT NULL,
    stime         BIGINT       NOT NULL,
    accepted      SMALLINT     NOT NULL DEFAULT 1,
    expiration    TIMESTAMP    NULL,
    token         VARCHAR(32)  NULL,
    password      VARCHAR(255) NULL,
    mail_send     SMALLINT     NOT NULL DEFAULT 0
);

CREATE INDEX idx_share_with        ON oc_share (share_with, share_type);
CREATE INDEX idx_share_owner       ON oc_share (uid_owner);
CREATE INDEX idx_share_item_source ON oc_share (item_source);
CREATE UNIQUE INDEX idx_share_token ON oc_share (token) WHERE token IS NOT NULL;
```

MySQL and Postgres variants follow the same shape with dialect-appropriate types:

- ID: `BIGINT AUTO_INCREMENT` (mysql) / `BIGSERIAL` (postgres).
- `TIMESTAMP` uses the same dialect mapping as `0005_webdav_props_and_locks`.
- The `idx_share_token` unique constraint is a partial index on sqlite + postgres; on mysql it's a full unique index (mysql treats multiple NULLs in a unique index as distinct, which is exactly what we want — same approach as `0003_auth_tokens`).

Notes:

- `item_source` and `file_source` are always equal in SP7. Nextcloud distinguishes them for federated shares — we keep both columns so a future SP needs no migration.
- `file_target` is stored at create time and never modified by SP7 (no recipient rename). SP8 may update it for link-share custom names.
- No FK on `uid_owner` / `share_with`. Matches the existing pattern (cross-dialect FK ordering is brittle and `oc_users` rows aren't routinely deleted).
- `permissions INTEGER` (max value used in SP7 is `15 = 0x0F`; Nextcloud's full bitmask reaches `0x1F`). Plenty of headroom.

## 5. The `crabcloud-sharing` crate

New workspace crate. Depends on `crabcloud-db`, `crabcloud-users`, `crabcloud-filecache`, `crabcloud-storage`. Does **not** depend on `crabcloud-fs` (one-way) so `crabcloud-fs` can depend on `crabcloud-sharing` without a cycle.

```rust
pub struct Shares {
    pool: Arc<DbPool>,
    users: Arc<dyn UserLookup>,        // display_name + exists checks
    groups: Arc<dyn GroupLookup>,      // groups_of(uid)
    filecache: Arc<Filecache>,         // path → (fileid, storage_id)
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ShareType { User = 0, Group = 1, Link = 3 }

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ItemType { File, Folder }

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct SharePermissions(u8);
impl SharePermissions {
    pub const READ:   Self = Self(1);
    pub const UPDATE: Self = Self(2);
    pub const CREATE: Self = Self(4);
    pub const DELETE: Self = Self(8);
    pub fn from_bitmask(b: u32) -> Self { Self((b & 0x0F) as u8) }
    pub fn bitmask(self)   -> u32  { self.0 as u32 }
    pub fn allows_write(self)  -> bool { (self.0 & (Self::UPDATE.0 | Self::CREATE.0)) != 0 }
    pub fn allows_delete(self) -> bool { (self.0 & Self::DELETE.0) != 0 }
    pub fn allows_create(self) -> bool { (self.0 & Self::CREATE.0) != 0 }
}

#[derive(Debug, Clone)]
pub struct ShareRow {
    pub id: i64,
    pub share_type: ShareType,
    pub share_with: Option<String>,
    pub uid_owner: String,
    pub uid_initiator: String,
    pub parent: Option<i64>,
    pub item_type: ItemType,
    pub item_source: i64,
    pub file_target: String,
    pub permissions: SharePermissions,
    pub stime: i64,
    pub accepted: bool,
    pub expiration: Option<DateTime<Utc>>,
    pub token: Option<String>,
    pub password_hash: Option<String>,
}

pub struct CreateShareRequest<'a> {
    pub requester: &'a UserId,
    pub path: UserPath,
    pub share_type: ShareType,
    pub share_with: String,
    pub permissions: u32,
}

pub struct UpdateShareFields {
    pub permissions:  Option<u32>,            // bit 16 stripped
    pub expire_date:  Option<Option<NaiveDate>>,
    pub password:     Option<Option<String>>, // SP8
    pub note:         Option<String>,         // deferred
}

#[derive(thiserror::Error, Debug)]
pub enum ShareError {
    #[error("not found")]                  NotFound,
    #[error("forbidden")]                  Forbidden,
    #[error("recipient unknown")]          RecipientUnknown,
    #[error("invalid share type")]         InvalidShareType,
    #[error("bad permissions bitmask")]    BadPermissions,
    #[error("re-share rejected")]          ReshareRejected,
    #[error("path not owned by requester")]PathNotOwned,
    #[error("not implemented (SP8)")]      NotImplemented,
    #[error(transparent)]                  DbError(#[from] sqlx::Error),
}

impl Shares {
    pub async fn create(&self, req: CreateShareRequest<'_>) -> Result<ShareRow, ShareError>;
    pub async fn get(&self, id: i64) -> Result<Option<ShareRow>, ShareError>;
    pub async fn list_outgoing(&self, owner: &UserId) -> Result<Vec<ShareRow>>;
    pub async fn list_for_owner_path(&self, owner: &UserId, path: &UserPath) -> Result<Vec<ShareRow>>;
    pub async fn list_incoming(&self, recipient: &UserId) -> Result<Vec<ShareRow>>;
    pub async fn update(&self, id: i64, requester: &UserId, fields: UpdateShareFields) -> Result<ShareRow, ShareError>;
    pub async fn delete(&self, id: i64, requester: &UserId) -> Result<(), ShareError>;
}
```

### `Shares::create` policy

1. Resolve `req.path` via `filecache.lookup_by_user_path(&req.requester, &req.path)`. Missing → `PathNotOwned`.
2. Verify the row's `storage_id` is the requester's home storage id. Otherwise `ReshareRejected`. (This is also how owner-only is enforced.)
3. Mask supplied `permissions` to `0x1F`, then strip bit `16`. Bit `1` (read) is required — if absent, reject with `BadPermissions` mapped to `400`.
4. `share_type == Link` → `NotImplemented` (SP7 boundary; SP8 will implement).
5. Verify recipient: `users.exists(share_with)` for `User`, `groups.exists(share_with)` for `Group`. Otherwise `RecipientUnknown`.
6. Insert with `accepted=1`, `stime=now()`, `uid_initiator = uid_owner = requester`, `file_target = "/" + basename(path)`.

### `list_incoming` query

```sql
SELECT * FROM oc_share
WHERE accepted = 1
  AND share_type IN (0, 1)
  AND (
        (share_type = 0 AND share_with = :uid)
     OR (share_type = 1 AND share_with IN (:user_groups))
      )
```

`:user_groups` comes from `groups.groups_of(uid)`. Empty group list → second branch trivially false; first branch handles user shares.

### `delete` semantics

- Owner (`requester == row.uid_owner`) → `DELETE FROM oc_share WHERE id=:id`. Row vanishes.
- Recipient (`share_type=0 AND share_with=requester`, or `share_type=1 AND requester ∈ groups(:share_with)`) → `UPDATE oc_share SET accepted=0 WHERE id=:id`. Filtered out of `list_incoming`; still visible to the owner via `list_outgoing` (decorated as "removed by recipient" in SP7+? — SP7 returns it without special-case decoration).
- Neither owner nor recipient → `Forbidden`.

## 6. `crabcloud-fs` additions

### `Mount` gains `metadata`

```rust
#[derive(Clone, Debug)]
pub struct MountMetadata {
    pub kind: MountKind,
    pub owner_uid: Option<String>,         // Some for share mounts
    pub permissions: Option<SharePermissions>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MountKind { Home, Share }

#[derive(Clone)]
pub struct Mount {
    pub path_prefix: StoragePath,
    pub storage:     Arc<dyn Storage>,
    pub metadata:    Option<MountMetadata>,  // None for the home mount
}
```

Existing call sites pass `metadata: None` (one-line addition). `View::list` reads `metadata.owner_uid` when decorating share-mount root entries with `shared_by`.

### `SharedSubrootStorage`

```rust
pub struct SharedSubrootStorage {
    inner: Arc<dyn Storage>,
    owner_path: StoragePath,
    permissions: SharePermissions,
}

#[async_trait::async_trait]
impl Storage for SharedSubrootStorage {
    fn id(&self) -> &str { self.inner.id() }

    async fn read(&self, p: &StoragePath, range: ByteRange) -> StorageResult<Bytes> {
        self.inner.read(&self.translate(p), range).await
    }

    async fn write(&self, p: &StoragePath, body: Bytes, existing: bool) -> StorageResult<Metadata> {
        let need_bit = if existing {
            SharePermissions::UPDATE
        } else {
            SharePermissions::CREATE
        };
        if (self.permissions.bitmask() as u8 & need_bit.0) == 0 {
            return Err(StorageError::PermissionDenied);
        }
        self.inner.write(&self.translate(p), body, existing).await
    }

    async fn delete(&self, p: &StoragePath) -> StorageResult<()> {
        if !self.permissions.allows_delete() { return Err(StorageError::PermissionDenied); }
        self.inner.delete(&self.translate(p)).await
    }

    async fn mkdir(&self, p: &StoragePath) -> StorageResult<()> {
        if !self.permissions.allows_create() { return Err(StorageError::PermissionDenied); }
        self.inner.mkdir(&self.translate(p)).await
    }

    async fn move_(&self, from: &StoragePath, to: &StoragePath) -> StorageResult<()> {
        // Both endpoints translate; permission check uses update for inside-mount move.
        if !self.permissions.allows_write() { return Err(StorageError::PermissionDenied); }
        self.inner.move_(&self.translate(from), &self.translate(to)).await
    }

    async fn list(&self, p: &StoragePath) -> StorageResult<Vec<DirEntry>> {
        self.inner.list(&self.translate(p)).await
    }

    async fn head(&self, p: &StoragePath) -> StorageResult<Metadata> {
        self.inner.head(&self.translate(p)).await
    }
}

impl SharedSubrootStorage {
    fn translate(&self, recipient_relative: &StoragePath) -> StoragePath {
        self.owner_path.join(recipient_relative)
    }
}
```

The View layer enforces "moves across mounts must be copy + delete" already (existing rule). A share→home or home→share move therefore decomposes into a `read` on one side and a `write` on the other, each going through its own permission filter. Nothing share-specific needs to land in View.

### `ShareMountResolver`

```rust
pub struct ShareMountResolver {
    home: HomeMountResolver,
    shares: Arc<Shares>,
    storage_factory: Arc<dyn StorageFactory>,
    filecache: Arc<Filecache>,            // path_for_fileid lookups
}

#[async_trait::async_trait]
impl MountResolver for ShareMountResolver {
    async fn mounts_for(&self, uid: &UserId) -> FsResult<Vec<Mount>> {
        let mut mounts = self.home.mounts_for(uid).await?;
        let incoming = self.shares.list_incoming(uid).await?;
        let mut used_names = home_top_level_names(&mounts[0]).await?;

        for row in incoming {
            let owner_id = UserId::new(&row.uid_owner)?;
            let owner_home = self.storage_factory.home_storage(&owner_id).await?;

            // Look up the source's current location in alice's filecache by
            // fileid, NOT by row.file_target — the share survives owner-side
            // renames cleanly. file_target is only used to choose the mount
            // name bob sees at his root (see below).
            let owner_path = match self.filecache.path_for_fileid(owner_home.id(), row.item_source).await? {
                Some(p) => p,
                None => {
                    tracing::warn!(share_id = row.id, "share source not in filecache; skipping mount");
                    continue;
                }
            };

            let display_basename = basename_of(&row.file_target);
            let mount_name = unique_name(display_basename, &mut used_names);
            mounts.push(Mount {
                path_prefix: StoragePath::from_user_path(&format!("/{mount_name}"))?,
                storage: Arc::new(SharedSubrootStorage::new(
                    owner_home,
                    owner_path,
                    row.permissions,
                )),
                metadata: Some(MountMetadata {
                    kind: MountKind::Share,
                    owner_uid: Some(row.uid_owner),
                    permissions: Some(row.permissions),
                }),
            });
        }
        Ok(mounts)
    }
}
```

`unique_name` is the collision-suffix helper: `"Photos"`, `"Photos (2)"`, `"Photos (3)"`, … Home top-level names win — i.e. if bob already has `/Photos`, the share is renamed `"Photos (2)"` at mount time.

`home_top_level_names` lists the home mount's root and stores the results in a `HashSet<String>`. Each share mount's chosen name is added to the set so subsequent shares cascade their suffixes.

## 7. OCS API surface

All under `/ocs/v2.php/apps/files_sharing/api/v1/`. Responses use the existing OCS envelope (`<ocs><meta>…</meta><data>…</data></ocs>` for XML, `{"ocs":{"meta":…,"data":…}}` for JSON — the existing `format=json` query param switches per the rest of our OCS surface).

### `POST shares`

Form-encoded body:

```
path=/Vacation Photos&shareType=0&shareWith=bob&permissions=3
```

Success response (`200`, `meta.statuscode=200`):

```json
{
  "id": "42",
  "share_type": 0,
  "share_with": "bob",
  "share_with_displayname": "Bob",
  "uid_owner": "alice",
  "uid_initiator": "alice",
  "displayname_owner": "Alice",
  "item_type": "folder",
  "item_source": 1234,
  "file_source": 1234,
  "file_target": "/Vacation Photos",
  "path": "/Vacation Photos",
  "permissions": 3,
  "stime": 1747094400,
  "expiration": null,
  "token": null,
  "parent": null,
  "storage_id": "home::alice",
  "mail_send": 0
}
```

Errors:

- `400` — missing / unparseable fields.
- `403` — re-share rejection (path not owned).
- `404` — `share_with` unknown.
- `501` — `shareType=3` (link) until SP8.

### `GET shares`

Query params:

- `path=<userpath>` — list outgoing shares the requester created on this path.
- `shared_with_me=true` — list incoming shares for the requester.
- `subfiles=true` — Nextcloud's "shares inside this folder" — **SP7 returns `501`**.
- No params → outgoing shares the requester created on any path.

Response: `data` is a JSON array of the same shape as `POST shares`.

### `GET shares/{id}`

Returns the single share. `404` if not found or if the requester is neither owner, recipient, nor admin.

### `PUT shares/{id}`

Form-encoded; any subset of:

- `permissions=<int>` (bit `16` stripped; bit `1` required — `400` if absent).
- `expireDate=<YYYY-MM-DD>` — stored. SP7 doesn't enforce it for user/group shares. Documented as SP8.
- `password=<string>` — `501` for `share_type=0|1`. SP8 wires it for links.
- `note=<string>` — `501`. Deferred.

Only the share's owner can `PUT`. `403` otherwise.

### `DELETE shares/{id}`

- Owner → row deleted entirely.
- Recipient (direct user or via group membership) → `accepted=0`. The share-mount drops out of the recipient's view but the row stays around so the owner can still see it.
- Neither → `403`.

### Display-name resolution

`share_with_displayname`, `displayname_owner` come from `crabcloud-users::Users::display_name(&uid)` at response-construction. Empty string if the user no longer exists.

### Route wiring

Add `crates/crabcloud-http/src/routes/ocs/files_sharing.rs`. Register inside the existing `routes::ocs::router()` under `apps/files_sharing/api/v1/`. CSRF + auth already cover it. The `OCS-APIRequest: true` header (which Nextcloud clients send) already bypasses CSRF for non-session auth, so Bearer/Basic clients work without further changes.

## 8. UI surface (Files page)

Additive to SP6.

### Share button — row `⋯` menu

```
┌────────────────┐
│ ✏  Rename       │
│ 🗑  Delete       │
│ 🔗  Share        │   ← new
└────────────────┘
```

Click opens `ShareModal` pre-filled with the row's `path` + `file_id`.

### `ShareModal`

```
┌─────────────────────────────────────────────────────────┐
│  Share "Vacation Photos"                            ✕   │
│                                                         │
│  Add a user or group:                                   │
│  ┌───────────────────────────────┐  ┌─────────────┐     │
│  │ bo                            │  │  Add        │     │
│  └───────────────────────────────┘  └─────────────┘     │
│    • Bob (bob)                       (user)             │
│    • Board members                   (group)            │
│                                                         │
│  Current shares:                                        │
│  ┌─────────────────────────────────────────────────┐    │
│  │ ▶  Carol      [✓] Can edit  [ ] Can delete   ✕  │    │
│  │ ▶  Engineers  [✓] Can edit  [✓] Can delete   ✕  │    │
│  └─────────────────────────────────────────────────┘    │
│                                                         │
│                                          [  Close  ]    │
└─────────────────────────────────────────────────────────┘
```

- **Recipient picker** — debounced (250 ms) autocomplete. New `#[server]` fn `share_recipient_search(q: String) → Vec<RecipientCandidate>` returns up to 10 results unioned from `Users::search_by_uid_or_display_name` and `Groups::search_by_gid`, tagged `User | Group`. Selecting a candidate fills the input. Clicking *Add* posts to OCS `POST shares` with default permissions `read | update` (`3`).
- **Current shares list** — rendered from OCS `GET shares?path=<path>`. Permission checkboxes call `PUT shares/{id}` to flip bits (UPDATE bit on for "Can edit", DELETE bit on for "Can delete"). The `✕` calls `DELETE shares/{id}`.
- **Modal chrome** — reuses the same backdrop + panel structure as `DeleteModal` (SP6 followup note).

Permission UI mapping:

- Read — always on; not shown as a toggle.
- "Can edit" — `UPDATE | CREATE` (`2 | 4 = 6`) for folders, `UPDATE` (`2`) for files.
- "Can delete" — `DELETE` (`8`).

### Recipient view — "Shared with you" sidebar entry

```
☰  All files
🌐 Shared with you            ← new (chip)
⭐ Favorites                  (placeholder, no behavior — SP7+)
…
```

Click → navigate to `/apps/files/`. No filter, no separate route. The recipient's root listing already includes the share mounts as first-class entries (because `ShareMountResolver` returns them). The chip is mostly a label; it's grayed when the recipient has zero incoming shares.

### Share-mount visual treatment

Share mounts at root render with the standard folder icon plus a "shared by …" suffix:

```
📁  Vacation Photos    (shared by alice)    12 items     2 days ago
```

Rendered from `FileEntry.shared_by: Option<String>` (new additive DTO field; SP6 followup-compatible). `list_dir` populates it from `Mount.metadata.owner_uid` for the matching mount.

### Existing-share indicator (owner side)

Files alice has shared get a small chip next to their name:

```
📁  Vacation Photos    🔗 1        12 items
```

`🔗 N` is rendered when `FileEntry.share_count > 0`. `list_dir` computes this with one batched query per directory:

```sql
SELECT file_source, COUNT(*)
  FROM oc_share
 WHERE file_source IN (…)
   AND uid_owner = :uid
 GROUP BY file_source
```

## 9. Auth, permissions, error surfaces

### Who can call what

| Endpoint | Allowed | Auth methods |
|---|---|---|
| `POST shares` | Authenticated; must own the path | Session / Bearer / Basic |
| `GET shares` (any filter) | Authenticated; scoped to requester | Session / Bearer / Basic |
| `GET shares/{id}` | Owner, recipient, or admin | Session / Bearer / Basic |
| `PUT shares/{id}` | Owner only | Session / Bearer / Basic |
| `DELETE shares/{id}` | Owner (revoke) or recipient (self-unshare) | Session / Bearer / Basic |

All gated by the existing `AuthLayer`. Handlers use the `require_user()` helper SP6 added (reads `AuthContext`, so all three auth methods work).

CSRF: unchanged. The `OCS-APIRequest: true` header (sent by Nextcloud clients) already bypasses CSRF for non-session auth. Session-auth + browser fetches carry the CSRF token via SP6's WASM fetch interceptor.

### Permission semantics on the wire vs in storage

Wire permissions are the full Nextcloud bitmask `u32`. SP7 honors:

- bit `1` (read) — required. Requests that clear it are rejected with `400`.
- bit `2` (update) — enforced by `SharedSubrootStorage.allows_write`.
- bit `4` (create) — enforced by `SharedSubrootStorage.allows_create` on `mkdir` + new-file `write`.
- bit `8` (delete) — enforced by `SharedSubrootStorage.allows_delete`.
- bit `16` (share) — stripped at create. SP7 invariant: `permissions & 16 == 0`.

Bits ≥ `32`: preserved on round-trip via the stored `INTEGER`; ignored by enforcement (no logic reads them).

### Re-share rejection

`Shares::create` looks up `path` → filecache → `storage_id`. If `storage_id != home_storage_id(requester)`, the request is `403`. When bob lists `/Vacation Photos/x.jpg` from alice's share and tries `POST shares` on that path, the filecache lookup returns alice's storage_id, the comparison fails, the request is rejected.

### View-layer changes (none beyond mount routing)

`View::list`, `View::read`, `View::write`, etc. don't grow share-specific cases. They consult the resolver, pick the matching mount, and call `storage.<op>(path)`. The wrapper does the rest. Permission errors propagate as `FsError::Storage(PermissionDenied)`, which the WebDAV layer already maps to `403`; server fns map to their existing forbidden response.

The single View-layer addition is in `list_dir`'s DTO construction: when the listed entry corresponds to a mount with `metadata.kind == Share`, decorate `FileEntry.shared_by = metadata.owner_uid`. For the owner side, run the batched `share_count` query and decorate `FileEntry.share_count`.

### Filecache semantics

Filecache rows still belong to the owner's `storage_id`. Bob reading `/Vacation Photos/x.jpg` queries `(alice_storage_id, /Vacation Photos/x.jpg)` and gets alice's `fileid`. Sync clients pick up the same fileid alice's clients see. No bob-specific filecache entries are created for share contents. The recipient's filecache (`(bob_storage_id, …)`) only contains bob's own home files.

## 10. Tests, acceptance criteria, open items

### Test coverage SP7 must ship

**`crabcloud-sharing`:**

- `Shares::create` round-trip on each dialect (sqlite/mysql/postgres) via the existing test-pool harness.
- Bit `16` strip on create; verifies stored permissions.
- Re-share rejection: alice shares with bob; bob attempts to share the same path; returns `ReshareRejected`.
- `list_incoming` resolves user and group memberships correctly; respects `accepted=0`.
- `delete` semantics: owner deletes → row gone; recipient deletes → `accepted=0`; second recipient delete → `NotFound`.

**`crabcloud-fs`:**

- `SharedSubrootStorage::write` returns `PermissionDenied` when permissions lack bit `2` (existing path) or bit `4` (new path).
- `SharedSubrootStorage::delete` returns `PermissionDenied` when permissions lack bit `8`.
- Path translation: `recipient_relative=/sub/x.jpg` → `inner.read("/Vacation Photos/sub/x.jpg")`.
- `ShareMountResolver` collision suffix: bob has `/Photos` in home + an incoming share named `Photos` → resolver returns `Photos` and `Photos (2)`. Home wins.
- `ShareMountResolver` honors `accepted=0` (mount disappears when the recipient self-unshares).

**OCS (`crabcloud-http`):**

- `POST shares` happy path returns Nextcloud-shaped JSON.
- `POST shares` with `shareType=3` returns `501`.
- `GET shares?shared_with_me=true` returns the recipient's incoming shares only.
- `PUT shares/{id}` permission flip; non-owner gets `403`.
- `DELETE shares/{id}`: distinct owner vs recipient behavior.
- CSRF regression: session-auth POST without `OCS-APIRequest` + without token → `403`.

**End-to-end (Playwright):**

- alice shares folder with bob → bob's `/apps/files/` shows it at root with the "shared by alice" badge.
- bob navigates into the share, uploads a file (write permission set) → alice sees it.
- alice revokes → bob's mount disappears after reload.
- bob without write permission tries to upload → server returns `403`, UI surfaces an error.

### Acceptance criteria

- The Nextcloud desktop client (3.x) can create + list + revoke shares against Crabcloud's OCS endpoints. Verified manually (not in CI).
- All recipient-side mounts appear in WebDAV `PROPFIND /dav/files/bob/` as siblings of bob's own folders.
- `npm test` (e2e suite) passes, including new share scenarios.
- `cargo test --workspace` passes including new sharing crate tests on all three dialects.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `cargo fmt --all --check` clean.
- A screenshot of the share modal added to `docs/screenshots/` during the tests + polish batch, captured via the existing screenshots tool.

### Carry-forward to SP8 / later

1. **Storage-event invalidation across share boundaries.** When alice deletes `/Vacation Photos/x.jpg`, bob's client should observe the delete. The existing `StorageEvent`/filecache-propagate path is keyed by `(storage_id, path)`; since shares delegate to alice's storage with alice's `storage_id`, propagation should work as-is. Worth verifying with an integration test during SP7 batch F before assuming.
2. **`uid_initiator` divergence.** SP7 always sets `uid_initiator = uid_owner`. SP8's link shares may differ (a delegated admin minting a link). The column is writable; the `Shares::create` request shape can grow an optional `initiator` field then.
3. **Mount-list caching.** `ShareMountResolver::mounts_for` runs per request and hits the DB. If hot, cache by `uid` in `AppState` with explicit invalidation on `Shares::create/delete`. Not needed for MVP.
4. **Trash on share revoke.** When alice revokes, bob's mount disappears. Files alice put in there pre-share stay (in alice's home). Files bob uploaded also stay (they're in alice's storage now). UI should make this clear in the revoke confirmation copy; otherwise users will assume revoke "deletes" things.
5. **Notifications.** No activity stream in SP7. Alice's revoke doesn't tell bob anything beyond the mount silently vanishing on reload.
6. **Reasoning about deep paths with broken filecache.** If alice's filecache row for the source vanishes (manual DB surgery, crash mid-scan), `list_incoming` still returns the share row but the mount construction fails. Should fall through to "skip this mount with a warning" rather than erroring the whole request. Worth a test in batch F.

### Decomposition into batches

Mirrors SP6's A–F shape. Concrete batching is the writing-plans skill's job; sketch only:

- **A**: migration `0006_shares` + `crabcloud-sharing` crate skeleton (types + empty impls + test fixtures).
- **B**: `Shares` service logic (create / list / update / delete) with full unit tests per dialect.
- **C**: `crabcloud-fs` — `SharedSubrootStorage` + `ShareMountResolver` + `Mount.metadata` + tests.
- **D**: OCS endpoints (`files_sharing.rs`) + handler integration tests.
- **E**: UI — Share button, `ShareModal`, recipient autocomplete server fn, `FileEntry.shared_by` / `share_count` rendering, sidebar chip.
- **F**: e2e tests, share-modal screenshot, acceptance polish.
