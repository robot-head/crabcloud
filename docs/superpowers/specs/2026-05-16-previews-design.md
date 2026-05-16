# Public-link previews + thumbnails — Design (Sub-project 10)

**Status:** spec — design only, no implementation.
**Date:** 2026-05-16
**Sub-project:** 10 of ~13. Adds on-demand thumbnail generation for image and PDF source files on both the authed Files surface and the anonymous public-link viewer. Builds on SP8 (`PublicLinkAuthContext`, `PublicLinkMountResolver`) and SP9 (the established `crabcloud-zip` crate shape for new presentation crates).

## 1. Goal

Ship on-demand thumbnail generation for image files (JPEG / PNG / GIF / WebP) and PDF first-page renders, served from two HTTP endpoints: `GET /api/files/preview/{fileid}?size=N` (authenticated) and `GET /s/{token}/preview/{*path}?size=N` (public-link). The Files UI replaces its generic file icons with inline `<img>` tags pointing at these endpoints. Thumbnails are generated synchronously on first request, cached on disk under `<data_dir>/appdata/preview/<storage_id>/<fileid>/<size>-<sourceetag>.jpg`, and served from cache thereafter.

**In scope:**

- New `crabcloud-preview` crate housing the per-mime provider trait, image + PDF backends, on-disk cache, per-key generation lock, and HTTP-stable error mapping.
- Two HTTP handlers (authed + public) delegating to the same `PreviewCache::get_or_render` helper.
- Files UI: inline thumbnails in `FileRow` + `PublicListing`, with graceful fallback to the generic icon on 404 / 415.
- Per-storage cleanup of stale cache entries when source ETag changes (on read; lazy).
- Permission gating on public-link side: read bit required; create-only → 403; password-gate enforced.
- E2E coverage on both surfaces, plus unit coverage of each provider backend.

**Explicitly out of scope (deferred):**

- Video thumbnails (would require ffmpeg or similar).
- Office document thumbnails (`.docx` / `.xlsx` / `.pptx`).
- Async / background pre-generation. Always lazy / on-demand for MVP.
- Cleanup task that walks the cache and removes orphans. MVP relies on read-time lazy invalidation.
- Animated GIF / WebP preservation. We always emit a still JPEG.
- HEIC / RAW / SVG. SVG is technically an image but needs server-side rendering (different pipeline).
- Cache size caps.

## 2. Load-bearing decisions (brainstormed)

| # | Decision | Rationale |
|---|---|---|
| 1 | **New crate `crabcloud-preview`**. Depends on `image = "0.25"`, `hayro` (pure-Rust PDF renderer), `tokio`, `tracing`, `dashmap`. | Same shape as `crabcloud-zip`: focused presentation crate, no DB coupling. Each provider is its own module under `providers/`. |
| 2 | **Provider trait + per-mime dispatch.** A `PreviewProvider` trait with `async fn render(source_bytes: Vec<u8>, size_px: u32) -> ProviderResult<Vec<u8>>` (returns JPEG bytes). Implementations: `ImageProvider` (image crate), `PdfProvider` (hayro). Dispatch via `fn provider_for_mime(mime: &str) -> Option<&'static dyn PreviewProvider>` over a small const table. Unsupported mimes return `PreviewError::Unsupported`. | Each backend is isolated and unit-testable. New providers (SVG, video) are additive. The signature takes owned `Vec<u8>` so providers can hand it to `spawn_blocking` cleanly. |
| 3 | **Hayro for PDF** (pure-Rust renderer, per user choice). | No binary deps, single-binary release stays small. We render page 0 only. Pin a minor version; provider trait makes swap-out contained. |
| 4 | **On-disk cache** under `<data_dir>/appdata/preview/<storage_id>/<fileid>/<size>-<source_etag>.jpg`. | Mirrors Nextcloud's `appdata_<instanceid>/preview/` layout. Stale entries get cleaned lazily when source etag changes (list the `<fileid>/` directory, delete entries whose etag prefix doesn't match, after a successful write of the new one). No DB schema needed. |
| 5 | **Synchronous on-demand generation with per-key dedup lock.** A `DashMap<(String, i64, u32), Arc<tokio::sync::OnceCell<ProviderResult<PathBuf>>>>` keyed on `(storage_id, fileid, size)`. Concurrent requests for the same preview share one render. The entry is removed from the map after the render completes; subsequent cache hits don't even touch the map. | User chose synchronous. Dedup prevents thundering-herd on first cache miss. `OnceCell` lets followers `await` the result rather than re-acquiring the lock. |
| 6 | **Fixed size ladder: `[64, 256, 1024]` pixels.** Client passes `?size=N`. Server rounds UP to the nearest ladder rung; `N > 1024` → `400 Bad Request`. Cache filenames encode the ladder rung, not the requested N. | Bounded cache footprint (3 sizes × file count). Matches Nextcloud's `forceSize` discipline. UI scales any of 64/256/1024 via CSS for in-between widths. |
| 7 | **Always emit JPEG, quality 80.** Source mime is irrelevant for output; cache filename ends in `.jpg`; `Content-Type: image/jpeg` unconditionally. | One cache layout. JPEG has the best size / quality tradeoff for photos; for line-art or screenshots the loss is minor at thumbnail sizes. Simplifies clients. |
| 8 | **Source freshness via ETag in the cache filename, not mtime comparison.** Each cached preview encodes the source's ETag in its filename. A read for the current ETag either finds the matching file (cache hit) or doesn't (render fresh + write new file; on success, delete any sibling files in the `<fileid>/` directory matching the same `<size>-` prefix with a different etag). | Cheap to compare (string equality); no stat race; old generations get garbage-collected on next access. |
| 9 | **Public-link preview gate** mirrors `download_handler`: `password_gate_required == false` AND `SharePermissions::from_wire(ctx.permissions).contains_read()`. Create-only links → 403. | Per user choice: read bit required, no opt-in for file-drop previews. Consistent with sibling endpoints. |
| 10 | **HTTP response headers**: `Content-Type: image/jpeg`, `ETag: "<source_etag>-<size>"`, `Cache-Control: private, max-age=86400`. Conditional GET via `If-None-Match` returns `304 Not Modified`. | Browsers + clients revalidate cheaply. The composite ETag lets us bump the ladder rung or the encoder version without thrashing — change the composite, force a refresh. |
| 11 | **Errors map to deterministic statuses**: source not found → `404`; mime unsupported → `415`; size out of range → `400`; render failure → `500` (logged). The Files UI treats `404` / `415` identically — falls back to the generic icon via `<img onerror>`. | Lets the Files UI shed thumbnails for files it can't preview without retrying. |
| 12 | **Files UI integration**: `FileRow` renders `<img src=/api/files/preview/{fileid}?size=64 onerror=fallbackToIcon>` for entries whose mime starts with `image/` or equals `application/pdf`. Non-previewable mimes render the existing generic icon (no `<img>` request emitted). Same pattern in the public-link `PublicListing` component. | Tight: don't make wasted requests. The mime allowlist lives client-side so we don't probe the server for every `.zip` row. The `onerror` covers the rare case where the server can't render an allowlisted mime. |

## 3. Architecture

```
Files UI (FileRow + PublicListing)
 └─ <img src=/api/files/preview/{fileid}?size=64> on each previewable row

Browser anonymous viewer
 └─ <img src=/s/{token}/preview/{*path}?size=64> on each previewable row

Authed user (other clients, e.g. mobile)
 └─ GET /api/files/preview/{fileid}?size=N

Anonymous viewer
 └─ GET /s/{token}/preview/{*path}?size=N

Server
 ├─ crabcloud-preview  (NEW crate)
 │   ├─ PreviewProvider trait + ProviderResult / PreviewError
 │   │     async fn render(source: Vec<u8>, size_px: u32) -> ProviderResult<Vec<u8>>  (JPEG)
 │   ├─ ImageProvider: image::ImageReader → resize → JpegEncoder (qual 80)
 │   ├─ PdfProvider: hayro::render page 0 → image::DynamicImage → resize → JPEG
 │   ├─ provider_for_mime(mime: &str) -> Option<&'static dyn PreviewProvider>
 │   ├─ PreviewCache::get_or_render(...)
 │   │     1. Map size_px to ladder rung (64/256/1024); 400 if out of range
 │   │     2. Compose cache path:
 │   │        <data_dir>/appdata/preview/<storage_id>/<fileid>/<size>-<etag>.jpg
 │   │     3. If exists → return Path + file metadata for axum stream
 │   │     4. Else: take per-key OnceCell lock, re-check, then
 │   │        - look up provider for source mime; 415 if none
 │   │        - View::read(source path) → bytes
 │   │        - provider.render(bytes, size) → Vec<u8> (JPEG) via spawn_blocking
 │   │        - atomically write to cache path via temp-file + rename
 │   │        - sweep sibling entries with prefix "<size>-" but different etag
 │   ├─ PreviewError mapped at the handler boundary to 400 / 404 / 415 / 500
 │   └─ LADDER: &[u32] = &[64, 256, 1024]; round_up_to_ladder(n) -> Option<u32>
 │
 ├─ FileConfig (extension)
 │     pub preview_root: PathBuf,    // default <data_dir>/appdata/preview
 │     pub preview_max_pixels: u32,  // safety cap on source decode, default 64M
 │
 ├─ Authed handler (crabcloud-http, new routes/files_preview.rs)
 │     extracts AuthenticatedUser, builds View, looks up filecache row for
 │     {fileid} (scoped to the user's home storage via the storage factory),
 │     delegates to PreviewCache::get_or_render.
 │
 └─ Public handler (crabcloud-http, new routes/public_link/preview.rs)
       extracts PublicLinkAuthContext, checks read bit + password gate, builds
       View via PublicLinkMountResolver, resolves *path to user_path, delegates
       to PreviewCache::get_or_render.
```

### 3.1 Data flow — authed thumbnail request

1. `GET /api/files/preview/42?size=64` with session cookie or bearer.
2. Auth layer attaches `AuthenticatedUser`. Handler builds `View` via `state.view_for(uid)`, resolves the user's home storage id via `state.storage_factory.home_storage(uid).await?.id()`, then `state.filecache.lookup_by_id(42)` (which returns a `FilecacheRow` carrying the file's own `storage_id`). Not found → `404`. Row's `storage_id` does not match the user's home (and no incoming share mount of the user exposes that storage_id) → `404` (file-id leak resistance: don't differentiate from "not found"). Successful match yields the source row (storage path + mime + etag).
3. `provider_for_mime(row.mime)` → `Some(&ImageProvider)`. Otherwise → `415`.
4. `PreviewCache::get_or_render(storage_id, fileid=42, size=64, etag, view, source_path, provider)`:
   - Ladder snap: `size=64` → 64 (already on ladder).
   - Cache path: `<data_dir>/appdata/preview/<storage_id>/42/64-<etag>.jpg`.
   - Cache hit → open + stat for `Content-Length`; respond.
5. Response: `200 OK`, `Content-Type: image/jpeg`, `ETag: "<etag>-64"`, `Cache-Control: private, max-age=86400`. Body streamed via `tokio::fs::File` + `tokio_util::io::ReaderStream`.

### 3.2 Data flow — cache miss with concurrent requests

1. Two browsers simultaneously request `?size=256` for the same fileid.
2. Both reach `PreviewCache::get_or_render`. First to grab the entry creates a new `Arc<OnceCell<ProviderResult<PathBuf>>>` in the dedup `DashMap`; second sees the existing cell and awaits.
3. The winner runs the render: `View::read` source bytes, `spawn_blocking` into the provider, encode JPEG, atomic-rename into the cache path, sweep stale siblings. Sets the cell's `Ok(path)`.
4. Both await the same `OnceCell::get_or_init` future; both resume with the same `Ok(path)`.
5. After both responses complete, the `DashMap` entry is dropped (`Arc<OnceCell>` falls to zero refs). Next request reads straight from disk — no dedup overhead on cache hits.

### 3.3 Data flow — public link with create-only permissions

1. `GET /s/AbCd123Xyz0789Q/preview/holiday.jpg?size=256` (a file-drop link).
2. `PublicLinkAuthLayer` attaches `PublicLinkAuthContext`. Handler checks `ctx.password_gate_required == false` → ok. `SharePermissions::from_wire(ctx.permissions).contains_read()` → **false** (create-only is bit 4 only) → **`403 read_not_granted`** without touching the View.

### 3.4 Data flow — source ETag changed (stale cache)

1. Owner overwrites the file. New filecache row has `etag = "newetag456"`.
2. Next preview request → cache path is `<...>/<fileid>/64-newetag456.jpg`. The directory still contains `64-oldetag123.jpg` from before, but that filename doesn't match.
3. Cache miss → render fresh → atomic-rename into the new path → sweep all `64-*.jpg` siblings except the just-written one. Old preview deleted.
4. Subsequent requests hit cache.

### 3.5 Conditional GET (304)

1. Client GETs `/api/files/preview/42?size=64`. Response includes `ETag: "<etag>-64"`.
2. Client re-requests with `If-None-Match: "<etag>-64"`.
3. Handler computes the expected ETag from the current filecache row + ladder rung, compares to the header, returns `304 Not Modified` with no body. No need to even open the cache file.
4. If the source has changed (filecache row's `etag` differs), the comparison fails, and the handler proceeds to the cache hit / render path as usual.

## 4. Testing strategy

The riskiest seams: (a) cache key construction (a wrong key gives bad data to every consumer until manual cleanup), (b) provider dispatch (a wrong mime mapping serves a `415` for something users expect to work), (c) the per-key dedup lock under load (lockup or thundering-herd), (d) ETag-based staleness, (e) public-link permission semantics, (f) source-too-large rejection.

### 4.1 `crabcloud-preview` unit tests

- `ladder_rounds_up_within_range`: 16 → 64, 64 → 64, 65 → 256, 256 → 256, 1024 → 1024.
- `ladder_rejects_above_1024`: 1025 → `Err(SizeOutOfRange)`.
- `provider_for_mime_image`: jpg, png, gif, webp → `Some(_)`; pdf → `Some(_)`; mp4, zip, octet → `None`.
- `image_provider_resizes_jpeg`: seed an 800×600 JPEG; render at 256; resulting bytes decode as a valid JPEG with max dimension ≤ 256.
- `image_provider_preserves_aspect`: seed a 1024×512; render at 256; resulting image is 256×128 (long edge = ladder, short edge scaled proportionally).
- `image_provider_strips_animation`: seed a 2-frame animated GIF; render at 64; result is a single-frame JPEG (decoded frame count == 1).
- `image_provider_rejects_oversize_source`: seed a 5000×5000 image with `preview_max_pixels = 4_000_000` cap; render → `Err(SourceTooLarge)`.
- `pdf_provider_renders_first_page_of_two`: seed a 2-page PDF; render at 256; result decodes as a valid JPEG of the first page only.
- `pdf_provider_handles_empty_pdf`: seed a 0-page PDF; render → `Err(RenderFailed)` (handler will surface this as `500` + logged).
- `cache_hit_returns_existing_path`: pre-seed a file in the cache dir; `get_or_render` returns the path without calling any provider (counter-instrumented test provider records 0 calls).
- `cache_miss_renders_and_writes`: empty cache; first request invokes provider; resulting file exists at the expected path.
- `cache_sweeps_stale_siblings_on_write`: pre-seed `64-oldetag.jpg`; request with new etag; assert old file gone after write.
- `dedup_lock_serializes_concurrent_renders`: spawn 10 concurrent `get_or_render` calls with the same key; instrumentation counter inside a test provider records `<= 1` invocation.

### 4.2 `crabcloud-http` e2e — authed surface

- `preview_returns_jpeg_for_image_file`: seed a JPEG file owned by alice; GET `/api/files/preview/{id}?size=64` → 200, `Content-Type: image/jpeg`, body decodes as a valid JPEG with max dimension ≤ 64.
- `preview_returns_jpeg_for_pdf`: seed a 1-page PDF; GET → 200, JPEG.
- `preview_unsupported_mime_returns_415`: seed a `.zip`; GET → 415.
- `preview_unknown_fileid_returns_404`: GET with a never-existing fileid.
- `preview_cross_user_fileid_returns_404`: bob requests alice's fileid → 404 (NOT 403 — don't leak existence).
- `preview_size_out_of_ladder_rounds_up`: request `?size=200`; assert response body matches the 256-rung cache file.
- `preview_size_too_large_returns_400`: `?size=2048` → 400.
- `preview_etag_revalidation_returns_304`: first GET, capture `ETag`; second GET with `If-None-Match: <etag>` → 304.
- `preview_source_modified_returns_new_etag`: GET, modify source, GET again; new `ETag` header value; the previous `If-None-Match` no longer 304s.
- `preview_no_auth_returns_401`: no Authorization header → 401 (mirrors `files_zip` Batch B fix).

### 4.3 `crabcloud-http` e2e — public surface

- `public_preview_read_link_returns_jpeg`: read-only link on a folder of images; GET `/s/<token>/preview/cat.jpg?size=64` → 200, JPEG.
- `public_preview_create_only_link_returns_403`: file-drop link → 403.
- `public_preview_password_gated_no_cookie_returns_403`: password-protected link, no cookie → 403 + body contains `password_required`.
- `public_preview_expired_token_returns_404`.
- `public_preview_path_traversal_returns_404`: `?path=../../etc/passwd` → 404 (`UserPath::new` rejects `..` segments).
- `public_preview_unsupported_mime_returns_415`: link contains a `.zip`; preview that → 415.

### 4.4 Files UI smoke

- `crates/crabcloud-app/tests/server_fns_files.rs`: SSR snapshot of `FileRow` for a previewable mime confirms the `<img src=/api/files/preview/{fileid}?size=64>` tag is present.
- `crates/crabcloud-app/tests/server_fns_public_link.rs`: same for `PublicListing` rows.

## 5. Risks & mitigations

| Risk | Mitigation |
|---|---|
| `hayro` panics on a malformed PDF and brings down the worker. | Provider call runs inside `tokio::task::spawn_blocking` (rendering is CPU-bound anyway). A panic there is caught by `JoinError`; the handler returns `500` and the rest of the server stays up. |
| Source image is a 50,000×50,000 JPEG that exhausts memory during decode. | `FileConfig::preview_max_pixels` (default 64 megapixels) is checked against the decoded dimensions BEFORE the resize step. Over-budget → render fails with `PreviewError::SourceTooLarge` → `413`. `image::ImageReader::with_guessed_format` followed by `into_dimensions()` lets us peek before the full decode. |
| Cache directory fills disk. | Documented limitation: no auto-cleanup task in MVP. Operator can `rm -rf <data_dir>/appdata/preview/<storage_id>` to reset a storage's cache. Follow-up SP can add a sweeper. |
| Concurrent first-request thundering herd renders the same preview N times. | `DashMap<key, Arc<OnceCell>>` dedup lock. Unit test `dedup_lock_serializes_concurrent_renders` proves it. |
| Two replicas (multi-node deploy) both decide to render the same preview. | Filesystem cache is per-node; both render once, both write to their respective local disk. Acceptable for MVP. Multi-node deduplication is SP-later if needed (shared NFS or Redis lock). |
| ETag string contains filesystem-hostile characters. | `crabcloud_storage::ETag` is 40 lowercase-hex chars (per SP6); safe verbatim in filenames. Audit comment + a test pins the format. |
| Public-link preview leaks file existence via timing differences between 404 and 415. | The handler doesn't probe filesystem before the auth + permission check. The 415 path doesn't reach the source. The 404 path requires `View::stat` which is uniform timing. Documented; not a hard guarantee. |
| Files UI sends thousands of preview requests when a user scrolls a 10k-entry folder. | Browser natively limits parallel `<img>` fetches per origin. We rely on that. No server-side rate limit in MVP. If abusive, the existing axum body limits + per-IP throttle (not yet implemented) catch it. |
| Hayro's API changes break our PDF provider. | Pin a minor version in `Cargo.toml`. Provider trait abstracts away the renderer; swap-out is contained. |
| Source file is a very large PDF (100s of pages). | We render page 0 only; page count doesn't affect per-request cost. `hayro` is lazy about non-rendered pages. |
| User overrides `preview_root` to a path outside `data_dir`. | Configuration is operator territory; we validate the path is absolute and writable at startup, but don't sandbox it. Standard Unix permissions apply. |

## 6. Future work / SP-later hooks

- Video thumbnails: a `VideoProvider` that shells out to `ffmpeg -ss 1 -frames:v 1 input.mp4 - | image-resize`.
- Office docs: `.docx` / `.xlsx` / `.pptx` → render via libreoffice headless or unoconv.
- Cache sweeper: walks `preview/` periodically and deletes orphans (no matching filecache row).
- Multi-node coordination: Redis-backed dedup lock so two replicas don't both render the same preview.
- Cache size caps: LRU eviction once `preview_root` exceeds an operator-configured byte budget.
- Animated WebP preview preservation.
- HEIC / RAW providers using the `libheif` and `rawloader` crates.
