//! `ShareMountResolver` — composes `HomeMountResolver` with the sharing
//! service to surface one extra mount per accepted incoming share.
//!
//! For each row returned by `Shares::list_incoming(uid)`:
//! 1. Resolve the owner's home storage via the shared `StorageFactory`.
//! 2. Translate `row.item_source` (fileid) into the owner's current
//!    `StoragePath` via the filecache. If the row's gone the mount is
//!    skipped + warned (spec §10 #6 — survives owner-side renames; ignores
//!    rows whose target was deleted out from under the share).
//! 3. Choose a mount name from `basename_of(row.file_target)`, suffixing
//!    `(2)`, `(3)`, … when it collides with an entry the recipient already
//!    has at their home root (or with a previously-resolved share mount).
//! 4. Wrap the owner's home storage in a `SharedSubrootStorage` pinned at
//!    the resolved `owner_path` with the share's recipient permissions.
//!
//! Testability: the dependency on `Shares` + `FileCache` is taken through
//! the local `SharesLookup` / `FileCacheLookup` traits so unit tests can
//! plug in in-memory fakes without bringing up a real DB pool. Blanket
//! impls below adapt the real types to the trait surface.

use async_trait::async_trait;
use crabcloud_filecache::{FileCache, FileCacheError};
use crabcloud_sharing::{ShareError, ShareRow, Shares};
use crabcloud_storage::StoragePath;
use crabcloud_users::UserId;
use std::collections::HashSet;
use std::sync::Arc;

use crate::error::{FsError, FsResult};
use crate::mount::{Mount, MountKind, MountMetadata, MountResolver, StorageFactory};
use crate::resolver::HomeMountResolver;
use crate::storage::SharedSubrootStorage;

/// Subset of `Shares` the resolver needs. Decoupled into its own trait so
/// tests can substitute an in-memory faker without standing up a DB pool.
#[async_trait]
pub trait SharesLookup: Send + Sync {
    async fn list_incoming(&self, recipient: &UserId) -> Result<Vec<ShareRow>, ShareError>;
}

#[async_trait]
impl SharesLookup for Shares {
    async fn list_incoming(&self, recipient: &UserId) -> Result<Vec<ShareRow>, ShareError> {
        Shares::list_incoming(self, recipient).await
    }
}

/// Subset of `FileCache` the resolver needs: map a fileid (under a known
/// storage id) to its current `StoragePath`. Returning `Ok(None)` signals
/// "the row was removed from the cache" — spec §10 #6 says we drop the
/// share mount rather than fail the whole login.
#[async_trait]
pub trait FileCacheLookup: Send + Sync {
    async fn path_for_fileid(
        &self,
        storage_id: &str,
        fileid: i64,
    ) -> Result<Option<StoragePath>, FileCacheError>;
}

#[async_trait]
impl FileCacheLookup for FileCache {
    async fn path_for_fileid(
        &self,
        storage_id: &str,
        fileid: i64,
    ) -> Result<Option<StoragePath>, FileCacheError> {
        match self.lookup_by_id(fileid).await? {
            Some(row) if row.storage_id == storage_id => Ok(Some(row.path)),
            // Either the row is gone or it lives under a different storage —
            // treat both as "not present in the owner's namespace".
            _ => Ok(None),
        }
    }
}

pub struct ShareMountResolver {
    home: HomeMountResolver,
    shares: Arc<dyn SharesLookup>,
    storage_factory: Arc<dyn StorageFactory>,
    filecache: Arc<dyn FileCacheLookup>,
}

impl ShareMountResolver {
    pub fn new(
        home: HomeMountResolver,
        shares: Arc<dyn SharesLookup>,
        storage_factory: Arc<dyn StorageFactory>,
        filecache: Arc<dyn FileCacheLookup>,
    ) -> Self {
        Self {
            home,
            shares,
            storage_factory,
            filecache,
        }
    }
}

#[async_trait]
impl MountResolver for ShareMountResolver {
    async fn mounts_for(&self, uid: &UserId) -> FsResult<Vec<Mount>> {
        let mut mounts = self.home.mounts_for(uid).await?;
        // `HomeMountResolver` always returns at least the home mount at the
        // root. Guard anyway — the bound prevents a panic if a future
        // resolver swap returns an empty list.
        let home_mount = mounts.first().ok_or(FsError::MountNotFound)?;
        let mut used_names = home_top_level_names(home_mount).await?;

        let incoming = self
            .shares
            .list_incoming(uid)
            .await
            .map_err(share_err_to_fs)?;

        for row in incoming {
            let owner_id = match UserId::new(row.uid_owner.clone()) {
                Ok(u) => u,
                Err(e) => {
                    tracing::warn!(share_id = row.id, error = %e, "share owner uid invalid; skipping mount");
                    continue;
                }
            };
            let owner_home = self.storage_factory.home_storage(&owner_id).await?;
            let owner_path = match self
                .filecache
                .path_for_fileid(owner_home.id(), row.item_source)
                .await?
            {
                Some(p) => p,
                None => {
                    tracing::warn!(
                        share_id = row.id,
                        fileid = row.item_source,
                        owner_uid = %row.uid_owner,
                        recipient_uid = %uid.as_str(),
                        "share source not in filecache; skipping mount"
                    );
                    continue;
                }
            };

            let display_basename = basename_of(&row.file_target);
            let mount_name = unique_name(display_basename, &mut used_names);
            let prefix = StoragePath::new(mount_name.clone())
                .map_err(|_| FsError::InvalidPath(mount_name.clone()))?;
            mounts.push(Mount {
                path_prefix: prefix,
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

async fn home_top_level_names(home: &Mount) -> FsResult<HashSet<String>> {
    let entries = home.storage.list(&StoragePath::root()).await?;
    Ok(entries.into_iter().map(|e| e.name).collect())
}

fn basename_of(file_target: &str) -> String {
    let trimmed = file_target.trim_matches('/');
    if trimmed.is_empty() {
        return file_target.to_string();
    }
    match trimmed.rsplit_once('/') {
        Some((_, last)) => last.to_string(),
        None => trimmed.to_string(),
    }
}

fn unique_name(desired: String, used: &mut HashSet<String>) -> String {
    if !used.contains(&desired) {
        used.insert(desired.clone());
        return desired;
    }
    let mut n = 2_u32;
    loop {
        let candidate = format!("{desired} ({n})");
        if !used.contains(&candidate) {
            used.insert(candidate.clone());
            return candidate;
        }
        n += 1;
    }
}

fn share_err_to_fs(err: ShareError) -> FsError {
    // The sharing service surfaces its own errors. None of them are a
    // crabcloud-fs error variant; lift through the upload-string variant
    // so callers see a human-readable message without us inventing a new
    // `FsError::Share`. The resolver is read-only from the caller's
    // perspective so the imprecision is acceptable.
    FsError::Upload(format!("sharing: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crabcloud_sharing::{ItemType, SharePermissions, ShareType};
    use crabcloud_storage::{memory::MemoryStorage, NoopEventSink, Storage};
    use std::sync::Mutex;

    struct FakeShares {
        rows: Mutex<Vec<ShareRow>>,
    }

    impl FakeShares {
        fn new(rows: Vec<ShareRow>) -> Self {
            Self {
                rows: Mutex::new(rows),
            }
        }
    }

    #[async_trait]
    impl SharesLookup for FakeShares {
        async fn list_incoming(&self, _: &UserId) -> Result<Vec<ShareRow>, ShareError> {
            Ok(self.rows.lock().unwrap().clone())
        }
    }

    // (storage_id, fileid) -> Option<StoragePath>. Missing entry = None.
    type FilecacheEntry = ((String, i64), Option<StoragePath>);

    struct FakeFilecache {
        entries: Mutex<Vec<FilecacheEntry>>,
    }

    impl FakeFilecache {
        fn new(entries: Vec<FilecacheEntry>) -> Self {
            Self {
                entries: Mutex::new(entries),
            }
        }
    }

    #[async_trait]
    impl FileCacheLookup for FakeFilecache {
        async fn path_for_fileid(
            &self,
            storage_id: &str,
            fileid: i64,
        ) -> Result<Option<StoragePath>, FileCacheError> {
            for ((sid, fid), p) in self.entries.lock().unwrap().iter() {
                if sid == storage_id && *fid == fileid {
                    return Ok(p.clone());
                }
            }
            Ok(None)
        }
    }

    struct MemoryFactory {
        // Hand each requested uid the same storage handle so test setup can
        // pre-seed the "owner's home" via the returned Arc.
        per_user: Mutex<Vec<(String, Arc<dyn Storage>)>>,
    }

    impl MemoryFactory {
        fn new() -> Self {
            Self {
                per_user: Mutex::new(Vec::new()),
            }
        }
        fn install(&self, uid: &str, s: Arc<dyn Storage>) {
            self.per_user.lock().unwrap().push((uid.to_string(), s));
        }
    }

    #[async_trait]
    impl StorageFactory for MemoryFactory {
        async fn home_storage(&self, uid: &UserId) -> FsResult<Arc<dyn Storage>> {
            for (k, v) in self.per_user.lock().unwrap().iter() {
                if k == uid.as_str() {
                    return Ok(v.clone());
                }
            }
            Ok(Arc::new(MemoryStorage::new(uid.as_str())))
        }
    }

    fn share_row(
        id: i64,
        owner: &str,
        recipient: &str,
        file_target: &str,
        item_source: i64,
    ) -> ShareRow {
        ShareRow {
            id,
            share_type: ShareType::User,
            share_with: Some(recipient.to_string()),
            uid_owner: owner.to_string(),
            uid_initiator: owner.to_string(),
            parent: None,
            item_type: ItemType::Folder,
            item_source,
            file_source: item_source,
            file_target: file_target.to_string(),
            permissions: SharePermissions::from_wire(1 | 2),
            stime: 0,
            accepted: true,
            expiration: None,
            token: None,
            password_hash: None,
            last_warned: None,
        }
    }

    async fn seeded_alice_home() -> Arc<dyn Storage> {
        let s: Arc<dyn Storage> = Arc::new(MemoryStorage::new("alice"));
        s.mkdir(&StoragePath::new("Photos").unwrap(), &NoopEventSink)
            .await
            .unwrap();
        s.mkdir(&StoragePath::new("Vacation").unwrap(), &NoopEventSink)
            .await
            .unwrap();
        s
    }

    async fn seeded_bob_home_with_photos() -> Arc<dyn Storage> {
        let s: Arc<dyn Storage> = Arc::new(MemoryStorage::new("bob"));
        s.mkdir(&StoragePath::new("Photos").unwrap(), &NoopEventSink)
            .await
            .unwrap();
        s
    }

    #[test]
    fn basename_extracts_last_segment() {
        assert_eq!(basename_of("/Photos"), "Photos");
        assert_eq!(basename_of("/a/b/c.txt"), "c.txt");
        assert_eq!(basename_of("Photos"), "Photos");
        assert_eq!(basename_of("/"), "/");
    }

    #[test]
    fn unique_name_suffixes_on_collision() {
        let mut used: HashSet<String> = ["Photos".into()].into_iter().collect();
        assert_eq!(unique_name("Photos".into(), &mut used), "Photos (2)");
        assert_eq!(unique_name("Photos".into(), &mut used), "Photos (3)");
        assert_eq!(unique_name("Other".into(), &mut used), "Other");
        assert!(used.contains("Photos (2)"));
        assert!(used.contains("Photos (3)"));
        assert!(used.contains("Other"));
    }

    #[tokio::test]
    async fn share_collides_with_home_entry_gets_suffix() {
        let factory = Arc::new(MemoryFactory::new());
        let alice_home = seeded_alice_home().await;
        factory.install("alice", alice_home.clone());
        factory.install("bob", seeded_bob_home_with_photos().await);

        let shares = Arc::new(FakeShares::new(vec![share_row(
            1, "alice", "bob", "/Photos", 100,
        )]));
        let filecache = Arc::new(FakeFilecache::new(vec![(
            (alice_home.id().to_string(), 100),
            Some(StoragePath::new("Photos").unwrap()),
        )]));

        let resolver = ShareMountResolver::new(
            HomeMountResolver::new(factory.clone()),
            shares,
            factory,
            filecache,
        );
        let bob = UserId::new("bob").unwrap();
        let mounts = resolver.mounts_for(&bob).await.unwrap();
        assert_eq!(mounts.len(), 2);
        assert!(mounts[0].path_prefix.is_root());
        assert_eq!(mounts[1].path_prefix.as_str(), "Photos (2)");
        let md = mounts[1].metadata.as_ref().unwrap();
        assert_eq!(md.kind, MountKind::Share);
        assert_eq!(md.owner_uid.as_deref(), Some("alice"));
    }

    #[tokio::test]
    async fn two_incoming_shares_with_same_basename_cascade_suffixes() {
        let factory = Arc::new(MemoryFactory::new());
        let alice_home = seeded_alice_home().await;
        factory.install("alice", alice_home.clone());
        // bob's home has no `Photos`, so the first share takes the bare name.
        factory.install("bob", Arc::new(MemoryStorage::new("bob")));

        let shares = Arc::new(FakeShares::new(vec![
            share_row(1, "alice", "bob", "/Photos", 100),
            share_row(2, "alice", "bob", "/Photos", 200),
        ]));
        let filecache = Arc::new(FakeFilecache::new(vec![
            (
                (alice_home.id().to_string(), 100),
                Some(StoragePath::new("Photos").unwrap()),
            ),
            (
                (alice_home.id().to_string(), 200),
                Some(StoragePath::new("Vacation").unwrap()),
            ),
        ]));

        let resolver = ShareMountResolver::new(
            HomeMountResolver::new(factory.clone()),
            shares,
            factory,
            filecache,
        );
        let bob = UserId::new("bob").unwrap();
        let mounts = resolver.mounts_for(&bob).await.unwrap();
        assert_eq!(mounts.len(), 3);
        assert_eq!(mounts[1].path_prefix.as_str(), "Photos");
        assert_eq!(mounts[2].path_prefix.as_str(), "Photos (2)");
    }

    #[tokio::test]
    async fn all_shares_missing_filecache_rows_returns_home_only() {
        // Spec §10 carry-forward #6: a `ShareRow` whose `item_source`
        // does NOT correspond to any filecache entry must be dropped
        // silently (with a warn) rather than erroring the whole login.
        // Distinct from `missing_filecache_row_skips_mount` below: that
        // case mixes one present and one absent row; this one exercises
        // the all-absent path so the resulting list collapses to just
        // the home mount.
        let factory = Arc::new(MemoryFactory::new());
        let alice_home = seeded_alice_home().await;
        factory.install("alice", alice_home.clone());
        factory.install("bob", Arc::new(MemoryStorage::new("bob")));

        let shares = Arc::new(FakeShares::new(vec![share_row(
            1, "alice", "bob", "/Photos", 100,
        )]));
        // Faker returns None for every (storage_id, fileid) lookup — the
        // share row stands but its source can't be located.
        let filecache = Arc::new(FakeFilecache::new(vec![]));

        let resolver = ShareMountResolver::new(
            HomeMountResolver::new(factory.clone()),
            shares,
            factory,
            filecache,
        );
        let bob = UserId::new("bob").unwrap();
        let mounts = resolver.mounts_for(&bob).await.unwrap();
        assert_eq!(
            mounts.len(),
            1,
            "share with missing filecache row must be skipped"
        );
        assert!(mounts[0].path_prefix.is_root());
        assert!(mounts[0].metadata.is_none());
    }

    #[tokio::test]
    async fn missing_filecache_row_skips_mount() {
        let factory = Arc::new(MemoryFactory::new());
        let alice_home = seeded_alice_home().await;
        factory.install("alice", alice_home.clone());
        factory.install("bob", Arc::new(MemoryStorage::new("bob")));

        let shares = Arc::new(FakeShares::new(vec![
            share_row(1, "alice", "bob", "/Photos", 100),
            share_row(2, "alice", "bob", "/Vacation", 200),
        ]));
        // Only share 2 has a filecache entry; share 1 has no row → skipped.
        let filecache = Arc::new(FakeFilecache::new(vec![(
            (alice_home.id().to_string(), 200),
            Some(StoragePath::new("Vacation").unwrap()),
        )]));

        let resolver = ShareMountResolver::new(
            HomeMountResolver::new(factory.clone()),
            shares,
            factory,
            filecache,
        );
        let bob = UserId::new("bob").unwrap();
        let mounts = resolver.mounts_for(&bob).await.unwrap();
        assert_eq!(mounts.len(), 2);
        assert_eq!(mounts[1].path_prefix.as_str(), "Vacation");
    }

    #[tokio::test]
    async fn zero_incoming_shares_returns_home_only() {
        let factory = Arc::new(MemoryFactory::new());
        factory.install("bob", Arc::new(MemoryStorage::new("bob")));

        let shares = Arc::new(FakeShares::new(vec![]));
        let filecache = Arc::new(FakeFilecache::new(vec![]));

        let resolver = ShareMountResolver::new(
            HomeMountResolver::new(factory.clone()),
            shares,
            factory,
            filecache,
        );
        let bob = UserId::new("bob").unwrap();
        let mounts = resolver.mounts_for(&bob).await.unwrap();
        assert_eq!(mounts.len(), 1);
        assert!(mounts[0].path_prefix.is_root());
        assert!(mounts[0].metadata.is_none());
    }
}
