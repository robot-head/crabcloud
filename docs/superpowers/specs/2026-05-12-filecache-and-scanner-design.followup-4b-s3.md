# Sub-project 4b-S3 prep — S3 backend

Notes captured during 4b implementation that should inform the 4b-S3 spec when we brainstorm it. **These are prep notes, not a spec** — the actual 4b-S3 spec will be authored via the brainstorming skill before implementation begins.

## Scope sketch

Add `S3Storage` to `crabcloud-storage`. Plug it into the existing `Storage` trait without changes (4a's trait was designed for this). Multipart semantics map cleanly to S3's UploadPart/CompleteMultipartUpload. `Scanner::register_storage` accepts the new backend without modification.

## Crate + dep choices

Use `aws-sdk-s3` (official, async, multipart-first). Workspace deps to add:

- `aws-sdk-s3 = "1"`
- `aws-config = "1"`
- `aws-credential-types = "1"`

S3 backend lives in `crabcloud-storage/src/s3/` alongside `local/` and `memory/`.

## Operation mapping

| Storage method | S3 op |
|---|---|
| `id()` | `format!("s3::{bucket}/{prefix}")` |
| `stat(path)` | `HeadObject` (or `ListObjectsV2` with prefix for directories) |
| `exists(path)` | `HeadObject` with 404 catch |
| `list(path)` | `ListObjectsV2` with `Delimiter=/` |
| `read(path)` | `GetObject` |
| `read_range(path, range)` | `GetObject` with `Range` header |
| `put_file(path, body)` | `PutObject` for small bodies; `CreateMultipartUpload` + chunked PUT for large |
| `mkdir(path)` | `PutObject` with `Key=prefix/` and empty body (S3 directory convention) |
| `delete(path)` | `DeleteObject` (single) or `DeleteObjects` (batched for directories) |
| `rename(from, to)` | `CopyObject` + `DeleteObject` (S3 has no native rename) |
| `copy(from, to)` | `CopyObject` |
| `begin_multipart(target)` | `CreateMultipartUpload`; `upload_id` ← S3's UploadId |
| `put_part(handle, n, body)` | `UploadPart`; `PartTag.etag` ← S3 part ETag |
| `commit_multipart(handle, parts)` | `CompleteMultipartUpload` |
| `abort_multipart(handle)` | `AbortMultipartUpload` |

## ETag normalization

S3 returns ETags as quoted strings, sometimes with a `-<part_count>` suffix for multipart uploads. The 4a contract expects 40-char lowercase hex. Two options:

1. **Mint a synthetic ETag** (random hex via `ETag::new()`) on every PUT/multipart commit. Stash it in object metadata (`x-amz-meta-crabcloud-etag`). Reads pull the metadata. Decouples our ETag from S3's. Cost: one extra metadata read per stat.
2. **Use S3's ETag as-is** (strip quotes; hash-md5-strip-suffix transformation). Cheaper but couples our cache to S3's hashing.

Recommend option 1 — matches LocalStorage's xattr persistence pattern.

## Mimetype + permissions

- Mimetype on PUT: set `ContentType` from the same detection logic LocalStorage uses (extension table + magic-byte sniff). Stat reads `ContentType`.
- Permissions: bucket-policy-mediated, not per-object. Map to `Permissions::full()` for owned objects. Future: integrate with S3 bucket ACLs.

## Directories

S3 has no real directories. Two patterns coexist in the wild:

- **Empty-object marker** (`prefix/`): explicit; visible via `ListObjectsV2`.
- **Prefix-only**: derived from the existence of child objects.

Recommend the empty-object marker for parity with how filecache rows track directories; emit one on `mkdir`. `list(prefix)` then sees both real markers and synthetic derivations via the `CommonPrefixes` response field.

## Drift recovery

S3 console writes won't fire `StorageEvent`s. `Scanner::full_scan` is the only reconciler. Operators should be told to `crabcloud files:scan s3::<bucket>/<prefix>` after out-of-band uploads.

## Open questions for 4b-S3 brainstorming

- **Region + credentials config:** static config block, env vars, or AWS SDK default chain (recommended)?
- **Object size limits:** S3 supports 5 TiB objects; multipart-part minimum is 5 MiB. Do we enforce the 5 MiB minimum in the storage trait, or push it to the backend?
- **Presigned URLs:** offer a method for clients to upload directly to S3 without proxying through our server? Bypasses the event sink — the scanner reconciles via `files:scan`.
- **`ListObjectsV2` pagination:** 1000-objects-per-call default. `list()` should paginate transparently.
- **Eventual consistency:** read-after-write is now strong on S3, but cross-region consistency lags. Document the assumption.
- **Test strategy:** use `minio` testcontainer or LocalStack? Both work; minio is simpler.
