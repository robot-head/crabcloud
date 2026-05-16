# Filecache key translation for `SharedSubrootStorage` — Design

**Status:** spec — design only, no implementation.
**Date:** 2026-05-14
**Sub-project:** SP7 carryforward #2. The last carryforward before SP8 (public links).
**Context:** SP7 Batch C introduced `SharedSubrootStorage` to wrap an owner's storage at an `owner_path` with permission filtering. The wrapper's `id()` returns `inner.id()` so file_ids stay stable across recipients (spec SP7 §3.2). But every `Filecache::stat/list` call against the wrapper keys cache rows by `(wrapper.id() = inner.id(), recipient_relative_path)`. The recipient-relative paths collide with the owner's actual paths in her home — e.g., bob's `view.list("/Photos")` populates `(alice_id, /)` with `/Photos`-shaped metadata, poisoning alice's actual root row. Reads return correct data because the wrapper's storage call goes through `inner.list(translate(p))`; only the cache rows end up wrong, until the scanner repairs them.

## 1. Goal

Stop `SharedSubrootStorage` reads from poisoning `Filecache` rows in the owner's namespace. The wrapper continues to expose `inner.id()` (file_id continuity). Every `Filecache::stat/list` call against a wrapper now translates to the inner storage + owner's actual path before key construction. Cache rows go to alice's actual `(alice_id, /Photos/...)` rows, not the poisoning `(alice_id, /...)` rows.

**In scope:**

- One new method on the `Storage` trait: `inner_storage() -> Option<(&Arc<dyn Storage>, &StoragePath)>`, default `None`.
- `SharedSubrootStorage` implements it.
- `View::stat`, `View::list`, `View::list_with_meta` use a small `cache_key_for` helper that translates `(storage, path)` to `(inner_storage, inner_path)` before calling `Filecache`.
- The synthetic-entry stat in `View::list_with_meta` (which currently bypasses the cache to avoid poisoning) switches to the translated cache path.
- One regression test that pins down the no-poisoning invariant.
- One explicit file-id continuity test.

**Explicitly out of scope:**

- Other wrappers. `SharedSubrootStorage` is the only one; the trait method has a no-op default so future storages don't have to opt in.
- Storage event propagation. Already correct: writes go through `wrapper.put_file/mkdir/etc.` which delegate to `inner` with translated paths; `inner` emits StorageEvents with its own id + the translated path directly to the sink.
- Changes to `Filecache` itself. The translation lives at the call site (`View`), not inside the cache.
- SP8 public-link work. This unblocks SP8 by making the wrapper's caching behavior trustworthy; SP8 itself is out of scope.

## 2. Load-bearing decisions (brainstormed)

| # | Decision | Rationale |
|---|---|---|
| 1 | **Add `Storage::inner_storage() -> Option<(&Arc<dyn Storage>, &StoragePath)>`** to the trait with a default impl returning `None`. | Wrappers opt in; every existing implementor compiles unchanged. Returns the `(storage, path_prefix)` pair used for filecache key derivation. |
| 2 | **`SharedSubrootStorage::inner_storage` returns `Some((&self.inner, &self.owner_path))`.** Only override; all other storages keep the default. | One implementor, one call site. The wrapper already holds both pieces in its struct. |
| 3 | **Translation lives in `View`, not in `Filecache`.** A small helper `cache_key_for(storage, path) -> (storage_for_cache, path_for_cache)` consults `inner_storage()` and returns the translated pair. `View::stat / list / list_with_meta` call it before invoking `Filecache`. | The `Storage` trait stays clean of cache concerns; `Filecache`'s signature stays unchanged. View already owns the resolved-mount → storage mapping, so adding a one-line translation here is the smallest blast radius. |
| 4 | **Writes are not changed.** `SharedSubrootStorage::{put_file, mkdir, delete, rename, copy, begin_multipart}` delegate to `inner` with translated paths. The inner storage emits StorageEvents with `(inner_id, translated_path)` directly to the sink, never via the wrapper. So scanner cache populations are already correct. | Verified by reading the wrapper code: each mutating method does `self.inner.<op>(&self.translate(p)?, ..., sink)`. The sink sees inner's identity, not the wrapper's. |
| 5 | **The synthetic-entry stat in `View::list_with_meta` switches to the translated cache path** instead of bypassing the cache. The current SP7 Batch E bypass exists only because of this bug; with the fix in place, the synthetic stat can go through the cache and pick up the owner's actual root row. | Removes a workaround. Synthetic entries get cached metadata like any other entry. |
| 6 | **No new wrapper types in scope.** Only `SharedSubrootStorage` exists today; future storage stacking (external mounts, overlay storages, etc.) gets `inner_storage()` for free with the no-op default and can opt in if it needs the same behavior. | YAGNI; we don't speculate about hypothetical wrappers. The trait method's shape works for any future case but isn't designed around it. |

## 3. Trait surface change

```rust
// crates/crabcloud-storage/src/lib.rs
pub trait Storage: Send + Sync {
    fn id(&self) -> &str;
    // … existing methods …

    /// For wrappers that delegate to an inner storage at a sub-path:
    /// returns the inner storage and the owner-side path prefix.
    ///
    /// Callers that key caches by `(storage.id(), path)` should
    /// consult this and translate to `(inner.id(), prefix.join(path))`
    /// before lookup; otherwise the cache row keyed by the wrapper's
    /// (recipient-relative) path will collide with the owner's actual
    /// rows in the same storage namespace.
    ///
    /// Default: `None` — this storage is not a wrapper.
    fn inner_storage(&self) -> Option<(&Arc<dyn Storage>, &StoragePath)> {
        None
    }
}
```

## 4. `SharedSubrootStorage` impl

```rust
// crates/crabcloud-fs/src/storage/share_subroot.rs
impl Storage for SharedSubrootStorage {
    fn id(&self) -> &str { self.inner.id() }

    fn inner_storage(&self) -> Option<(&Arc<dyn Storage>, &StoragePath)> {
        Some((&self.inner, &self.owner_path))
    }

    // … all existing methods unchanged …
}
```

## 5. View call-site updates

```rust
// crates/crabcloud-fs/src/view.rs — new helper
fn cache_key_for(
    storage: &Arc<dyn Storage>,
    path: &StoragePath,
) -> FsResult<(Arc<dyn Storage>, StoragePath)> {
    match storage.inner_storage() {
        Some((inner, prefix)) => {
            let translated = if path.is_root() {
                prefix.clone()
            } else if prefix.is_root() {
                path.clone()
            } else {
                prefix.join(path.as_str())?
            };
            Ok((inner.clone(), translated))
        }
        None => Ok((storage.clone(), path.clone())),
    }
}
```

`View::stat`:

```rust
pub async fn stat(&self, user_path: &UserPath) -> FsResult<FileMetadata> {
    let (mount, storage_path) = self.resolve(user_path)?;
    let (cache_storage, cache_path) = cache_key_for(&mount.storage, &storage_path)?;
    let meta = self.filecache.stat(&cache_storage, &cache_path).await?;
    Ok(meta)
}
```

`View::list_with_meta` (the same translation; the `storage_path.is_root()` fallback to `mount.storage.list(root)` for `MemoryStorage` empty-root stays as-is):

```rust
pub async fn list_with_meta(&self, user_path: &UserPath) -> FsResult<Vec<ListedEntry>> {
    let (mount, storage_path) = self.resolve(user_path)?;
    let (cache_storage, cache_path) = cache_key_for(&mount.storage, &storage_path)?;
    // … listed_abs computation unchanged …
    let base_entries = if storage_path.is_root() {
        match self.filecache.list(&cache_storage, &cache_path).await {
            Ok(es) => es,
            Err(crabcloud_filecache::FileCacheError::NotFound)
            | Err(crabcloud_filecache::FileCacheError::Storage(
                crabcloud_storage::StorageError::NotFound,
            )) => mount.storage.list(&storage_path).await?,
            Err(e) => return Err(e.into()),
        }
    } else {
        self.filecache.list(&cache_storage, &cache_path).await?
    };
    // … child-mount enumeration unchanged …
    // For each share-mount child:
    //   Use cache_key_for(&child.storage, &StoragePath::root()) to get the
    //   (inner, owner_path) pair, then `filecache.stat(&inner, &owner_path)`.
    //   This replaces the SP7 Batch E bypass that called
    //   `child.storage.stat(&StoragePath::root())` directly.
}
```

`View::list` delegates to `list_with_meta` and drops the metadata — no change.

## 6. Tests

### `view_descend_into_share_does_not_poison_owner_cache`

New test in `crates/crabcloud-fs/tests/view_reads.rs`. The smallest test that pins down the no-poisoning invariant:

1. Seed alice's home with `/Photos/sunset.jpg` and `/notes.txt` (sibling of `/Photos`).
2. Construct bob's view with a share mount at `/Photos` (owner = alice).
3. Bob calls `view.list("/Photos")` — populates filecache.
4. Alice (via her direct home mount) calls `view.stat("/notes.txt")`. Read the resulting metadata.
5. Assert the metadata is `/notes.txt`'s, not `/Photos`'s.
6. As an additional check: directly query `filecache.lookup_by_id` for the owner's `/` row (if exposed) and confirm it has no `Photos`-shaped fields.

This test currently fails before the fix and passes after.

### `view_share_mount_preserves_file_id_continuity`

New test in `view_reads.rs`:

1. Seed alice's home with `/Photos/sunset.jpg`.
2. Alice stats `/Photos/sunset.jpg` (via her direct view) — capture `meta_alice.etag` and the underlying `fileid` if it's surfaced. (If the filecache row isn't directly accessible, compare `etag` instead — same invariant.)
3. Bob stats `/Photos/sunset.jpg` (via his share mount; user_path `/Photos/sunset.jpg`).
4. Assert `meta_bob.etag == meta_alice.etag`.

This explicitly pins down SP7 §3.2's file-id-continuity invariant against the new translation behavior.

### Existing tests continue to pass

- `view_list_root_surfaces_share_mount_entry`
- `view_list_share_mount_entry_carries_owners_metadata`
- `view_list_root_home_only_user_unchanged`
- `view_list_inside_share_mount_returns_owners_children`
- `view_list_dir_decorates_shared_by_for_recipient_and_share_count_for_owner` (server-fn-level)

These verify user-visible behavior, which is unchanged. Internal cache key shape changes, but the external API contract is the same.

## 7. Acceptance criteria

- `cargo fmt --all --check` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `cargo test --workspace` clean — both new tests and all existing view / sharing / DAV tests pass.
- The new `view_descend_into_share_does_not_poison_owner_cache` test fails when reverted to the pre-fix code (regression-guard property verified).
- The SP7 Batch E synthetic-entry stat bypass (`mount.storage.stat(&StoragePath::root())` in `view.rs`) is gone — replaced by the translated-cache-key path.

## 8. Risks and notes

- **`storage.clone()` cost.** `cache_key_for` clones the `Arc<dyn Storage>`. Arc-clone is cheap (one atomic increment), so this is fine for every `View::stat` / `View::list` call.
- **`StoragePath::join` failure.** Only fails on malformed input. The `storage_path` is already validated by `View::resolve`, so a failure here would represent an internal contract violation. `cache_key_for` propagates the error as `FsError::InvalidPath`.
- **Future storage stacking.** Two-level wrappers (e.g., a public-link viewer wrapping a `SharedSubrootStorage`) would need each layer's `inner_storage()` to chain correctly. The current design only follows one level. SP8's public-link path likely shares directly off the owner's home and not through another shared mount, so this isn't blocking — but worth noting for SP8 review.
- **`Filecache` doesn't need to know.** The cache stays cleanly storage-agnostic. Any future caller that bypasses View (e.g., a CLI scanner tool) would need to apply the same translation if it accepts arbitrary wrappers — but no such caller exists today.

## 9. Out-of-scope follow-ups (not in this spec)

- SP8 (public links). Unblocked by this fix; planned next.
- A `Storage` trait audit for other potential wrapper patterns. None exist today.
- A `Filecache` API that consumes the trait and does translation internally. Out of YAGNI; one call site is enough.
