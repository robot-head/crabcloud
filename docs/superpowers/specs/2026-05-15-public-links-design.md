# Public links (read, password, expiration, anonymous WebDAV, file-drop) — Design (Sub-project 8)

**Status:** spec — design only, no implementation.
**Date:** 2026-05-15
**Sub-project:** 8 of ~13. Builds on SP7 (sharing user+group + `SharedSubrootStorage`). Schema columns landed in SP7 (`token`, `password`, `expiration`, `mail_send`, `share_type=3`); SP7's `Shares::create` returns `NotImplemented` for `share_type=3` — SP8 lifts that gate.

## 1. Goal

Ship Nextcloud-compatible public links: owners create read-only or upload-only ("file-drop") shareable URLs at `/s/{token}` for anonymous recipients. Recipients see a dedicated viewer page (browse + download, or an upload widget for file-drops) and can also drive the link with a Nextcloud desktop/mobile client over `/public.php/dav/files/{token}`. Links support optional password protection and optional expiration.

**In scope:**

- OCS link create / update / delete (`share_type=3`) on the existing `/ocs/v2.php/apps/files_sharing/api/v1/shares` surface.
- `/s/{token}` SSR viewer page (folder browse, file download, upload-only file-drop, password gate).
- `/public.php/dav/files/{token}/...` public WebDAV: `GET`, `PROPFIND`, `PUT` (file-drop).
- Password protection with signed-cookie unlock for the browser viewer + HTTP Basic for DAV clients.
- Expiration enforcement.
- Per-token password-attempt rate limit + per-IP file-drop upload rate limit (in-memory, MVP).
- Anonymous viewer reuses existing FileRow / Breadcrumb / upload widget primitives.
- New `crabcloud-publiclinks` crate (tokens, password hashing, unlock cookies, rate limiting, auth layer).
- New `PublicLinkMountResolver` in `crabcloud-fs`.

**Explicitly out of scope (deferred):**

- Email-on-share / `mail_send` actually sending mail (column is wired but ignored).
- Share notifications / activity stream.
- Federated public links.
- Link analytics / hit counters.
- Public-link previews / thumbnails (existing thumbnail endpoint requires auth; SP9 can extend).
- Per-link "hide download" flag.
- Public-link IP-allowlist / referrer-allowlist.
- Multi-node rate-limit state (in-memory only is documented as MVP; SP-later can swap for Redis).

## 2. Load-bearing decisions (brainstormed)

| # | Decision | Rationale |
|---|---|---|
| 1 | **Nextcloud-verbatim URL shape:** `/s/{token}` for viewer, `/public.php/dav/files/{token}/...` for WebDAV. Token = 15-char URL-safe random `[A-Za-z0-9]`. | One-shot compat with desktop & mobile clients that already speak Nextcloud's public-link surface. Matches token format byte-for-byte. |
| 2 | **Virtual user backed by share owner.** A new `PublicLinkAuthContext` resolves to `uid = owner_uid`, but `PublicLinkMountResolver` returns a single mount = `SharedSubrootStorage(owner_home, owner_path, link_permissions)` rooted at `/`. View / Filecache stay identical to a regular session. | No second code path through View. Permission denials happen at the storage wrapper, exactly like SP7 user shares. File-id continuity is automatic — the wrapper exposes the inner storage id via SP7's `Storage::inner_storage()` accessor, so Filecache rows live in the owner's namespace at the owner's path. |
| 3 | **Password unlock = signed HttpOnly cookie** named `pl_<token>`, value = base64(`exp_unix_le \|\| hmac_sha256(secret, token \|\| exp_unix_le)`). Scoped `Path=/`. `Secure` in production, `SameSite=Lax`. 1-hour TTL, refreshed on use. | Stateless server-side; survives reload; revoking the link instantly invalidates the cookie because the token vanishes from the DB and the auth layer rejects on lookup-miss. `Path=/` because the same cookie must travel to both `/s/{token}` and `/public.php/dav/...`; cookies are name-scoped per token via `pl_<token>`. |
| 4 | **Permissions stored as the existing `oc_share.permissions` bitmask.** Read link = `1` (read). File-drop = `4` (create), explicitly no read/list. Mixed (read + upload) = `1\|2\|4`. Bit `16` (re-share) always stripped. | Reuses SP7's `SharePermissions` type and the `SharedSubrootStorage` enforcement. No new permission concept. |
| 5 | **Token generator:** 15 chars from `[A-Za-z0-9]`, ~89 bits entropy. DB column `oc_share.token` is already `UNIQUE` and nullable. Collision retry on insert up to 3 attempts. | Matches Nextcloud token format. 89 bits is well clear of guessing risk for a single-node instance. |
| 6 | **Password storage:** Argon2id hash in `oc_share.password` (existing column, nullable, currently unused). Constant-time verification. NULL = no password. | Reuses the `argon2` crate already pulled in by `app_passwords`. |
| 7 | **Expiration enforcement at the auth layer.** On token resolve, if `expiration IS NOT NULL AND expiration < now()` → 404. Same response shape as "token doesn't exist". | Indistinguishability protects against token enumeration. Identical failure path as revoke. |
| 8 | **File-drop write path** = `<owner_path>/<basename(uploaded_name)>` with `(N)` collision suffix computed inside the new write, never overwriting. Quota check against owner's home quota before stream-write. Probe loop bounded to 50. | Matches Nextcloud default. Anonymous uploaders can't clobber each other. Owner is the only quota holder — anonymous user has none. |
| 9 | **Rate limiting:** in-memory `DashMap<token, AttemptLog>` for password attempts (10/hr per token → 429 for 1h) and `DashMap<ip, AttemptLog>` for file-drop POSTs (60/hr per IP → 429). Both reset on process restart. | Cheap. No new tables. Documented as MVP single-node; replaceable later. |
| 10 | **New crate scope:** `crabcloud-publiclinks` owns tokens / password verification / unlock cookies / rate-limit state / auth layer. `crabcloud-sharing` keeps the `oc_share` row schema and SP7's create/update/delete handlers — it learns to dispatch `share_type=3` to a new `LinkShares` collaborator backed by `crabcloud-publiclinks`. | Keeps SP7's sharing service focused on identity-based shares. Public-link concerns live in one place. |
| 11 | **Viewer page = dedicated dx 0.7 SSR route** at `/s/:token` and `/s/:token/*path`, with its own auth context. Reuses FileRow / Breadcrumb / upload widget components but not the Files page chrome. | Avoids conditional sprawl in the authed Files page. No risk of an anonymous request accidentally hitting authed routes. |

## 3. Architecture

```
Browser (anonymous)
 ├─ GET /s/{token}                          ← dx SSR viewer page (new)
 │   ├─ unlocked → folder browse / file preview / file-drop upload widget
 │   └─ locked   → password gate (POST /s/{token}/unlock sets pl_<token> cookie)
 ├─ GET /s/{token}/download/{*path}          ← stream download (read perm only)
 ├─ POST /s/{token}/upload/{filename}        ← file-drop upload (create perm only)
 ├─ GET /s/{token}/zip/{*path}               ← folder zip download (read perm only)
 └─ /public.php/dav/files/{token}/...        ← anonymous WebDAV (GET / PROPFIND / PUT)

Desktop / mobile clients
 └─ /public.php/dav/files/{token}/...        ← same as above; clients already speak this

Web UI (authenticated owner) — SP7 share modal gains a "Public link" tab
 └─ POST/PUT/DELETE /ocs/v2.php/apps/files_sharing/api/v1/shares (share_type=3)

Server
 ├─ axum router
 │   ├─ /s/{token}            → public_link_viewer subrouter (auth: PublicLinkAuthLayer)
 │   ├─ /public.php/dav/...   → existing dav subrouter, wrapped with PublicLinkAuthLayer
 │   └─ /ocs/.../shares       → existing OCS router; create-path now dispatches link type
 │
 ├─ crabcloud-publiclinks  (NEW crate)
 │   ├─ Tokens              generate / lookup-by-token / revoke
 │   ├─ Passwords           hash / verify (Argon2id)
 │   ├─ UnlockCookie        sign / verify HMAC blob; cookie name pl_<token>
 │   ├─ RateLimiter         per-token (password) + per-IP (upload) in-memory windows
 │   └─ PublicLinkAuthLayer axum middleware: resolves token, enforces expiry, gates on
 │                          cookie or Basic if password set, builds PublicLinkAuthContext,
 │                          attaches it as a request extension
 │
 ├─ crabcloud-sharing  (extended)
 │   ├─ Shares::create now branches on share_type=3:
 │   │    delegates token+password to crabcloud-publiclinks::Tokens / Passwords
 │   │    persists the row with token (always) + password_hash (if any) + expiration
 │   ├─ Shares::update for link-only fields (password, expiration, permissions, note)
 │   └─ Shares::delete unchanged; deletion clears the token (cookies become invalid on
 │      next lookup because Tokens::resolve returns None)
 │
 ├─ PublicLinkMountResolver  (crabcloud-fs — NEW)
 │   returns exactly one mount per request:
 │     path_prefix = "/"
 │     storage     = SharedSubrootStorage(owner_home, owner_path, link_permissions)
 │     metadata    = Some(MountMetadata{ owner_uid, permissions })
 │
 └─ existing layers unchanged: View, Filecache, dav, SSR
```

### 3.1 Data flow — anonymous download of a file inside a folder link

alice has a read-only link `AbCd123Xyz0789Q` on `/Vacation`, password-protected. bob already unlocked it.

1. Browser `GET /s/AbCd123Xyz0789Q/photos/beach.jpg/download`.
2. `PublicLinkAuthLayer` parses path → token `AbCd123Xyz0789Q`. Looks up share row. Found, not expired, password-protected.
3. Cookie `pl_AbCd123Xyz0789Q` present → verify HMAC + `exp > now()` → ok → build `PublicLinkAuthContext{ uid: alice, owner_path: /Vacation, permissions: read, link_share_id: 42 }` → attach as request extension.
4. Handler resolves `View` using `PublicLinkMountResolver` → single mount at `/` backed by `SharedSubrootStorage(alice_home, /Vacation, read)`.
5. `view.read("/photos/beach.jpg")` → wrapper translates → reads `/Vacation/photos/beach.jpg` from alice's storage. Stream to client.

### 3.2 Data flow — file-drop upload

alice has a create-only link `XyZ987abcdEFGH2` on `/Inbox`. Anonymous user uploads `holiday.jpg`.

1. Browser `POST /s/XyZ987abcdEFGH2/upload/holiday.jpg` (binary body).
2. Auth layer resolves token. Permission = create-only (`4`). Cookie required only if password set.
3. `RateLimiter::check_upload(ip)` → ok.
4. Quota: `crabcloud-users::Quota::remaining(alice)` vs `Content-Length`. Over → `507 Insufficient Storage`, no body written.
5. Filename sanitization: reject path separators, NUL, control chars, leading `..`.
6. `view.create("/holiday.jpg")` → SharedSubrootStorage checks bit `4` → ok → write through. On collision probe loop (max 50): `holiday (1).jpg`, `holiday (2).jpg`, …
7. Response `201 Created` with `{ name: "holiday (1).jpg" }`.

### 3.3 Data flow — password gate

1. Browser `GET /s/<token>`. Auth layer finds row, sees `password IS NOT NULL`, no `pl_<token>` cookie. Renders the **password gate** variant of the viewer page (200 OK).
2. User submits `POST /s/<token>/unlock` with `password=...`.
3. `RateLimiter::check_password_attempt(token)` → 11th attempt = 429 with `Retry-After: 3600`.
4. `Passwords::verify(stored_hash, supplied)` → ok.
5. Build cookie value `base64(exp_unix_le \|\| hmac_sha256(secret, token \|\| exp_unix_le))`, `exp = now + 3600`.
6. `Set-Cookie: pl_<token>=<value>; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=3600`, then `302 → /s/<token>`.
7. DAV clients receive `401 WWW-Authenticate: Basic realm="public-link"` instead of the password gate; they resend with `Authorization: Basic`, which the auth layer verifies against the hash on every request (no cookie path for DAV).

## 4. Data model & HTTP surface

### 4.1 Schema

No migration. SP7 already laid down every column public links need:

| Column | Use in SP8 |
|---|---|
| `share_type` | `3` for link |
| `share_with` | `NULL` for link rows; SP7's enforcement that `share_with` is non-NULL is relaxed only for `share_type=3` |
| `uid_owner` / `uid_initiator` | owner who created the link |
| `item_type` / `item_source` / `file_source` / `file_target` | resolved like SP7; `file_target` = absolute owner path |
| `permissions` | bitmask: read=`1`, file-drop=`4`, mixed=`1\|2\|4`; bit `16` stripped |
| `token` | 15-char URL-safe; `UNIQUE`; required (`NOT NULL` at the service level) for link rows |
| `password` | Argon2id hash or `NULL` |
| `expiration` | `DATETIME` or `NULL` |
| `stime` / `accepted` | `accepted=1` on insert (no anonymous acceptance flow) |

### 4.2 HTTP endpoints added

| Method + Path | Auth | Body / Response |
|---|---|---|
| `POST /ocs/v2.php/apps/files_sharing/api/v1/shares` with `shareType=3` | session (owner) | `path, permissions, password?, expireDate?`. Returns `{ url: "https://host/s/<token>", token, ... }` |
| `PUT /ocs/v2.php/apps/files_sharing/api/v1/shares/{id}` | session (owner) | fields: `password`, `expireDate`, `permissions`. SP7 already routes the verb; SP8 wires link-only fields. |
| `GET /s/{token}` | PublicLinkAuth | SSR viewer page. Password-required + cookie-missing → renders password gate variant. |
| `POST /s/{token}/unlock` | none (token only) | form `password=...`. Match → set `pl_<token>` cookie + 302 → `/s/{token}`. Mismatch → increment counter, re-render gate with error. |
| `GET /s/{token}/download/{*path}` | PublicLinkAuth (read bit) | streams file body; supports `Range`. |
| `GET /s/{token}/zip/{*path}` | PublicLinkAuth (read bit) | streams a zip of a folder. MVP cap = 500 entries / 2 GiB uncompressed → 413. |
| `POST /s/{token}/upload/{filename}` | PublicLinkAuth (create bit) | request body = file content. Returns `{ name: "..." }` (collision-suffixed if needed). |
| `* /public.php/dav/files/{token}/{*path}` | PublicLinkAuth | full WebDAV: `GET` / `PROPFIND` (read), `PUT` (create). `MKCOL` / `DELETE` / `MOVE` / `COPY` are 403'd by `SharedSubrootStorage` because the MVP link permission set does not grant delete/update. |

The viewer page is registered as a dx 0.7 SSR route alongside the existing Files route. Route key is `/s/:token` and `/s/:token/*path`. Hydration ships the same WASM bundle, but the page surface only exposes upload widget + download buttons; no settings, no sidebar, no user chip.

## 5. Auth flow detail

`PublicLinkAuthLayer` (axum middleware) does this work, in order:

1. **Parse token from path.** Single extractor handles both `/s/{token}/...` and `/public.php/dav/files/{token}/...`.
2. **DB lookup** by token via `Tokens::resolve(&token) -> Option<LinkRow>`. Miss → `404` (indistinguishable from "no such page").
3. **Expiration check.** `expiration IS NOT NULL AND expiration < now()` → `404` (same response as miss).
4. **Password gate** (only if `password IS NOT NULL`):
   - For `/s/{token}` paths: read cookie `pl_<token>`. If absent or HMAC-invalid or `exp <= now()` → render password gate variant (200).
   - For `/public.php/dav/...` paths: read `Authorization: Basic`. If absent → `401 WWW-Authenticate: Basic realm="public-link"`. If present → `RateLimiter::check_password_attempt(token)` first (over → `429`), then `Passwords::verify` against the row hash. Mismatch → `401`.
5. **Build `PublicLinkAuthContext`** carrying `uid = owner_uid`, `owner_path`, `permissions`, `link_share_id`. Attach as a request extension.
6. Downstream handlers / dav adapter pull the context and use `PublicLinkMountResolver` to build a `View`.

`POST /s/{token}/unlock`:

1. `RateLimiter::check_password_attempt(token)` → over → `429` + `Retry-After: 3600`.
2. `Passwords::verify(stored, supplied)` → mismatch → re-render gate with error message; increment counter.
3. Match: build cookie value, set headers, `302 → /s/{token}`.

The HMAC secret is `AppConfig::public_link_secret`, a 32-byte value loaded from config (env var `CC_PUBLIC_LINK_SECRET` or auto-generated and persisted to the data dir on first start, mirroring the session secret pattern).

## 6. File-drop semantics

`SharedSubrootStorage` learns one nuance: when permissions = create-only (bit `4` without bit `1`), `list_dir` and `stat` of children return `Forbidden` for everything except the linked root itself. The viewer page reads this signal to render the upload-only UI: no file list, no breadcrumb beyond the root, just the upload zone.

Upload handler (`POST /s/{token}/upload/{filename}` and `PUT /public.php/dav/files/{token}/{filename}`):

1. **Filename sanitization** (in the handler, not just in `StoragePath::new`): reject path separators, NUL, control chars, leading `..`. Returns `400 Bad Request` with a stable error code.
2. **Quota check:** `crabcloud-users::Quota::remaining(owner_uid)` vs `Content-Length`. Over → `507 Insufficient Storage`, no body written.
3. **Collision suffix:** probe via `view.stat(filename)`. If exists, try `name (1).ext`, `name (2).ext`, … up to 50. On 50 → `409 Conflict`.
4. **Stream-write** through `view.create_with(reader)`. View enforces the create bit; `SharedSubrootStorage` delegates to owner's storage, which updates owner's filecache + owner's quota.
5. Response `{ name: <final-name> }`, status `201 Created`.

## 7. Testing strategy

The riskiest seams are: (a) anonymous identity not leaking past the subroot, (b) password gate state across the cookie + DAV `Basic` paths, (c) file-drop quota / collision behavior, (d) file-id continuity when an owner browses the same file the link exposes, (e) filecache not poisoned by anonymous traversal.

### 7.1 `crabcloud-publiclinks` unit tests

- Token generator: format, length, character set, uniqueness across 10k samples.
- HMAC cookie: round-trip; tampered MAC rejected; expired cookie rejected; cookie keyed to a different token rejected.
- RateLimiter: window math, reset on hour rollover, 11th attempt rejected.
- Argon2id verify: correct password, wrong password, constant-time on mismatched lengths.

### 7.2 `crabcloud-sharing` integration tests (testcontainers, multidialect: sqlite / mysql / postgres)

- Link create returns a row with `token`; `Tokens::resolve(token)` finds it.
- Update password sets hash; clearing password sets `NULL`.
- Delete revokes the row from `Tokens::resolve`.
- Expiration set in the past → `Tokens::resolve` returns `None`.
- Create with `share_with` non-NULL for `share_type=3` is accepted (we store NULL anyway; SP7 invariant relaxed only for link type).

### 7.3 `crabcloud-http` e2e tests

- `GET /s/<token>` with no password → `200`, viewer renders.
- `GET /s/<token>` with password, no cookie → `200` password gate.
- `POST /s/<token>/unlock` correct pw → `302` + `Set-Cookie`; subsequent `GET` → `200` viewer.
- 11 wrong unlock attempts → `429` on the 11th and for an hour.
- `GET /s/<token>/download/<path>` read-link → `200` + body; create-only link → `403`.
- `POST /s/<token>/upload/<name>` create-link → `201` + body; file appears in owner's home at the linked path.
- File-drop name collision: two uploads of same filename → second gets `(1)` suffix.
- File-drop over quota → `507`, no file written.
- DAV `PROPFIND /public.php/dav/files/<token>/` → multistatus listing for read links; `403` for create-only.
- DAV `PUT /public.php/dav/files/<token>/foo.bin` with `Authorization: Basic` correct pw → `201`; wrong pw → `401`.
- Expired token → `404` across all surfaces.
- Revoked token (`DELETE` on the OCS share) → `404` across all surfaces, and a previously valid cookie no longer unlocks anything (because `Tokens::resolve` returns `None` before the cookie path is reached).
- **File-id continuity:** owner sees file at `/Vacation/photos/beach.jpg` with etag X; anonymous viewer hits the same file via the link → etag X.
- **No filecache poisoning:** anonymous viewer's traversal does NOT write rows under any synthetic storage id; cache rows go to `(owner_storage_id, owner_path/...)` only. Asserted by directly probing `filecache.lookup` after a list (same approach as SP7's share-mount regression test).

### 7.4 dx SSR smoke

`cargo run --bin smoke-public-link` (new) hits `/s/<seeded-token>` headless and asserts `200` + presence of the viewer mount node, then asserts `200` on `<token>/download/<file>` and matches bytes.

## 8. Risks & mitigations

| Risk | Mitigation |
|---|---|
| In-memory rate-limit state lost on restart, attackers wait for a deploy to keep trying passwords | Documented MVP limitation. The 89-bit token entropy makes brute-force impossible without first knowing the token. If a token leaks, the password is the second factor; restart resets after a deploy but real attackers face the per-token 10/hr cap during normal operation. SP-later swaps to durable counters if needed. |
| Anonymous WebDAV path could accidentally route through authed handlers if middleware order is wrong | Public WebDAV is mounted on `/public.php/dav/...`, a wholly distinct router from authed `/remote.php/dav/...`. The `PublicLinkAuthLayer` is the *only* auth middleware on that subrouter; the regular session/CSRF layers are not attached. e2e test asserts that authed cookies on a public route are ignored. |
| File-drop upload from an anonymous user could exhaust owner's quota | Quota is checked before write; `507` returned cleanly. Owner can size their quota expectations around the link. |
| Anonymous viewer's WASM bundle exposes server-fn endpoints that anonymous users shouldn't call | Server-fns are auth-context-gated at the function level. The viewer page should only call public-link-scoped server-fns; any attempt to invoke an authed server-fn returns 401. e2e test asserts a viewer bundle cannot trigger an authed server-fn. |
| Filename in upload includes `../` or absolute path | Sanitization in the handler returns `400` before reaching `StoragePath::new`. Defense in depth: `StoragePath::new` also rejects. |
| Two anonymous users uploading the same name race on the existence probe → both write `name.ext` | Collision probe + create uses an atomic create-if-not-exists at the storage layer. The probe loop catches the AlreadyExists error and retries with the next suffix. Bounded to 50 retries. |

## 9. Future work / SP-later hooks

- Email-on-share: a notifier crate dispatches on `mail_send=1` link creation.
- Federated links: separate SP; out of scope.
- Multi-node rate-limit state: swap `DashMap` for Redis / a DB-backed counter.
- Thumbnail support for anonymous viewer: extend the thumbnail endpoint to accept a `PublicLinkAuthContext`.
- Hide-download flag: new column, viewer hides the Download button, DAV `GET` returns 403.
- Per-link IP-allowlist / referrer-allowlist: new column carrying a CIDR list and / or referrer regex.
