# Sub-project 4b prep — File cache + S3 + event consumer

Notes captured during 4a implementation that should inform the 4b spec when we brainstorm it. **These are prep notes, not a spec** — the actual 4b spec will be authored via the brainstorming skill before 4b implementation begins.

## Filecache schema sketch

Mirror upstream Nextcloud's `oc_filecache` shape:

| Column | Type | Notes |
|---|---|---|
| fileid | BIGINT PK | autoincrement |
| storage | INT | FK -> `oc_storages.numeric_id` |
| path | TEXT | the same `StoragePath::as_str()` |
| path_hash | CHAR(32) | md5 of path; indexed for path lookups |
| parent | BIGINT | self-FK -> fileid; nullable for root |
| name | TEXT | basename |
| mimetype | INT | FK -> `oc_mimetypes.id` (interned) |
| mimepart | INT | FK -> `oc_mimetypes.id` for "type/" half |
| size | BIGINT | bytes; -1 for incomplete |
| mtime | INT | unix seconds |
| storage_mtime | INT | mtime as observed on the backing storage |
| encrypted | INT | 0 in 4b; future encryption sub-project |
| etag | VARCHAR(40) | matches `ETag::as_str()` |
| permissions | INT | bitmap; `Permissions::bits()` |
| checksum | TEXT | nullable; future checksum sub-project |

Auxiliary tables: `oc_storages` (numeric_id PK, id VARCHAR(64)), `oc_mimetypes` (id PK, mimetype VARCHAR).

## Event consumer shape

Add `ChannelEventSink` to `crabcloud-storage` or a new `crabcloud-storage-events` crate:

```rust
pub struct ChannelEventSink {
    tx: tokio::sync::broadcast::Sender<StorageEvent>,
}

impl ChannelEventSink {
    pub fn new(capacity: usize) -> Self { ... }
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<StorageEvent> { ... }
}

#[async_trait::async_trait]
impl EventSink for ChannelEventSink {
    async fn emit(&self, event: StorageEvent) {
        let _ = self.tx.send(event);
    }
}
```

Sub-project 4b's filecache scanner subscribes to one receiver and updates `oc_filecache` rows on each event. Lag is OK — the cache is eventually consistent with storage state.

## S3 backend sketch

Use `aws-sdk-s3` (official, async, multipart-first). Map:

- `id()` -> `format!("s3::{bucket}/{prefix}")`
- `put_file` short-circuits to `PutObjectRequest` for small bodies; multipart-or-die for large.
- `begin_multipart` -> `CreateMultipartUpload`; `upload_id` <- S3's UploadId.
- `put_part` -> `UploadPart`; `PartTag.etag` <- S3 part ETag.
- `commit_multipart` -> `CompleteMultipartUpload(parts: parts.into_iter().map(|p| CompletedPart{...}))`.
- `abort_multipart` -> `AbortMultipartUpload`.

Stat/list use `HeadObject` + `ListObjectsV2`. ETag from S3's ETag header (already hex-ish; we may need to normalize). Mimetype from S3 `Content-Type` (set on PUT from the same detect logic used by Local).

S3 doesn't support directories natively — we use the common `<prefix>/` empty-object convention, or fold directories into the filecache layer (skip the empty-object marker; filecache rows track directories).

## Scanner-driven drift recovery

In 4b add a `Scanner` that walks a storage from root and reconciles cache rows. Triggered by:

- Operator CLI (`crabcloud files:scan <storage>`).
- Startup-time check of last-scan timestamp (every N hours).
- 4b's broadcast channel having a stale `RecvError::Lagged` (recover by full-scanning the affected subtree).

## Open questions for 4b brainstorming

- Folder-size aggregation: write-through or scan-only? Nextcloud uses write-through with parent ETag bumping.
- ETag propagation: every mutation should bump every ancestor's ETag so desktop clients see "something changed" at the top. Write-through during sink consumption is the natural place.
- Cache-miss policy: on `stat` for a path not in cache, do we walk + populate (expensive) or 404 (consistency-flaky)? Recommend populate-with-locked-claim.
