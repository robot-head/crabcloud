# Folder Zip Download — Design (Sub-project 9)

**Status:** spec — design only, no implementation.
**Date:** 2026-05-16
**Sub-project:** 9 of ~13. Closes SP8 carryforward E7 (`/s/{token}/zip/{*path}`) and ships the same capability for authenticated users.

## 1. Goal

Ship streaming `application/zip` downloads of folders, available to both authenticated users (`GET /api/files/zip/{*path}`) and anonymous public-link viewers (`GET /s/{token}/zip/{*path}`). The handler walks the folder tree, enforces operator-configurable size caps, then streams a zip archive with selective DEFLATE compression and UTF-8 filename support.

**In scope:**

- New `crabcloud-zip` crate housing the streaming zip helper.
- Two HTTP handlers (authed + public) delegating to a shared helper.
- `FileConfig::folder_zip_max_entries` and `folder_zip_max_bytes` config knobs (defaults 500 / 2 GiB).
- Per-mime compression selection (DEFLATE for text-ish, STORED otherwise).
- UTF-8 filename support via general-purpose bit 11 + Info-ZIP Unicode Path extra field (0x7075) on every entry.
- E2E tests across both surfaces: happy path, over-cap rejection, unknown path, non-ASCII filename round-trip, and public-link permission gating.

**Explicitly out of scope (deferred):**

- Zip64 (large file support). Current caps keep us well under the 4 GiB single-file / 65,535 entry limits.
- Resumable / range downloads on zip output. Streaming can't honor `Range`.
- Encryption.
- Selective entry inclusion / exclusion (a `?include=...` query). Future SP if needed.

## 2. Load-bearing decisions (brainstormed)

| # | Decision | Rationale |
|---|---|---|
| 1 | **New crate `crabcloud-zip`** (small, focused; depends on `zip = "5"` from crates.io). | Keeps `crabcloud-fs` lean (zip is a presentation concern, not a filesystem concern). One owner for compression heuristics, header building, walk-then-stream orchestration. |
| 2 | **Symmetric authed + public handler.** Authed at `GET /api/files/zip/{*path}`, public at `GET /s/{token}/zip/{*path}`. Both delegate to a shared `crabcloud_zip::stream_folder(view, path, caps, dest)` helper. | Symmetric coverage matches the user's requested scope. Identical zip output and cap behavior across surfaces. The shared helper is the single source of truth for zip semantics. |
| 3 | **Operator-tunable caps via `FileConfig`.** New fields `folder_zip_max_entries: u64` (default 500) and `folder_zip_max_bytes: u64` (default `2_147_483_648` = 2 GiB). Pre-flight walk counts entries + sums sizes; either over budget → `413 Payload Too Large` with a JSON response body listing the actual count / size and the configured limits. | Config-driven over hardcoded per user preference. Defaults match SP8's original spec numbers. Tunable post-deploy via `config.php` without a recompile. |
| 4 | **DEFLATE on text-ish mimes only.** Per-entry decision: a small const list of "compressible" mime prefixes (`text/`, `application/json`, `application/javascript`, `application/xml`, `application/x-yaml`, `image/svg+xml`, `application/wasm`) → `CompressionMethod::Deflated` level 6. Everything else → `CompressionMethod::Stored`. Mime comes from the filecache's stored mime (cheap), with `application/octet-stream` falling through to STORED. | Avoids burning CPU compressing already-compressed bytes (jpg / png / mp4 / zip). Wins on source / code / config-heavy folders. |
| 5 | **Filename encoding: UTF-8 (general-purpose bit 11) AND Info-ZIP Unicode Path extra field (0x7075)** on every entry, set unconditionally. | Maximum compatibility. macOS, Linux, modern Windows, 7-Zip all honor at least one. The `zip` crate emits both when the right options are set. |
| 6 | **Pre-flight walk before any body bytes go on the wire.** Walk the folder tree (DFS), count entries, sum sizes. Over caps → `413` with JSON error body. Only after the pre-flight passes do we emit the first byte of the zip header. | Lets us return `413` cleanly with a structured error (rather than streaming half a zip then erroring mid-flight, which clients handle poorly). Cost: one extra `View::list` per directory in the tree — metadata-only, cheap. |
| 7 | **`Content-Disposition: attachment; filename="<basename>.zip"; filename*=UTF-8''<percent-encoded basename>.zip`** (RFC 6266 dual-form). Basename comes from the requested folder. Root requests use a surface-specific fallback: `<uid>.zip` for authed, `<token>.zip` for public links. | Browser-friendly download. RFC 6266 `filename*` for non-ASCII display names. The fallback covers "zip the whole mount" cleanly. |
| 8 | **HTTP method = `GET`.** No request body. Idempotent — re-running the same GET against an unchanged tree produces an equivalent zip. | Browser-friendly. Cacheable in theory (we won't set `Cache-Control` because content changes when underlying files change). |
| 9 | **Range header: ignored.** Streaming zip output can't honor `Range` cleanly (the central directory is built last; offsets aren't known until all entries have streamed). Handler returns `200` with the full zip and does NOT set `Accept-Ranges: bytes`. | Mirrors how Nextcloud handles this. Resumable folder downloads are out of scope. |
| 10 | **Permission check before walk.** Authed: the View already filters at list/read time; an unauthorized user gets `403` from `View::list` and the walk fails. Public link: `ctx.permissions.contains_read()` is checked explicitly before walking (same pattern as the download handler). Public file-drop / create-only links → `403` without walking. Public-link handler also rechecks `ctx.password_gate_required == false`. | Defense in depth. Avoids walking a tree we'd refuse to read anyway. |

## 3. Architecture

```
Authenticated user (browser, Files UI)
 └─ GET /api/files/zip/{*path}                  ← new authed handler

Anonymous public-link viewer
 └─ GET /s/{token}/zip/{*path}                  ← new public handler
     ├─ permission check: ctx.permissions.contains_read()
     ├─ password gate check: ctx.password_gate_required == false
     └─ PublicLinkMountResolver (Batch C, SP8)

Server
 ├─ crabcloud-zip  (NEW crate)
 │   ├─ stream_folder(view: &View, root: &StoragePath, caps: ZipCaps,
 │   │                writer: impl AsyncWrite + Send + Unpin)
 │   │   -> Result<ZipSummary, ZipError>
 │   │     1. walk_for_caps(view, root, &caps) -> Result<ZipPlan, WalkError>
 │   │        — DFS via View::list; aborts with TooLarge { count, bytes }
 │   │          on first overflow.
 │   │     2. For each PlannedEntry:
 │   │          pick compression (DEFLATE for text-ish mimes, STORED otherwise),
 │   │          open zip::write::ZipWriter::start_file with FileOptions {
 │   │              compression_method, large_file: false, unix_permissions
 │   │          } plus the UTF-8 + Info-ZIP unicode-name flags,
 │   │          copy bytes from View::read into the zip writer.
 │   │     3. Finish the central directory; return ZipSummary { entries, bytes }.
 │   ├─ ZipCaps { max_entries: u64, max_bytes: u64 }
 │   ├─ ZipPlan { entries: Vec<PlannedEntry>, total_bytes: u64 }
 │   ├─ PlannedEntry { storage_path: StoragePath, zip_name: String,
 │   │                 kind: PlanKind, size: u64, mtime: Option<DateTime<Utc>>,
 │   │                 mime: String }
 │   ├─ PlanKind { File, Dir }
 │   ├─ compression_for_mime(mime: &str) -> CompressionMethod  (const list)
 │   ├─ ZipError { Walk(WalkError), Io(io::Error), Zip(zip::result::ZipError) }
 │   └─ WalkError { TooLarge { count: u64, bytes: u64 }, View(FsError) }
 │
 ├─ FileConfig (extension)
 │     pub folder_zip_max_entries: u64,   // default 500
 │     pub folder_zip_max_bytes: u64,     // default 2 GiB
 │
 ├─ Authed handler (crabcloud-http, new module routes/files/zip.rs)
 │     extracts AuthContext, builds View, validates the requested path is a
 │     directory, delegates to stream_folder with caps from AppConfig.
 │
 └─ Public handler (crabcloud-http, extends routes/public_link/mod.rs)
       extracts PublicLinkAuthContext, checks read bit + password gate,
       builds View via PublicLinkMountResolver, delegates to stream_folder.
```

### 3.1 Data flow — authed user downloads `/Photos.zip`

1. `GET /api/files/zip/Photos` with session cookie.
2. Auth layer attaches `AuthContext`. Handler resolves `View` for the user via the existing `state.view_for(uid)` path.
3. `View::stat("/Photos")` confirms it's a directory. Not-a-directory → `400 Bad Request`. Not-found → `404`. Permission denied at the storage layer → `403`.
4. `walk_for_caps(view, "/Photos", caps)` walks the tree depth-first, summing entries and bytes. Encounters 423 files / 1.7 GiB total — under both caps. Returns `ZipPlan`.
5. Handler sets response headers: `Content-Type: application/zip`, `Content-Disposition: attachment; filename="Photos.zip"; filename*=UTF-8''Photos.zip`. Does NOT set `Accept-Ranges: bytes`.
6. Handler creates a `tokio::sync::mpsc` channel (`Bytes` items, bounded buffer). The response Body wraps the receiver via `axum::body::Body::from_stream`. A `tokio::spawn`'d task drives `crabcloud_zip::stream_folder` into the sender end (the sender is wrapped in an `AsyncWrite` adapter — `tokio_util::io::SinkWriter` or a small custom one).
7. The driver task iterates `ZipPlan.entries`. For each: `View::read(path)`, pick compression via `compression_for_mime(entry.mime)`, copy bytes into the `ZipWriter`. Empty / non-file entries become zero-byte directory entries (`Foo/`).
8. After the last entry, `ZipWriter::finish` emits the central directory. The sender drops; the receiver hits clean EOF; the client sees a complete zip.

### 3.2 Data flow — public link zip with 413

1. `GET /s/AbCd123Xyz0789Q/zip/HugeFolder`. The link is read-only over a 6 GiB folder.
2. `PublicLinkAuthLayer` attaches `PublicLinkAuthContext`. Handler checks read bit (`SharePermissions::from_wire(ctx.permissions).contains_read()`) → ok. Checks `ctx.password_gate_required == false` → ok (no password on this link).
3. Handler builds `View` via `PublicLinkMountResolver` (Batch C, SP8).
4. `walk_for_caps` counts 1247 files totaling 6.1 GiB. The byte total exceeds `max_bytes` (2 GiB).
5. Walk returns `Err(WalkError::TooLarge { count: 1247, bytes: 6_549_876_543 })`.
6. Handler returns `413 Payload Too Large` with JSON body:
   ```json
   {
     "error": "folder too large",
     "entries": 1247,
     "bytes": 6549876543,
     "limits": { "max_entries": 500, "max_bytes": 2147483648 }
   }
   ```
   No zip bytes have been emitted.

### 3.3 Compression selection

```rust
fn compression_for_mime(mime: &str) -> CompressionMethod {
    let lc = mime.to_ascii_lowercase();
    const COMPRESSIBLE: &[&str] = &[
        "text/",
        "application/json",
        "application/javascript",
        "application/xml",
        "application/x-yaml",
        "application/wasm",
        "image/svg+xml",
    ];
    if COMPRESSIBLE.iter().any(|p| lc.starts_with(p)) {
        CompressionMethod::Deflated
    } else {
        CompressionMethod::Stored
    }
}
```

Per-entry compression is a small const-list dispatch. New entries added by editing one place.

### 3.4 Streaming integration with axum

`axum::body::Body::from_stream` takes a `Stream<Item = Result<Bytes, _>>`. The plan:

1. Create `mpsc::channel::<Result<Bytes, io::Error>>(buffer = 32)`.
2. Wrap the `Sender` in an `AsyncWrite` adapter that forwards `poll_write` calls into `try_send` (or `send` if buffer is full, applying backpressure). The adapter implements `tokio::io::AsyncWrite`.
3. Spawn the zip writer task: `tokio::spawn(async move { stream_folder(view, root, caps, sender_adapter).await })`. Errors close the channel; readers see truncation.
4. Convert the receiver into a stream via `tokio_stream::wrappers::ReceiverStream` and hand it to `Body::from_stream`.

The adapter type is small (~40 lines). It lives in `crabcloud_zip` since it's the only consumer.

## 4. Testing strategy

The riskiest seams: (a) cap pre-flight is correct (under-counts wouldn't trigger 413; over-counts would 413 valid downloads), (b) UTF-8 filenames round-trip through the zip on read-back, (c) DEFLATE/STORED dispatch matches mime, (d) the public-link path actually checks the read bit AND the password gate, (e) the central directory is well-formed (clients accept it).

### 4.1 `crabcloud-zip` unit tests

- `walk_for_caps_counts_entries_recursively`: seeded 3-level tree, assert total entry count + byte total match.
- `walk_for_caps_returns_too_large_on_entries_overflow`: `max_entries = 2` against a 3-file folder → `TooLarge { count: 3, bytes: <sum> }`.
- `walk_for_caps_returns_too_large_on_bytes_overflow`: `max_bytes = 100` against a 200-byte file → `TooLarge`.
- `walk_for_caps_includes_empty_directories_as_entries`: an empty subfolder counts toward `max_entries` (it becomes a `Foo/` entry).
- `compression_for_mime_text_returns_deflated`: 7 known compressible mimes round-trip.
- `compression_for_mime_image_returns_stored`: jpg, png, mp4, zip, octet-stream → STORED.
- `stream_folder_produces_valid_zip`: write into an in-memory cursor; re-parse the buffer with `zip::ZipArchive`; assert entry list + each file's bytes match the seed.
- `stream_folder_preserves_unicode_names`: seed a file named `Vacaciónes — España.txt`; re-parse the zip and assert the entry name decodes correctly as UTF-8.
- `stream_folder_emits_directory_entries`: empty folder in the seed tree gets a `Foo/` entry in the central directory.
- `stream_folder_compresses_text_files_smaller`: seed a 10 KiB text file of repeated characters; assert the resulting zip entry's compressed size is < 1 KiB (DEFLATE worked).
- `stream_folder_stores_jpeg_unchanged`: seed a small valid JPEG; assert the entry's compressed size equals its uncompressed size (STORED chosen).

### 4.2 `crabcloud-http` e2e tests — authed surface

- `authed_zip_returns_200_application_zip`: GET, status 200, `Content-Type: application/zip`, `Content-Disposition` includes `attachment` and `filename=`.
- `authed_zip_body_parses_with_zip_archive`: read the body, parse with `zip::ZipArchive`, assert entry list matches seed.
- `authed_zip_over_cap_returns_413_with_summary`: configure caps to `max_entries=1` in `AppConfig`, request a 5-file folder, assert `413` with JSON body containing `entries: 5` and `limits.max_entries: 1`.
- `authed_zip_of_regular_file_returns_400`: zip endpoint pointed at a file (not a directory) → `400`.
- `authed_zip_unknown_path_returns_404`.
- `authed_zip_root_uses_uid_basename`: `GET /api/files/zip/` (or empty path) → `Content-Disposition` has `filename="<uid>.zip"`.
- `authed_zip_through_share_mount_works`: alice shares `/Photos` with bob; bob hits `/api/files/zip/Photos` (his recipient view); assert zip contains alice's files. Covers SP7 share-mount integration automatically.

### 4.3 `crabcloud-http` e2e tests — public-link surface

- `public_zip_read_link_returns_200`: read-link, normal folder → `200` with zip bytes; parse and verify file count.
- `public_zip_create_only_link_returns_403`: file-drop link tries to zip the root → `403`.
- `public_zip_password_gated_no_cookie_returns_403`: password-protected link, no cookie. The auth layer attaches `PublicLinkAuthContext` with `password_gate_required = true`. Handler returns `403 password_required` (matching SP8's download handler at `public_link/mod.rs:163-165`).
- `public_zip_expired_token_returns_404`.
- `public_zip_root_uses_basename`: `Content-Disposition` filename uses the linked-folder basename for non-root paths, and `<token>.zip` for the link root.
- `public_zip_unicode_filename_round_trips`: seed a non-ASCII filename, hit the public zip, parse the result, assert the entry name decodes as UTF-8.

### 4.4 Cross-cutting

- The `compressible_mime` decision is consistent between unit and e2e tests (the same mimes go through DEFLATE in both layers; no per-layer reinterpretation).
- The streaming pipeline doesn't leak descriptors on early client disconnect — when the receiver drops mid-stream, the sender task observes `SendError` and returns cleanly.

## 5. Risks & mitigations

| Risk | Mitigation |
|---|---|
| Pre-flight walk + zip generation hits the same `View::list` paths twice → double cost. | Walk is metadata-only (no body reads). The metadata cost is one filecache scan per directory, which is already fast. The actual stream phase is the I/O-heavy one and only runs after the cap check passes. |
| Streaming task panics mid-zip; client sees truncated download. | Channel `Sender` drop on panic closes the receiver cleanly; client sees EOF mid-archive. Standard zip readers fail to parse — better than half-good data. Log the panic from the spawned task. |
| Public link with `password_gate_required` could leak zip if handler forgets to check. | The defensive check `ctx.password_gate_required == false` is in the handler (same pattern as download / upload). e2e test covers it. |
| Large zip blocks a worker thread on DEFLATE for compressible folders. | The `zip` crate's `Write` impl is sync; we drive it from a dedicated `tokio::spawn`'d task that's free to block on CPU. The task is its own future; the runtime can schedule other work on other workers. If hot in profiling, future SP can move to `spawn_blocking`. |
| Authed user zips a folder they share-mount into — must work via SP7 `SharedSubrootStorage`. | No new logic. The `View` already handles the translation. Covered by the `authed_zip_through_share_mount_works` e2e test. |
| Filename traversal in zip entries (a malicious folder name like `../../etc/passwd`). | Owner-uploaded names are subject to existing path validation (`StoragePath::new` rejects `..` segments at write time). Zip entry names are derived from the existing storage paths, which are already sanitized. No additional sanitation needed at zip time. |
| Operator sets `folder_zip_max_bytes = 0`. | Pre-flight walk returns `TooLarge` on the first file; 413 returned. No crash. The config validator (`crabcloud-config`) can warn at startup if `max_bytes < 1 MiB` but doesn't have to reject. |
| Zip output size is non-deterministic across runs (DEFLATE compression varies with `flate2` version). | Tests parse the zip and assert content, never byte equality of the whole archive. No flaky comparisons. |

## 6. Future work / SP-later hooks

- Zip64 support: trivial flag flip in `FileOptions` when we lift caps past 4 GiB / 65k entries.
- Selective entry inclusion via `?include=path1&include=path2`: the walk would filter at directory traversal time.
- Cache-Control: zip output could carry `ETag` derived from the tree's `mtime` aggregate; clients could revalidate. Out of scope until requested.
- Background generation + signed URL pickup: for genuinely huge archives, generate to a temp location asynchronously, email the link. Different SP entirely (overlaps with SP11 email notifications).
