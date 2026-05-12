# WebDAV + Files API (Sub-project 5)

**Date:** 2026-05-12
**Status:** Brainstormed; awaiting user approval before plan-writing.

## 1. Goal

Add Nextcloud-compatible WebDAV endpoints at `/remote.php/dav/files/<user>/<path>` and `/remote.php/dav/uploads/<user>/<id>/...` (plus the modern `/dav/...` alias). After SP5, existing Nextcloud desktop / iOS / Android sync clients can talk to Crabcloud unmodified.

HTTP handlers are thin protocol layers over 4c's `View` + `Uploads` façades plus two new tables (`oc_properties` for PROPPATCH custom props, `oc_filelocks` for WebDAV LOCK/UNLOCK).

## 2. Why now / who asked

Sub-projects 4a/4b/4c shipped the storage trait, file cache, and per-user filesystem facade. Without an HTTP surface, none of it is reachable by clients. SP5 closes the gap; the existing AuthLayer (Bearer/Basic/Cookie) from sub-project 2b already gates the route tree.

## 3. Scope

**In scope:**

- WebDAV method set: `OPTIONS`, `PROPFIND`, `GET`, `HEAD`, `PUT`, `MKCOL`, `DELETE`, `MOVE`, `COPY`, `PROPPATCH`, `LOCK`, `UNLOCK`.
- Chunked-upload protocol routes (`MKCOL` to begin, `PUT` per chunk, `MOVE` to commit, `DELETE` to abort) at `/dav/uploads/<user>/<id>/...`.
- Sync-essential PROPFIND prop set: `{DAV:}getcontentlength` / `getetag` / `getlastmodified` / `getcontenttype` / `resourcetype` / `displayname` + `{oc:}id` / `permissions` / `size` / `favorite`. 10 props total.
- PROPFIND `Depth: 0` and `Depth: 1`. `Depth: infinity` rejected (`403 Propfind-Finite-Depth`).
- Conditional headers on PUT/MOVE/COPY/DELETE/PROPPATCH: `If-Match: "<etag>"`, `If-Match: *`, `If-None-Match: *`.
- Single-range `Range:` GET (`Range: bytes=N-M`); 206 + `Content-Range`; 416 on invalid.
- PROPPATCH `<set>`+`<remove>` against `oc_properties`; path-keyed; rename hook updates paths.
- LOCK/UNLOCK; exclusive locks only; depth 0 or infinity; default timeout 1800s; expired locks reacquireable.
- Ancestor-lock check on every mutating method (PUT/MKCOL/DELETE/MOVE/COPY/PROPPATCH).
- Both URL prefixes routable: `/remote.php/dav/...` (legacy) and `/dav/...` (modern alias).
- Part-tag transport: PUT-chunk response carries `ETag: "<sha256>"`; MOVE-commit sends `X-Crabcloud-Part-Tags: <JSON>` header.
- AuthLayer gates all DAV routes (Bearer/Basic/Cookie).
- Migration `0005_webdav_props_and_locks` (sqlite/mysql/postgres).
- ~30 integration tests + ~10 unit tests + 1 Playwright e2e.

**Out of scope (deferred):**

- Trash / `/dav/trashbin/<user>/...` — separate sub-project.
- Versions / `/dav/versions/<user>/...` — separate sub-project.
- DAV `REPORT` method (sync-collection, calendar-query, addressbook-query) — separate sub-project (calendar/contacts).
- Shared locks (`<lockscope><shared/>`) — exclusive only in SP5.
- Comprehensive `If:` header grammar — SP5 parses the `<urn:uuid:...>` form only.
- Property metadata search (`{DAV:}principal-property-search`) — out of scope.
- Quota properties (`{DAV:}quota-available-bytes`, `quota-used-bytes`) — quota sub-project.
- Sharing-aware permissions (`oc:share-types`, `oc:share-permissions`) — sharing sub-project.
- Tags / favorites listing endpoints (`/dav/systemtags/...`) — separate sub-project; SP5 only handles `{oc:}favorite` as a PROPPATCH-able prop.
- Range request multipart/byteranges — SP5 supports single Range only.
- WebDAV LOCK refresh (`LOCK` with empty body + `If:` header) — could be added in a follow-up; SP5 accepts fresh LOCKs only.

## 4. Load-bearing decisions

- **Thin protocol layer over 4c.** Every mutation goes through `View::*` or `Uploads::*` from 4c. SP5 does not bypass 4c to touch storage directly. The exception: `PropertyStore` + `LockStore` touch `oc_properties` and `oc_filelocks` directly via the existing `DbPool`.
- **`quick-xml` for all XML.** Writer for Multistatus / PROPFIND / PROPPATCH / LOCK responses; reader for PROPFIND / PROPPATCH / LOCK request bodies. No hand-rolled XML — too error-prone.
- **`oc_properties` path-keyed** (matches Nextcloud upstream). PROPPATCH rows are `(userid, propertypath, propertyname) -> propertyvalue`. The MOVE handler runs a single `UPDATE` to rewrite paths atomically with the rename.
- **`oc_filelocks` keyed by `"files/{uid}/{path}"`.** Locks are global per user-path. SP5 ships exclusive locks only; the `scope` column exists for future shared-lock support.
- **Conditional-write TOCTOU is documented limitation.** `View::stat` (compare etag) → `View::put_file` is two ops; a concurrent write between can clobber. Future hardening: a `View::put_file_if_match(expected_etag, ...)` that pushes the check into the storage trait. SP5 lives with the small window.
- **Part-tag transport via ETag-response + header-on-commit** (chosen per 4c prep notes §1 option a). PUT-chunk responses carry the part's sha256 (the `PartTag.etag` from 4a) in the response's `ETag` header. The MOVE-commit request sends `X-Crabcloud-Part-Tags: [{"part_number":1,"etag":"abc..."}]` JSON. No new DB table; storage layer remains the only state.
- **Lock check is hand-rolled per mutation method**, not middleware. The `If:` request header must be parsed by handlers anyway (each method has slightly different semantics), so a layer that pre-parses doesn't save work. A small `lock_check(locks, user_path, uid, &tokens)` helper centralizes the logic.
- **`Depth: infinity` PROPFIND rejected by default** (RFC 4918 §9.1 recommends it; Nextcloud matches). A future operator override could allow it; out of scope.

## 5. Crate + module layout

```
crates/crabcloud-http/src/routes/dav/                       NEW MODULE TREE
├── mod.rs                                                   router (dav_router) + mount both /remote.php/dav and /dav
├── extractor.rs                                             UserPath extractor from URL segments + auth user
├── headers.rs                                               Destination/Depth/If/Lock-Token/Timeout/Overwrite parsers
├── xml.rs                                                   Multistatus writer; shared XML helpers
├── methods.rs                                               OPTIONS, GET/HEAD, PUT, MKCOL, DELETE, MOVE, COPY
├── propfind.rs                                              PROPFIND handler + props builder
├── proppatch.rs                                             PROPPATCH handler
├── lock.rs                                                  LOCK + UNLOCK handlers
└── uploads.rs                                               chunked upload routes (begin/put/commit/abort)

crates/crabcloud-filecache/src/                              MODIFIED
├── lib.rs                                                   + properties + locks module exports
├── properties.rs                                            NEW: PropertyStore (read/write/rename oc_properties)
└── locks.rs                                                 NEW: LockStore (acquire/release/current oc_filelocks)

migrations/core/0005_webdav_props_and_locks/                 NEW
├── sqlite.sql
├── mysql.sql
└── postgres.sql

crates/crabcloud-http/Cargo.toml                             MODIFIED + quick-xml, urlencoding, httpdate
crates/crabcloud-http/src/router.rs                          MODIFIED + dav_router attached at /remote.php/dav and /dav
crates/crabcloud-db/src/core_migrations.rs                   MODIFIED + Migration entry version=5
Cargo.toml                                                    MODIFIED + workspace deps for quick-xml + urlencoding
```

**Cargo deps to add:**

- `quick-xml = "0.36"` with `serialize` feature.
- `urlencoding = "2"` (Destination header parsing).
- `httpdate` is already a workspace dep (used elsewhere); just consume.
- `uuid` is already a workspace dep; `Uuid::new_v4()` for lock tokens.

## 6. HTTP route surface

### 6.1 Router prefixes

`dav_router()` returns an `axum::Router` mounting all DAV methods under a flat key set. The application router (`crabcloud-http::router::build_router`) installs it at TWO prefixes:

- `/remote.php/dav` (legacy Nextcloud)
- `/dav` (modern alias)

Both forms resolve to the same handlers. The `AuthLayer` (from sub-project 2b) wraps both branches.

### 6.2 Files routes

| Method | URL | Handler |
|---|---|---|
| `OPTIONS` | `/dav/files/{user}/{*path}` | `methods::options` |
| `PROPFIND` | `/dav/files/{user}/{*path}` | `propfind::handle` |
| `GET` | `/dav/files/{user}/{*path}` | `methods::get_or_head` |
| `HEAD` | `/dav/files/{user}/{*path}` | `methods::get_or_head` |
| `PUT` | `/dav/files/{user}/{*path}` | `methods::put` |
| `MKCOL` | `/dav/files/{user}/{*path}` | `methods::mkcol` |
| `DELETE` | `/dav/files/{user}/{*path}` | `methods::delete` |
| `MOVE` | `/dav/files/{user}/{*path}` | `methods::move_` |
| `COPY` | `/dav/files/{user}/{*path}` | `methods::copy` |
| `PROPPATCH` | `/dav/files/{user}/{*path}` | `proppatch::handle` |
| `LOCK` | `/dav/files/{user}/{*path}` | `lock::acquire` |
| `UNLOCK` | `/dav/files/{user}/{*path}` | `lock::release` |

The `{user}` URL segment is matched against the authenticated user — mismatch returns `403`. Cross-user file access happens via shares (separate sub-project).

### 6.3 Uploads routes

| Method | URL | Handler |
|---|---|---|
| `MKCOL` | `/dav/uploads/{user}/{upload_id}` | `uploads::mkcol_begin` |
| `PUT` | `/dav/uploads/{user}/{upload_id}/{part_n}` | `uploads::put_chunk` |
| `MOVE` | `/dav/uploads/{user}/{upload_id}/.file` | `uploads::move_commit` |
| `DELETE` | `/dav/uploads/{user}/{upload_id}` | `uploads::delete_abort` |

The `MKCOL begin` request requires a `Destination:` header pointing at the eventual target. The handler calls `Uploads::begin(destination)`; the returned `upload_id` is what the path segment becomes (validated against the URL).

The `MOVE commit` requires `Destination:` AND `X-Crabcloud-Part-Tags:` headers. The body of `X-Crabcloud-Part-Tags` is JSON:

```json
[{"part_number":1,"etag":"sha256-hex..."},{"part_number":2,"etag":"sha256-hex..."}]
```

## 7. PROPFIND XML schema

### 7.1 Request body

Three forms accepted (per §2.1 of the brainstorming):
- `<propfind><prop>...named props...</prop></propfind>` — explicit list.
- `<propfind><allprop/></propfind>` — server returns all 10 props it knows.
- `<propfind><propname/></propfind>` — return only the prop names (empty values).

Plus `Depth: 0` (resource only) or `Depth: 1` (resource + immediate children). `Depth: infinity` → 403 `<error xmlns="DAV:"><propfind-finite-depth/></error>`.

### 7.2 Response body (Multistatus)

207 response with `<d:multistatus>` containing one `<d:response>` per resource. Each response has:

- `<d:href>` — the resource URL (URL-encoded path including `/dav/files/<user>/<path>`).
- One or more `<d:propstat>` blocks. Each propstat groups props by HTTP status (`200 OK` for found; `404 Not Found` for requested-but-unknown).

Example: see brainstorming §2.2.

### 7.3 Props supplied by SP5

10 props in two namespaces:

| Prop | Source | File | Directory |
|---|---|---|---|
| `{DAV:}getcontentlength` | `FileMetadata.size` | bytes | omitted |
| `{DAV:}getetag` | `FileMetadata.etag` | `"<40-hex>"` | `"<40-hex>"` |
| `{DAV:}getlastmodified` | `FileMetadata.mtime` | RFC 1123 | RFC 1123 |
| `{DAV:}getcontenttype` | `FileMetadata.mimetype` | mime string | omitted |
| `{DAV:}resourcetype` | `FileMetadata.kind` | `<resourcetype/>` empty | `<resourcetype><collection/></resourcetype>` |
| `{DAV:}displayname` | `path.basename()` | basename | basename (or empty for root) |
| `{oc:}id` | `FilecacheRow.fileid` + instanceid | `format!("{:020}{}", fileid, instanceid)` | same |
| `{oc:}permissions` | `FileMetadata.permissions` + kind | letter string | letter string |
| `{oc:}size` | `FilecacheRow.size` | same as getcontentlength | aggregated from filecache |
| `{oc:}favorite` | `oc_properties` lookup | `0` or `1` | `0` or `1` |

### 7.4 `oc:permissions` letter encoding

Compose by walking the bitmap:

- `S` (Shared with me) — never set in SP5 (no shares); reserved.
- `R` — present if `Permissions::SHARE` is set (re-share allowed).
- `D` — `Permissions::DELETE`.
- `N` + `V` + `W` — `Permissions::UPDATE` adds all three (Nextcloud convention: Rename + moVe + Write).
- `C` — `Permissions::CREATE` on files.
- `CK` — `Permissions::CREATE` on directories.

For `Permissions::full()` user-owned file: `"RDNVWCK"`. For directory: `"RDNVWCK"` (same letters; the `K` makes the difference). Helper function code in §2.5 of the brainstorming.

### 7.5 `oc:id` format

`format!("{:020}{}", fileid, instanceid)`. `fileid` is the `oc_filecache.fileid` BIGINT; `instanceid` comes from `AppState::config.instanceid`. The SP5 `propfind` handler calls `FileCache::lookup(storage_id, &storage_path)` to get the fileid alongside the metadata.

### 7.6 `oc:size`

For files: equal to `getcontentlength`. For directories: the aggregated size from `oc_filecache.size` (4b populates via ancestor-bumping). The `propfind` handler fetches via `FileCache::lookup`.

### 7.7 `oc:favorite`

Read from `oc_properties` where `propertyname = "{http://owncloud.org/ns}favorite"`. Defaults to `"0"` if absent. PROPPATCH writes it.

For Depth: 1, fetched via batched `PropertyStore::get_many(userid, &paths, "{http://owncloud.org/ns}favorite")` (one query per directory listing).

## 8. PROPPATCH semantics

### 8.1 `oc_properties` schema

| Column | SQLite | MySQL | Postgres | Notes |
|---|---|---|---|---|
| id | INTEGER PK AUTOINCREMENT | BIGINT UNSIGNED PK AUTO_INCREMENT | BIGSERIAL PK | row id |
| userid | TEXT NOT NULL | VARCHAR(64) NOT NULL | VARCHAR(64) NOT NULL | uid |
| propertypath | TEXT NOT NULL | VARCHAR(4000) NOT NULL | VARCHAR(4000) NOT NULL | path relative to user home (no leading `/`) |
| propertyname | TEXT NOT NULL | VARCHAR(255) NOT NULL | VARCHAR(255) NOT NULL | `{ns}name` form |
| propertyvalue | TEXT NULL | LONGTEXT NULL | TEXT NULL | XML inner text (string) |

Indexes:
- `(userid, propertypath)` — fast PROPFIND lookups.
- UNIQUE `(userid, propertypath, propertyname)` — one row per prop.

### 8.2 Request parsing

```xml
<propertyupdate xmlns="DAV:" xmlns:oc="http://owncloud.org/ns">
  <set><prop><oc:favorite>1</oc:favorite></prop></set>
  <remove><prop><oc:tags/></prop></remove>
</propertyupdate>
```

Handler:

1. Read request body; quick-xml parses into `Vec<PropOp>` where `PropOp::Set { name, value }` or `PropOp::Remove { name }`.
2. Resolve resource via `View::stat` (404 if missing).
3. Lock check (the resource itself + ancestor locks with depth=infinity).
4. For each `Set { name, value }`:
   - If `name` is in the protected list (DAV: props the server computes — `getetag`, `getcontentlength`, `getlastmodified`, `getcontenttype`, `resourcetype`, `displayname`, `oc:id`, `oc:permissions`, `oc:size`), respond with `403 Forbidden` for that prop.
   - Else `PropertyStore::upsert(userid, path, name, value).await?`.
5. For each `Remove { name }`:
   - Protected check (same list).
   - `PropertyStore::delete(userid, path, name).await?`.
6. Return `207 Multi-Status` with one `propstat` per prop indicating status.

### 8.3 Path rewrite on MOVE

When `View::rename(from, to)` succeeds, the SP5 MOVE handler also runs `PropertyStore::rename_path(userid, from_storage_path, to_storage_path)` — a single UPDATE that rewrites `propertypath` for the resource AND all its descendants (`WHERE propertypath = ? OR propertypath LIKE 'from/%'`).

Same for COPY (INSERT new rows derived from source rows).

## 9. LOCK + UNLOCK semantics

### 9.1 `oc_filelocks` schema

| Column | SQLite | MySQL | Postgres | Notes |
|---|---|---|---|---|
| id | INTEGER PK AUTOINCREMENT | BIGINT UNSIGNED PK AUTO_INCREMENT | BIGSERIAL PK | row id |
| key | TEXT NOT NULL UNIQUE | VARCHAR(2048) NOT NULL UNIQUE | VARCHAR(2048) NOT NULL UNIQUE | `"files/{uid}/{path}"` |
| ttl | INTEGER NOT NULL DEFAULT 86400 | INT NOT NULL DEFAULT 86400 | INTEGER NOT NULL DEFAULT 86400 | unix-ts expiry (0 = no expiry) |
| lock | INTEGER NOT NULL DEFAULT 0 | INT NOT NULL DEFAULT 0 | INTEGER NOT NULL DEFAULT 0 | 0 = unlocked; -1 = exclusive |
| token | TEXT NULL | VARCHAR(255) NULL | VARCHAR(255) NULL | `urn:uuid:<random>` |
| scope | TEXT NULL | VARCHAR(32) NULL | VARCHAR(32) NULL | `exclusive` (only value used in SP5) |
| depth | TEXT NULL | VARCHAR(32) NULL | VARCHAR(32) NULL | `0` or `infinity` |
| owner | TEXT NULL | VARCHAR(2048) NULL | VARCHAR(2048) NULL | request body `<owner>` XML inner |

### 9.2 LOCK request

```http
LOCK /dav/files/alice/photos/cat.jpg HTTP/1.1
Depth: 0
Timeout: Second-1800
Content-Type: application/xml

<?xml version="1.0"?>
<d:lockinfo xmlns:d="DAV:">
  <d:lockscope><d:exclusive/></d:lockscope>
  <d:locktype><d:write/></d:locktype>
  <d:owner><d:href>mailto:alice@example.com</d:href></d:owner>
</d:lockinfo>
```

Handler:

1. Resolve resource via `View::stat`. If missing, return 404 (LOCK on non-existing creates an empty resource per RFC 4918, but SP5 treats this as a 404 — Nextcloud-compatible).
2. Compute key: `format!("files/{uid}/{}", path.as_str().trim_start_matches('/'))`.
3. `LockStore::current(key)` — return existing lock if any.
4. If existing AND unexpired AND no matching token in `If:` header → return `423 Locked` with body `<error><lock-token-submitted/></error>` and a `<lockdiscovery>` containing the existing lock.
5. Compute timeout: parse `Timeout:` header (`Second-<N>` or `Infinite`); cap at 1800; default 1800.
6. Generate token: `format!("urn:uuid:{}", Uuid::new_v4())`.
7. `LockStore::acquire(key, token, "exclusive", depth, owner_xml, ttl=now+timeout)`.
8. Return `200 OK` body:

```xml
<?xml version="1.0"?>
<d:prop xmlns:d="DAV:">
  <d:lockdiscovery>
    <d:activelock>
      <d:locktype><d:write/></d:locktype>
      <d:lockscope><d:exclusive/></d:lockscope>
      <d:depth>0</d:depth>
      <d:owner>...</d:owner>
      <d:timeout>Second-1800</d:timeout>
      <d:locktoken><d:href>urn:uuid:<random></d:href></d:locktoken>
      <d:lockroot><d:href>/dav/files/alice/photos/cat.jpg</d:href></d:lockroot>
    </d:activelock>
  </d:lockdiscovery>
</d:prop>
```

Plus header `Lock-Token: <urn:uuid:<random>>` (note: `<` and `>` literal per RFC 4918 §10.5).

### 9.3 UNLOCK request

```http
UNLOCK /dav/files/alice/photos/cat.jpg HTTP/1.1
Lock-Token: <urn:uuid:abc-def-...>
```

Handler:

1. Parse `Lock-Token` header (strip `<` and `>`).
2. Compute key.
3. `LockStore::release(key, token)` — DELETE WHERE key=? AND token=?.
4. If 0 rows affected → `409 Conflict`.
5. Else `204 No Content`.

### 9.4 Lock-aware mutations

Every mutating method (PUT/MKCOL/DELETE/MOVE/COPY/PROPPATCH and LOCK-on-already-locked) runs `lock_check(locks, user_path, uid, &submitted_tokens)`:

1. Compute self key. Check `LockStore::current(self_key)`. If locked + no matching token → `423`.
2. Walk ancestors via `user_path.parent()` chain. For each ancestor:
   - Compute ancestor key.
   - If `LockStore::current(ancestor_key)` has `depth == "infinity"` AND lock unexpired AND no matching token → `423`.
3. Return `Ok(())` if no blocking lock.

Submitted tokens come from the `If:` header: SP5 parses the `<urn:uuid:...>` form only. Format: `If: (<urn:uuid:abc>)`. Multiple tokens separated by whitespace inside one paren-group.

### 9.5 Lock expiry

`LockStore::current(key)` filters by `ttl > unix_now() OR ttl = 0`. Expired rows persist until a future `crabcloud locks:gc` CLI subcommand sweeps them. The `acquire` path always upserts, overwriting any stale row.

## 10. Conditional headers + Range support

### 10.1 If-Match / If-None-Match

On PUT/MOVE/COPY/DELETE/PROPPATCH:

- `If-Match: "<etag>"` — resolve target; compare quoted etag to current. Mismatch → `412 Precondition Failed`.
- `If-Match: *` — require existing resource; 412 if missing.
- `If-None-Match: *` on PUT/MKCOL — require resource absent; 412 if present.

Check fires BEFORE the storage call. Documented TOCTOU between check and operation.

### 10.2 Overwrite on MOVE/COPY

The `Overwrite:` header gates whether MOVE/COPY can overwrite an existing destination:

- `Overwrite: T` (default per RFC 4918 §9.8.4) — overwrite allowed.
- `Overwrite: F` — destination must not exist; 412 if it does.

Handler resolves destination via `View::stat`; if it exists AND `Overwrite: F`, return `412 Precondition Failed`. Otherwise call `View::rename`/`copy` (which today overwrites the destination — verify against `Storage::rename`/`copy` behavior, which currently errors `AlreadyExists` for `LocalStorage`; SP5 may need a `View::put_or_overwrite` path or delete-then-rename).

**Known gotcha:** 4a's `Storage::rename`/`copy` returns `AlreadyExists` on existing destination. SP5's MOVE handler with `Overwrite: T` needs to DELETE the destination first (within the same lock check), then call rename/copy. Document this.

### 10.3 Range on GET

Single `Range: bytes=N-M`:

1. Resolve target via `View::stat`; get size.
2. Parse `Range: bytes=N-M`. Accepted forms: `bytes=0-499`, `bytes=500-`, `bytes=-500` (last 500 bytes).
3. If parse fails or range exceeds size → `416 Range Not Satisfiable` with `Content-Range: bytes */<size>`.
4. Else `206 Partial Content` with `Content-Range: bytes <N>-<M>/<size>` and body from `View::read_range(path, N..M+1)`.

Multi-range (`Range: bytes=0-499,1000-1499`) returns `416` (not supported).

## 11. Chunked upload routes

### 11.1 Begin (MKCOL)

```http
MKCOL /dav/uploads/alice/<upload_id> HTTP/1.1
Destination: https://crabcloud.example/dav/files/alice/big.zip
```

Handler:

1. Auth check (user = alice).
2. Parse `Destination:` header — absolute URL or path-only; extract the dav-files path.
3. Convert to `UserPath`.
4. Call `Uploads::begin(destination)` — returns `UploadHandle { upload_id, destination }`.
5. **Note:** the client picks the URL path's `<upload_id>` segment; the server's `Uploads::begin` produces its own opaque `upload_id`. SP5 must reconcile these. **Approach:** the server's `upload_id` IS what the client sees as the URL segment — meaning the server-issued `upload_id` is returned in the response (`X-Crabcloud-Upload-Id` header), and the client MUST use it for subsequent PUT/MOVE/DELETE. Effectively the client's MKCOL chooses an arbitrary URL segment but the server's response says "use this id from now on." Diverges slightly from upstream Nextcloud (which has the client choose); documented limitation.
6. Return `201 Created` with the `X-Crabcloud-Upload-Id` header.

Actually, simpler approach: **accept the client's chosen URL segment as the `upload_id`** and pass it as a hint to `Uploads::begin`. But 4c's `Uploads::begin` doesn't accept a hint — it derives the upload_id from the storage's multipart handle (which is base64-encoded). **Recompromise:** the URL-segment `<upload_id>` is an opaque token the client invents (random hex); the server uses it as a lookup key in an internal `DashMap<String, String>` (URL-segment → server-encoded-upload-id). The map is in-process; restarting the server loses in-progress uploads. **This is a known compromise.** A future hardening would store the URL-segment-to-server-id mapping in DB (or change the server-side upload_id encoding to BE the URL segment).

For SP5 ship: in-process `DashMap` for the URL-segment lookup. Lives on `AppState`.

### 11.2 Put chunk

```http
PUT /dav/uploads/alice/<upload_id>/<part_n> HTTP/1.1
<body bytes>
```

Handler:

1. Look up `<upload_id>` in the in-process map. 404 if unknown.
2. Get the server-encoded `upload_id`.
3. Call `Uploads::put_part(server_upload_id, part_n, body)` → `PartTag`.
4. Return `201 Created` with `ETag: "<part_tag.etag>"` header.

### 11.3 Commit (MOVE)

```http
MOVE /dav/uploads/alice/<upload_id>/.file HTTP/1.1
Destination: https://crabcloud.example/dav/files/alice/big.zip
X-Crabcloud-Part-Tags: [{"part_number":1,"etag":"sha..."},{"part_number":2,"etag":"sha..."}]
```

Handler:

1. Look up `<upload_id>` in the map.
2. Parse `Destination:` → `UserPath`. Must match the destination passed to `begin`.
3. Parse `X-Crabcloud-Part-Tags:` JSON → `Vec<PartTag>`.
4. Call `Uploads::commit(server_upload_id, destination, parts)` → `FileMetadata`.
5. Remove `<upload_id>` from the map.
6. Return `201 Created` with the destination's ETag in the response header.

### 11.4 Abort (DELETE)

```http
DELETE /dav/uploads/alice/<upload_id> HTTP/1.1
```

Handler:

1. Look up `<upload_id>` in the map. 404 if unknown.
2. Call `Uploads::abort(server_upload_id)` — idempotent.
3. Remove from map.
4. Return `204 No Content`.

## 12. Migration `0005_webdav_props_and_locks`

Three dialect files create the two tables. Sample (SQLite):

```sql
CREATE TABLE oc_properties (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    userid          TEXT    NOT NULL,
    propertypath    TEXT    NOT NULL,
    propertyname    TEXT    NOT NULL,
    propertyvalue   TEXT    NULL
);
CREATE        INDEX properties_pathonly   ON oc_properties (userid, propertypath);
CREATE UNIQUE INDEX properties_pathname   ON oc_properties (userid, propertypath, propertyname);

CREATE TABLE oc_filelocks (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    key     TEXT    NOT NULL UNIQUE,
    ttl     INTEGER NOT NULL DEFAULT 86400,
    lock    INTEGER NOT NULL DEFAULT 0,
    token   TEXT    NULL,
    scope   TEXT    NULL,
    depth   TEXT    NULL,
    owner   TEXT    NULL
);
```

MySQL + Postgres variants per the type-mapping in §3.1/§3.4. All migrations use `CREATE TABLE IF NOT EXISTS` and `CREATE INDEX IF NOT EXISTS` for idempotency.

Registered as version `5` in `crabcloud-db/src/core_migrations.rs`. The existing `core_migration_applies_against_sqlite` test asserts `applied == 5` after the bump. Same for `migrate_end_to_end.rs`.

## 13. Estimated batches (~7 PRs)

| Batch | Theme |
|-------|---|
| **A** | Migration `0005_webdav_props_and_locks` + `PropertyStore` + `LockStore` in `crabcloud-filecache` + per-store unit tests |
| **B** | DAV router skeleton + URL extractor + AuthLayer attached + OPTIONS handler. Plus GET/HEAD/PUT/MKCOL/DELETE methods + tests for conditional + Range |
| **C** | MOVE/COPY + Destination header parsing + Overwrite header + delete-then-overwrite |
| **D** | PROPFIND (10 props, Depth 0/1) + Multistatus XML writer + tests #3, #4 |
| **E** | PROPPATCH + path-keyed properties + favorite round-trip + path-rewrite on MOVE/COPY |
| **F** | LOCK/UNLOCK + If header parsing + ancestor-lock check + lock-aware mutation tests |
| **G** | Chunked-upload routes + in-process upload_id map + Playwright e2e + acceptance docs |

## 14. Acceptance criteria

| # | Criterion | Verified by |
|---|---|---|
| 1 | `cargo xtask check-all` clean (sqlite/mysql/postgres) | CI |
| 2 | Migration `0005_webdav_props_and_locks` creates 2 tables on all three dialects | migration test |
| 3 | PROPFIND Depth 0/1 returns the 10-prop set | integration |
| 4 | PROPFIND Depth: infinity returns 403 with propfind-finite-depth error body | integration |
| 5 | GET with single Range returns 206 + Content-Range; 416 on invalid | integration |
| 6 | PUT with If-Match mismatch returns 412 | integration |
| 7 | PUT with If-None-Match: * on existing returns 412 | integration |
| 8 | MKCOL/DELETE/MOVE/COPY happy paths | integration |
| 9 | `Overwrite: F` blocks MOVE/COPY onto existing | integration |
| 10 | OPTIONS advertises `DAV: 1, 2, 3` + supported methods | integration |
| 11 | PROPPATCH sets `oc:favorite`; PROPFIND reads it back | integration |
| 12 | PROPPATCH rejects protected props (403) | integration |
| 13 | PROPPATCH paths follow MOVE | integration |
| 14 | LOCK returns token; PUT without token on locked → 423 | integration |
| 15 | UNLOCK with wrong token → 409 | integration |
| 16 | Lock with Depth: infinity locks subtree | integration |
| 17 | Expired lock can be reacquired | integration |
| 18 | Chunked upload: MKCOL/PUT/MOVE/DELETE flow works | integration |
| 19 | Both `/remote.php/dav/files/...` AND `/dav/files/...` resolve | integration |
| 20 | Playwright e2e: full sync + chunked-upload + lock flow | e2e |
| 21 | Workspace `-D warnings` clean | CI |
| 22 | `git grep -i rustcloud` empty | CI |

## 15. Risks + mitigations

- **TOCTOU between conditional check and operation.** Documented. Future `View::put_file_if_match(etag, ...)` could push the check into the storage trait; not in SP5 scope.
- **`Overwrite: T` on MOVE/COPY** needs delete-then-rename because `Storage::rename`/`copy` errors on existing destination. Mitigation: dedicated `methods::move_or_overwrite` helper that DELETEs first if Overwrite=T. Lock check runs against both source and destination.
- **In-process upload_id map** loses in-progress uploads on server restart. Mitigation: documented operator concern; future hardening can persist the URL-segment-to-server-id mapping in DB.
- **`oc_properties` path-keyed semantics** mean MOVE must rewrite paths in one tx. Mitigation: a single `UPDATE oc_properties SET propertypath = REPLACE(propertypath, ?, ?) WHERE userid = ? AND (propertypath = ? OR propertypath LIKE ?)`. Tested by `proppatch_on_move_rewrites_path` integration test.
- **Lock-ttl orphans.** Mitigation: documented; the upsert-on-acquire pattern means stale rows get overwritten on the next LOCK for the same key. Operators can `crabcloud locks:gc` (future CLI) to actively reap.
- **PROPFIND XML size for Depth: 1 on large directories.** Mitigation: no limit in SP5; rely on the existing `RequestBodyLimitLayer` to cap incoming. Response size can grow large (10 props × N children); document as operator concern.
- **`urlencoding` Destination header parsing edge cases** — Nextcloud accepts both `Destination: /dav/files/...` and `Destination: https://host/dav/files/...`. SP5 parses both. Documented in handler tests.

## 16. Open questions (deferred)

- **`If:` header full grammar.** SP5 parses `(<urn:uuid:...>)` form only. RFC 4918 §10.4 has a more complex grammar (paren-groups, etag-list, NOT-prefix, etc.) that clients might use. Revisit if real clients fail.
- **LOCK refresh** (LOCK with empty body + `If:` header to extend timeout). Defer.
- **`{nc:}has-preview` and other Nextcloud-specific props** that desktop clients optionally request. Server returns 404 for unknown props per RFC; clients tolerate. Add later if needed.
- **Quota properties** (`{DAV:}quota-available-bytes`, `quota-used-bytes`). Defer to quota sub-project.
- **`{DAV:}supportedlock` PROPFIND response.** RFC 4918 §15.10 says servers should advertise it. SP5 deferred; clients work without.
- **Chunked-upload URL-segment hint** (better integration with `Uploads::begin`). Defer.

## 17. Dependencies on other sub-projects

- **Upstream:** 4a (Storage trait), 4b (FileCache + PropertyStore/LockStore tables live in `crabcloud-filecache`), 4c (View + Uploads façades + AppState factory methods), 2b (AuthLayer).
- **Downstream:** trash + versions sub-projects layer on `/dav/trashbin/...` and `/dav/versions/...`. Sharing sub-project layers `oc:share-types` + `share-permissions` props onto SP5's PROPFIND set. Calendar / contacts sub-projects implement `REPORT` method against their own collections.
