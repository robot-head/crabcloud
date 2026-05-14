# Filecache key translation for share-mount wrapper — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop `SharedSubrootStorage` reads from poisoning `Filecache` rows in the owner's namespace, while preserving file-id continuity across recipients.

**Architecture:** Add `Storage::inner_storage() -> Option<(&Arc<dyn Storage>, &StoragePath)>` to the trait with a `None` default. `SharedSubrootStorage` returns `Some((&inner, &owner_path))`. `View::stat / list / list_with_meta` consult a new `cache_key_for` helper that translates `(wrapper, recipient_path)` to `(inner, owner_path.join(recipient_path))` before invoking `Filecache`. Reads stay correct; cache rows go to alice's actual namespace, not the poisoning one. Writes are unaffected — they delegate to `inner` directly and emit events from `inner`'s id.

**Tech Stack:** Rust 1.95, existing `crabcloud-storage`, `crabcloud-fs`, `crabcloud-filecache` crates. No new deps.

**Spec:** `docs/superpowers/specs/2026-05-14-filecache-share-mount-translation-design.md`

---

## Conventions

- **Single branch / single PR.** All tasks land in one PR — the scope is small enough that splitting adds overhead without value. Branch off `origin/master`:
  ```bash
  cd "C:/Users/Matt Stone/git/crabcloud"
  git fetch origin master
  git switch -c fix/filecache-share-translation origin/master
  ```
- **Commit cadence:** one commit per task (see step labeled "Commit" in each task).
- **Pre-PR check:**
  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```
  All three must pass.
- **Established workaround to remove:** Task 3 deletes the SP7 Batch E synthetic-entry stat bypass in `view.rs:181-204` and replaces it with the new translated cache call. The doc-comment there documents why the bypass existed — that paragraph also goes.

---

## Task 1: Add `inner_storage()` to the `Storage` trait

**Files:**
- Modify: `crates/crabcloud-storage/src/lib.rs` (the `Storage` trait definition)

- [ ] **Step 1: Add the trait method with a default impl**

Open `crates/crabcloud-storage/src/lib.rs`. Find the `pub trait Storage: Send + Sync {` block (around line 119). Immediately after the `fn id(&self) -> &str;` declaration, add:

```rust
    /// For wrappers that delegate to an inner storage at a sub-path:
    /// returns the inner storage and the owner-side path prefix.
    ///
    /// Callers that key caches by `(storage.id(), path)` should consult
    /// this and translate to `(inner.id(), prefix.join(path))` before
    /// lookup; otherwise the cache row keyed by the wrapper's
    /// (recipient-relative) path will collide with the owner's actual
    /// rows in the same storage namespace.
    ///
    /// Default: `None` — this storage is not a wrapper.
    fn inner_storage(&self) -> Option<(&std::sync::Arc<dyn Storage>, &StoragePath)> {
        None
    }
```

(If the file already imports `Arc`, drop the `std::sync::` prefix. If `StoragePath` is in scope at the trait declaration, drop that prefix too. Check the existing imports at the top of the file.)

- [ ] **Step 2: Verify the workspace still compiles**

Run:
```bash
cargo check --workspace
```
Expected: PASS. All existing `impl Storage` blocks compile unchanged because the new method has a default implementation.

- [ ] **Step 3: Commit**

```bash
git add crates/crabcloud-storage/src/lib.rs
git commit -m "fs: add Storage::inner_storage trait method with None default"
```

---

## Task 2: Implement `inner_storage` on `SharedSubrootStorage`

**Files:**
- Modify: `crates/crabcloud-fs/src/storage/share_subroot.rs`

- [ ] **Step 1: Override the trait method**

In `crates/crabcloud-fs/src/storage/share_subroot.rs`, find the `impl Storage for SharedSubrootStorage { ... }` block. Inside it, immediately after `fn id(&self) -> &str { self.inner.id() }`, add:

```rust
    fn inner_storage(&self) -> Option<(&std::sync::Arc<dyn Storage>, &StoragePath)> {
        Some((&self.inner, &self.owner_path))
    }
```

(Same import-prefix note as Task 1: drop `std::sync::` if `Arc` is already in scope; check the file's imports.)

- [ ] **Step 2: Verify**

```bash
cargo check -p crabcloud-fs
```
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/crabcloud-fs/src/storage/share_subroot.rs
git commit -m "fs: SharedSubrootStorage exposes (inner, owner_path) via inner_storage"
```

---

## Task 3: Translate before filecache in `View::stat / list / list_with_meta`

**Files:**
- Modify: `crates/crabcloud-fs/src/view.rs`

This task does three things in one edit (they're tightly coupled):
1. Add a private `cache_key_for` helper.
2. Update `View::stat` to call it.
3. Update `View::list_with_meta` to call it AND drop the SP7 Batch E synthetic-entry bypass.

- [ ] **Step 1: Add the `cache_key_for` helper**

In `crates/crabcloud-fs/src/view.rs`, add this function at the top of the file (after the imports, before `impl View`), or at the bottom of the file as a private function. Either placement is fine; pick whichever matches the file's existing style. The function body:

```rust
/// Translate a `(storage, path)` pair before filecache lookup, so that
/// `Storage` wrappers (e.g. `SharedSubrootStorage`) route cache rows
/// through the underlying owner storage and owner-side path instead of
/// the recipient-relative path. See spec
/// `docs/superpowers/specs/2026-05-14-filecache-share-mount-translation-design.md`.
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

`Arc`, `StoragePath`, `FsResult`, and `Storage` should already be in scope at the top of `view.rs`. If `FsResult` isn't, use the actual error type the rest of the file uses for `View::resolve`'s return — search for `fn resolve(&self, ...) -> ` to find the exact signature.

- [ ] **Step 2: Update `View::stat`**

Find the existing `View::stat` (around line 95):
```rust
pub async fn stat(&self, user_path: &UserPath) -> FsResult<FileMetadata> {
    let (mount, storage_path) = self.resolve(user_path)?;
    let meta = self.filecache.stat(&mount.storage, &storage_path).await?;
    Ok(meta)
}
```

Replace the body with:
```rust
pub async fn stat(&self, user_path: &UserPath) -> FsResult<FileMetadata> {
    let (mount, storage_path) = self.resolve(user_path)?;
    let (cache_storage, cache_path) = cache_key_for(&mount.storage, &storage_path)?;
    let meta = self.filecache.stat(&cache_storage, &cache_path).await?;
    Ok(meta)
}
```

- [ ] **Step 3: Update `View::list_with_meta`**

Find `View::list_with_meta` (around line 122). The function has two distinct filecache call sites that both need translation, plus the synthetic-entry stat bypass at the end.

**3a. Translate the base-listing filecache call** (around line 144). Replace:
```rust
let base_entries = if storage_path.is_root() {
    match self.filecache.list(&mount.storage, &storage_path).await {
        Ok(es) => es,
        Err(crabcloud_filecache::FileCacheError::NotFound)
        | Err(crabcloud_filecache::FileCacheError::Storage(
            crabcloud_storage::StorageError::NotFound,
        )) => mount.storage.list(&storage_path).await?,
        Err(e) => return Err(e.into()),
    }
} else {
    self.filecache.list(&mount.storage, &storage_path).await?
};
```

With:
```rust
let (cache_storage, cache_path) = cache_key_for(&mount.storage, &storage_path)?;
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
```

Note: the `NotFound` fallback still calls `mount.storage.list(&storage_path)` (the wrapper) — that's correct, it goes through the wrapper's translation to inner.

**3b. Switch the synthetic-entry stat from bypass-to-storage to translated-cache-call.** Around lines 181-204 there's a block that currently does:
```rust
// Stat directly through the wrapped storage (bypassing the
// filecache populate path). The wrapper translates root →
// `owner_path` and hits the owner's backing storage, so the
// returned metadata is the OWNER's view of the shared dir.
// We deliberately skip `FileCache::stat` here: that call
// would key the resulting cache row by `(wrapper.id(), root)`
// (alice's storage_id, recipient-relative root), which would
// poison alice's actual `/` row with Photos-shaped metadata.
// The fileid invariant the spec calls out lives in the
// owner's existing filecache row at `(alice_id, owner_path)`,
// which is unaffected here.
let meta = child.storage.stat(&StoragePath::root()).await?;
```

Replace with:
```rust
// Stat through the filecache with the share-mount wrapper
// translated to (owner_storage, owner_path) — keeps cache rows
// in the owner's namespace instead of poisoning. See
// `cache_key_for` and the spec at
// `docs/superpowers/specs/2026-05-14-filecache-share-mount-translation-design.md`.
let (child_cache_storage, child_cache_path) =
    cache_key_for(&child.storage, &StoragePath::root())?;
let meta = self
    .filecache
    .stat(&child_cache_storage, &child_cache_path)
    .await?;
```

- [ ] **Step 4: Build + run existing tests**

```bash
cargo build -p crabcloud-fs
cargo test -p crabcloud-fs --lib
cargo test -p crabcloud-fs --test view_reads
```
Expected: all pass. The existing tests verify user-visible behavior, which is unchanged.

- [ ] **Step 5: Commit**

```bash
git add crates/crabcloud-fs/src/view.rs
git commit -m "fs: translate share-mount paths before Filecache lookup in View

Adds a `cache_key_for` helper that consults `Storage::inner_storage()`
and translates `(wrapper, recipient_path)` to `(inner, owner_path.join(
recipient_path))` for cache key derivation. `View::stat` and
`View::list_with_meta` use it. The synthetic-entry stat for share-mount
children — which previously bypassed the cache to avoid poisoning
alice's rows — now goes through the cache with the translated key.

Spec: docs/superpowers/specs/2026-05-14-filecache-share-mount-translation-design.md"
```

---

## Task 4: Regression test — descend into share doesn't poison owner cache

**Files:**
- Modify: `crates/crabcloud-fs/tests/view_reads.rs` (append new tests)

- [ ] **Step 1: Inspect the existing test harness**

```bash
head -25 crates/crabcloud-fs/tests/view_reads.rs
grep -n "fn view_with_share_mount\|fn view_home\|fn harness" crates/crabcloud-fs/tests/view_reads.rs crates/crabcloud-fs/tests/support/mod.rs
```

Confirm the helpers `harness`, `view_home`, `view_with_share_mount` are available (they were added during SP7). They construct a test harness with a shared `FileCache` + `EventSink`. We'll use them.

- [ ] **Step 2: Add `view_descend_into_share_does_not_poison_owner_cache`**

Append at the end of `crates/crabcloud-fs/tests/view_reads.rs`:

```rust
#[tokio::test]
async fn view_descend_into_share_does_not_poison_owner_cache() {
    // After bob descends into a share, alice's cache row at her actual
    // home root must NOT be poisoned with the share's metadata. Before
    // the cache-key-translation fix, bob's `view.list("/Photos")` was
    // populating `(alice_id, root)` with `/Photos`-shaped metadata,
    // which then leaked into alice's own `view.stat("/")` calls.
    let h = harness().await;

    // Alice's home: /Photos/sunset.jpg AND /notes.txt (a sibling so we
    // have two distinct paths to disambiguate cache rows).
    let alice_home: Arc<dyn Storage> = Arc::new(MemoryStorage::new("alice"));
    alice_home
        .mkdir(&StoragePath::new("Photos").unwrap(), &NoopEventSink)
        .await
        .unwrap();
    alice_home
        .put_file(
            &StoragePath::new("Photos/sunset.jpg").unwrap(),
            body(b"jpeg".to_vec()),
            &NoopEventSink,
        )
        .await
        .unwrap();
    alice_home
        .put_file(
            &StoragePath::new("notes.txt").unwrap(),
            body(b"buy milk".to_vec()),
            &NoopEventSink,
        )
        .await
        .unwrap();

    // Bob's view: empty home + share mount at /Photos.
    let bob_home: Arc<dyn Storage> = Arc::new(MemoryStorage::new("bob"));
    let bob_view = view_with_share_mount(
        &h,
        bob_home,
        alice_home.clone(),
        "Photos",
        "Photos",
    );

    // Bob descends into /Photos. This is the call that used to poison.
    let entries = bob_view
        .list(&UserPath::new("/Photos").unwrap())
        .await
        .unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(
        names.contains(&"sunset.jpg"),
        "bob should see alice's /Photos children: got {names:?}"
    );

    // Alice (sharing the same harness, so the same FileCache) stats her
    // own /notes.txt. The metadata must be /notes.txt's, NOT a poisoned
    // value that bob's descend cached.
    let alice_view = view_home_for(&h, alice_home.clone());
    let notes_meta = alice_view
        .stat(&UserPath::new("/notes.txt").unwrap())
        .await
        .unwrap();
    assert_eq!(notes_meta.size, b"buy milk".len() as u64);
    assert_eq!(notes_meta.kind, FileKind::File);
}
```

The test uses a `view_home_for(&h, alice_storage)` helper that builds a View for an arbitrary owner. If that helper doesn't exist in `tests/support/mod.rs`, add it (see Step 3 below).

- [ ] **Step 3: Ensure `view_home_for` helper exists in the test support**

Open `crates/crabcloud-fs/tests/support/mod.rs`. Find `view_home` (the existing helper used by other tests). If it builds a View with the harness's storage at the root mount, generalize / supplement it:

```rust
/// Build a View for any user with the given storage at root. Used when
/// tests need to act as a non-default user (e.g., alice while bob has a
/// share mount on the same harness).
pub fn view_home_for(h: &Harness, storage: Arc<dyn Storage>) -> View {
    View::new(
        UserId::new("alice").unwrap(),
        vec![Mount {
            path_prefix: StoragePath::root(),
            storage,
            metadata: None,
        }],
        h.filecache.clone(),
        h.sink.clone(),
    )
}
```

If `view_home` already does something close (e.g., takes a `&Harness` and uses `h.storage` directly), prefer to keep it and add `view_home_for` as a sibling. Don't refactor `view_home` itself — other tests rely on its specific shape.

- [ ] **Step 4: Run the new test**

```bash
cargo test -p crabcloud-fs --test view_reads view_descend_into_share_does_not_poison_owner_cache
```
Expected: PASS.

Sanity-check that the test would have failed before the Task 3 fix: temporarily revert `cache_key_for` in `view.rs::stat` (replace the body with the pre-Task-3 version), re-run the test, confirm it FAILS, then restore the fix. (Optional but recommended; document the result in the PR description.)

- [ ] **Step 5: Add `view_share_mount_preserves_file_id_continuity`**

Append in `view_reads.rs`:

```rust
#[tokio::test]
async fn view_share_mount_preserves_file_id_continuity() {
    // SP7 §3.2: a file accessed through a share mount must have the
    // SAME identity as the owner's direct access — the recipient's
    // sync client should see the same fileid as the owner's. We pin
    // this via the etag: both views, going through the filecache,
    // must read the same cache row for the same underlying file.
    let h = harness().await;
    let alice_home: Arc<dyn Storage> = Arc::new(MemoryStorage::new("alice"));
    alice_home
        .mkdir(&StoragePath::new("Photos").unwrap(), &NoopEventSink)
        .await
        .unwrap();
    alice_home
        .put_file(
            &StoragePath::new("Photos/sunset.jpg").unwrap(),
            body(b"jpeg-bytes".to_vec()),
            &NoopEventSink,
        )
        .await
        .unwrap();

    let alice_view = view_home_for(&h, alice_home.clone());
    let alice_meta = alice_view
        .stat(&UserPath::new("/Photos/sunset.jpg").unwrap())
        .await
        .unwrap();

    let bob_home: Arc<dyn Storage> = Arc::new(MemoryStorage::new("bob"));
    let bob_view = view_with_share_mount(
        &h,
        bob_home,
        alice_home,
        "Photos",
        "Photos",
    );
    let bob_meta = bob_view
        .stat(&UserPath::new("/Photos/sunset.jpg").unwrap())
        .await
        .unwrap();

    assert_eq!(
        alice_meta.etag.as_str(),
        bob_meta.etag.as_str(),
        "alice and bob must see the same etag for the same file (cache row)"
    );
    assert_eq!(alice_meta.size, bob_meta.size);
}
```

- [ ] **Step 6: Run the new test**

```bash
cargo test -p crabcloud-fs --test view_reads view_share_mount_preserves_file_id_continuity
```
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/crabcloud-fs/tests/view_reads.rs crates/crabcloud-fs/tests/support/mod.rs
git commit -m "fs(tests): pin no-poisoning + file-id continuity for share mounts"
```

---

## Task 5: Pre-PR sweep + PR

- [ ] **Step 1: Run the full local test matrix**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
All three must pass.

If Docker is available locally and you want to be thorough, also:
```bash
cargo test -p crabcloud-sharing -- --include-ignored --test-threads=1
cargo test -p crabcloud-db --test migrate_end_to_end -- --include-ignored
```
These exercise the multidialect paths that touched the share-mount + filecache surface during SP7. (Optional; CI runs them.)

- [ ] **Step 2: Push the branch**

```bash
git push -u origin fix/filecache-share-translation
```

- [ ] **Step 3: Open the PR**

```bash
gh pr create --title "fix(fs): translate share-mount paths before Filecache lookup" --body "$(cat <<'EOF'
SP7 carryforward #2. Last carryforward before SP8.

## What

\`SharedSubrootStorage\` exposes \`inner.id()\` so file_ids stay stable across recipients (SP7 §3.2). But every \`Filecache::stat/list\` call against the wrapper used to key cache rows by \`(wrapper.id() = inner.id(), recipient_relative_path)\` — the recipient-relative paths collided with the owner's actual paths in her home. Bob's \`view.list("/Photos")\` populated \`(alice_id, /)\` with \`/Photos\`-shaped metadata, poisoning alice's root row.

## How

- New trait method \`Storage::inner_storage() -> Option<(&Arc<dyn Storage>, &StoragePath)>\` with a \`None\` default. \`SharedSubrootStorage\` overrides it to return \`Some((&self.inner, &self.owner_path))\`.
- New \`cache_key_for\` helper in \`crabcloud-fs::view\` translates \`(wrapper, recipient_path)\` to \`(inner, owner_path.join(recipient_path))\` before calling \`Filecache\`.
- \`View::stat\`, \`View::list_with_meta\` (both call sites) consult the helper before \`Filecache\`.
- The synthetic-entry stat for share-mount children — which SP7 Batch E added as a workaround that bypassed the cache to avoid poisoning — switches to the new translated-cache call. Removes ~20 lines of workaround comments.

Writes are unaffected: \`wrapper.put_file/mkdir/delete/...\` already delegate to \`inner\` which emits StorageEvents from \`inner\`'s id, so scanner cache populations were already correct.

## Tests

- \`view_descend_into_share_does_not_poison_owner_cache\` — regression guard. Bob descends into a share; alice's home row stays correct.
- \`view_share_mount_preserves_file_id_continuity\` — alice and bob see the same etag (same cache row) for the same file through a share mount.

Existing user-visible behavior unchanged: \`view_list_inside_share_mount_returns_owners_children\`, \`view_list_share_mount_entry_carries_owners_metadata\`, the \`list_dir_decorates_shared_by_for_recipient_and_share_count_for_owner\` server-fn test, and all sharing-e2e tests on sqlite/mysql/postgres continue to pass.

## Test plan

- [ ] CI green (fmt-and-clippy, test-sqlite, test-multidialect, e2e).

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 4: Wait for CI; merge when green**

```bash
gh pr merge --squash --delete-branch
```

---

## Closing checklist

After merge:

- [ ] Spec §7 acceptance criteria all green on master:
  - `cargo fmt --all --check` clean.
  - `cargo clippy --workspace --all-targets -- -D warnings` clean.
  - `cargo test --workspace` clean.
  - Reverting the fix temporarily makes `view_descend_into_share_does_not_poison_owner_cache` fail (regression-guard property).
  - The SP7 Batch E synthetic-entry stat bypass is gone from `view.rs`.
- [ ] Last SP7 carryforward closed. Ready for SP8 (public links).
