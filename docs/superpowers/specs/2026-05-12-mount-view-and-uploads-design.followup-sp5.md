# Sub-project 5 prep — WebDAV (and Files API)

Notes captured during 4c implementation that should inform the sub-project 5 spec when we brainstorm it. **These are prep notes, not a spec** — the actual SP5 spec will be authored via the brainstorming skill before implementation begins.

## Scope sketch

Sub-project 5 adds HTTP routes that:

1. Implement WebDAV at `/remote.php/dav/files/<user>/<path>` (`PROPFIND`/`GET`/`PUT`/`MKCOL`/`DELETE`/`MOVE`/`COPY`).
2. Implement Nextcloud's chunked-upload protocol at `/remote.php/dav/uploads/<user>/<upload_id>/...`.
3. Re-export the same paths under `/dav/` (Nextcloud's modern alias).

All HTTP handlers call into `AppState::view_for(uid)` and `AppState::uploads_for(uid)`. The View+Uploads façades from 4c are the only state-mutation surface; WebDAV is a thin protocol layer.

## Trait-shape implications confirmed in 4c

- `View::stat` returns `FileMetadata` which is the right shape for `PROPFIND`'s response.
- `View::list` returns `Vec<DirEntry>` — matches PROPFIND's `Depth: 1` children.
- `View::read_range` matches HTTP `Range` requests (use `Range<u64>` from the `bytes=` header).
- `Uploads::commit` accepts a `Vec<PartTag>` — WebDAV must communicate part tags client-side. Two options:
  - (a) Server returns each `PartTag.etag` in the `PUT /uploads/<id>/<n>` response's `ETag` header; client sends them back in the MOVE request's `X-Crabcloud-Part-Tags` header (JSON-encoded).
  - (b) Server stores `PartTag`s in a per-upload state file in the storage's tempdir; commit re-reads them. Simpler protocol, more storage I/O.

Recommend (a) — keeps the storage layer stateless beyond the multipart primitives themselves.

## Operation mapping (WebDAV → View/Uploads)

| WebDAV request | Crabcloud handler call |
|---|---|
| `GET /dav/files/<user>/<path>` | `View::read(user_path)` |
| `GET /dav/files/<user>/<path>` with `Range:` header | `View::read_range(user_path, range)` |
| `PUT /dav/files/<user>/<path>` body | `View::put_file(user_path, body)` |
| `MKCOL /dav/files/<user>/<path>` | `View::mkdir(user_path)` |
| `DELETE /dav/files/<user>/<path>` | `View::delete(user_path)` |
| `MOVE /dav/files/<user>/<from>` with `Destination: /dav/files/<user>/<to>` | `View::rename(from, to)` |
| `COPY /dav/files/<user>/<from>` with `Destination:` header | `View::copy(from, to)` |
| `PROPFIND /dav/files/<user>/<path>` (Depth: 0) | `View::stat(user_path)` |
| `PROPFIND /dav/files/<user>/<path>` (Depth: 1) | `View::stat` + `View::list` |
| `MKCOL /dav/uploads/<user>/<id>` (after client computes random id) | `Uploads::begin(destination via Destination: header)` |
| `PUT /dav/uploads/<user>/<id>/<n>` | `Uploads::put_part(id, n, body)` |
| `MOVE /dav/uploads/<user>/<id>/.file` with `Destination:` | `Uploads::commit(id, destination, parts)` |
| `DELETE /dav/uploads/<user>/<id>` | `Uploads::abort(id)` |

## Open questions for sub-project 5 brainstorming

- **PROPFIND XML schema:** match Nextcloud's exact prop set (`{DAV:}getcontentlength`, `{DAV:}getetag`, `{DAV:}getlastmodified`, `{DAV:}getcontenttype`, `{DAV:}resourcetype`, `{http://owncloud.org/ns}id`, `{http://owncloud.org/ns}permissions`, etc.). XML library choice: `quick-xml`? Hand-rolled?
- **`Destination` header parsing:** absolute URL vs. path-only. Nextcloud accepts both.
- **Part tag transport:** as suggested above, ETag-in-response + custom-header-on-commit?
- **Auth on WebDAV routes:** `AuthLayer` (Bearer/Basic/Cookie) from 2b already works; just attach to the route tree.
- **`Depth` header validation:** Nextcloud limits PROPFIND to Depth ≤ 1 by default. Same here?
- **Range request semantics:** support multiple ranges? Nextcloud doesn't.
- **`If-Match` / `If-None-Match`:** for conditional PUT (concurrent-write safety). Needs View to expose etag verification.

## Estimated scope

5–7 batches: WebDAV routes (PROPFIND/GET/PUT/MKCOL/DELETE/MOVE/COPY) + chunked-upload routes + auth wiring + Playwright e2e using a real Nextcloud desktop client SDK (or the same Playwright HTTP patterns from earlier tests) + acceptance docs.
