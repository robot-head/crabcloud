//! `View` — per-user filesystem façade. Resolves user paths to
//! `(Mount, StoragePath)` via longest-prefix match; reads route through
//! the `FileCache`; writes go to storage with events emitted via the
//! shared `ChannelEventSink`.

use crate::error::{FsError, FsResult};
use crate::mount::{Mount, MountMetadata};
use crate::path::UserPath;
use crabcloud_filecache::FileCache;
use crabcloud_storage::{ChannelEventSink, DirEntry, FileMetadata, Storage, StoragePath};
use crabcloud_users::UserId;
use std::ops::Range;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::AsyncRead;

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

/// One entry returned by [`View::list_with_meta`]. Pairs the raw
/// [`DirEntry`] with the [`MountMetadata`] of the mount the entry was
/// surfaced from when that entry is a share-mount root (so the caller
/// can decorate the row with `shared_by` etc.). `None` for entries
/// served from the longest-prefix mount itself (i.e. ordinary children
/// of the listed directory) — they live under the same mount as the
/// parent so there is no per-entry mount metadata distinct from the
/// resolver's view of the world.
#[derive(Debug, Clone)]
pub struct ListedEntry {
    pub entry: DirEntry,
    pub mount_metadata: Option<MountMetadata>,
}

pub struct View {
    pub(crate) uid: UserId,
    pub(crate) mounts: Vec<Mount>,
    pub(crate) filecache: Arc<FileCache>,
    pub(crate) storage_sink: Arc<ChannelEventSink>,
}

impl View {
    pub fn new(
        uid: UserId,
        mounts: Vec<Mount>,
        filecache: Arc<FileCache>,
        storage_sink: Arc<ChannelEventSink>,
    ) -> Self {
        Self {
            uid,
            mounts,
            filecache,
            storage_sink,
        }
    }

    pub fn uid(&self) -> &UserId {
        &self.uid
    }

    pub fn mounts(&self) -> &[Mount] {
        &self.mounts
    }

    /// Resolve a user-facing path to the responsible mount + the storage-
    /// relative path under that mount.
    ///
    /// Longest-prefix match against `self.mounts`. Strips the mount's
    /// `path_prefix` to produce the storage-relative `StoragePath`. Errors
    /// `MountNotFound` if no mount matches (shouldn't happen with a home
    /// mount anchored at `/`).
    pub(crate) fn resolve(&self, user_path: &UserPath) -> FsResult<(&Mount, StoragePath)> {
        // Strip leading `/` — `UserPath` guarantees one.
        let trimmed = user_path.as_str().trim_start_matches('/');
        let best = self
            .mounts
            .iter()
            .filter(|m| {
                let prefix = m.path_prefix.as_str();
                prefix.is_empty() || trimmed == prefix || trimmed.starts_with(&format!("{prefix}/"))
            })
            .max_by_key(|m| m.path_prefix.as_str().len())
            .ok_or(FsError::MountNotFound)?;
        let suffix = if best.path_prefix.is_root() {
            trimmed.to_string()
        } else {
            let with_slash = format!("{}/", best.path_prefix.as_str());
            trimmed
                .strip_prefix(&with_slash)
                .map(String::from)
                .unwrap_or_default()
        };
        let storage_path = StoragePath::new(suffix)?;
        Ok((best, storage_path))
    }

    /// Cached stat. Routes through `FileCache::stat` which populates on
    /// miss via the backing storage.
    pub async fn stat(&self, user_path: &UserPath) -> FsResult<FileMetadata> {
        let (mount, storage_path) = self.resolve(user_path)?;
        let (cache_storage, cache_path) = cache_key_for(&mount.storage, &storage_path)?;
        let meta = self.filecache.stat(&cache_storage, &cache_path).await?;
        Ok(meta)
    }

    /// Cached directory listing. Returns just the [`DirEntry`]s; share-
    /// mount children at the listed level ARE included (so PROPFIND on
    /// bob's root sees the share folder), but their mount metadata is
    /// dropped — callers that need `shared_by` / `share_count` should
    /// reach for [`Self::list_with_meta`] instead.
    pub async fn list(&self, user_path: &UserPath) -> FsResult<Vec<DirEntry>> {
        Ok(self
            .list_with_meta(user_path)
            .await?
            .into_iter()
            .map(|le| le.entry)
            .collect())
    }

    /// Like [`Self::list`] but also surfaces, for each share-mount whose
    /// `path_prefix` lives one level below `user_path`, the share's
    /// [`MountMetadata`] alongside its synthetic [`DirEntry`]. The
    /// synthetic entry's size / mtime / fileid come from `stat`-ing the
    /// share-mount's storage at its root — which routes through the
    /// [`SharedSubrootStorage`] wrapper to the OWNER's filecache row, so
    /// the file_id stays stable across recipients (spec §3.2).
    pub async fn list_with_meta(&self, user_path: &UserPath) -> FsResult<Vec<ListedEntry>> {
        let (mount, storage_path) = self.resolve(user_path)?;
        // Storage-path-formatted version of the listed directory, used to
        // match child-mount candidates by `path_prefix.parent()`. The
        // longest-prefix resolver guarantees `storage_path` is RELATIVE to
        // `mount.path_prefix`, so to compare against other mounts' absolute
        // prefixes we re-prepend it here.
        let listed_abs = if mount.path_prefix.is_root() {
            storage_path.clone()
        } else if storage_path.is_root() {
            mount.path_prefix.clone()
        } else {
            mount.path_prefix.join(storage_path.as_str())?
        };

        // For the storage root we tolerate `NotFound` — some backends
        // (e.g. `MemoryStorage`) don't materialize a root entry until
        // something is written into them. The cache-backed `list` calls
        // `stat` first, which fails on those backends; we fall back to
        // `storage.list(root)` directly so the listing still surfaces
        // children (plus any synthetic share-mount entries below). Non-
        // root paths route through the cache unconditionally.
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
        let resolved_prefix = mount.path_prefix.clone();
        let mut out: Vec<ListedEntry> = base_entries
            .into_iter()
            .map(|e| ListedEntry {
                entry: e,
                mount_metadata: None,
            })
            .collect();

        // Surface share-mount children one level below the listed path.
        // Skip the mount whose prefix IS `listed_abs` itself (that's the
        // resolved mount; we'd otherwise list it as its own entry).
        for child in &self.mounts {
            if child.path_prefix.is_root() {
                continue;
            }
            if child.path_prefix == resolved_prefix {
                continue;
            }
            let Some(parent) = child.path_prefix.parent() else {
                continue;
            };
            if parent != listed_abs {
                continue;
            }
            // Stat through the filecache with the share-mount wrapper translated to
            // (owner_storage, owner_path) — keeps cache rows in the owner's
            // namespace. See `cache_key_for` and the spec at
            // `docs/superpowers/specs/2026-05-14-filecache-share-mount-translation-design.md`.
            let (child_cache_storage, child_cache_path) =
                cache_key_for(&child.storage, &StoragePath::root())?;
            let meta = self
                .filecache
                .stat(&child_cache_storage, &child_cache_path)
                .await?;
            // The synthetic entry's display name is the LAST segment of
            // the mount's `path_prefix`. That's how the recipient sees
            // it; the owner's source basename (which may differ after a
            // rename) is not exposed at this layer.
            let name = child.path_prefix.basename().to_string();
            out.push(ListedEntry {
                entry: DirEntry {
                    name,
                    metadata: meta,
                },
                mount_metadata: child.metadata.clone(),
            });
        }

        Ok(out)
    }

    pub async fn read(&self, user_path: &UserPath) -> FsResult<Pin<Box<dyn AsyncRead + Send>>> {
        let (mount, storage_path) = self.resolve(user_path)?;
        let r = mount.storage.read(&storage_path).await?;
        Ok(r)
    }

    pub async fn read_range(
        &self,
        user_path: &UserPath,
        range: Range<u64>,
    ) -> FsResult<Pin<Box<dyn AsyncRead + Send>>> {
        let (mount, storage_path) = self.resolve(user_path)?;
        let r = mount.storage.read_range(&storage_path, range).await?;
        Ok(r)
    }

    /// Write through the storage backend. The storage emits a `Written`
    /// event on `storage_sink`; the scanner asynchronously updates the
    /// filecache. The caller gets the storage's fresh `FileMetadata`
    /// directly — no need to wait for the scanner to catch up.
    pub async fn put_file(
        &self,
        user_path: &UserPath,
        body: Pin<Box<dyn AsyncRead + Send>>,
    ) -> FsResult<FileMetadata> {
        let (mount, storage_path) = self.resolve(user_path)?;
        let meta = mount
            .storage
            .put_file(&storage_path, body, &*self.storage_sink)
            .await?;
        Ok(meta)
    }

    pub async fn mkdir(&self, user_path: &UserPath) -> FsResult<FileMetadata> {
        let (mount, storage_path) = self.resolve(user_path)?;
        let meta = mount
            .storage
            .mkdir(&storage_path, &*self.storage_sink)
            .await?;
        Ok(meta)
    }

    pub async fn delete(&self, user_path: &UserPath) -> FsResult<()> {
        let (mount, storage_path) = self.resolve(user_path)?;
        mount
            .storage
            .delete(&storage_path, &*self.storage_sink)
            .await?;
        Ok(())
    }

    /// Within-mount rename. Errors `FsError::CrossMount` if `from` and
    /// `to` resolve to different mounts (4c only ships one mount per
    /// user; this can't fire in practice but the wire shape is set).
    pub async fn rename(&self, from: &UserPath, to: &UserPath) -> FsResult<()> {
        let (from_mount, from_path) = self.resolve(from)?;
        let (to_mount, to_path) = self.resolve(to)?;
        if from_mount.path_prefix != to_mount.path_prefix {
            return Err(FsError::CrossMount);
        }
        from_mount
            .storage
            .rename(&from_path, &to_path, &*self.storage_sink)
            .await?;
        Ok(())
    }

    /// Within-mount copy. Same cross-mount restriction.
    pub async fn copy(&self, from: &UserPath, to: &UserPath) -> FsResult<()> {
        let (from_mount, from_path) = self.resolve(from)?;
        let (to_mount, to_path) = self.resolve(to)?;
        if from_mount.path_prefix != to_mount.path_prefix {
            return Err(FsError::CrossMount);
        }
        from_mount
            .storage
            .copy(&from_path, &to_path, &*self.storage_sink)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crabcloud_storage::{memory::MemoryStorage, Storage};

    fn build_view_with_mounts(mounts: Vec<Mount>) -> View {
        // Construct a minimal View for resolve()-only unit tests. The
        // filecache + storage_sink are unused on the resolve path; we
        // use a Storage-less stub that satisfies the type but never
        // sees a method call.
        //
        // For unit-testing resolve only, we build with dummy fields the
        // compiler accepts. Integration tests in `tests/view_reads.rs`
        // exercise real stat/list/etc.
        use crabcloud_cache::MemoryCache;
        use crabcloud_db::{core_set, DbPool, MigrationRunner};
        use crabcloud_storage::ChannelEventSink;

        // Build a stub pool synchronously for resolve-only tests by
        // tokio::runtime block_on. This is acceptable in a small unit
        // test; integration tests use the async harness in tests/support.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let pool = rt.block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let cfg =
                crabcloud_config::test_support::minimal_sqlite_config(dir.path().join("v.db"));
            std::mem::forget(dir);
            let pool = DbPool::connect(&cfg).await.unwrap();
            let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
            runner.register(core_set());
            runner.run().await.unwrap();
            pool
        });
        let _ = MemoryCache::new(); // anchor crabcloud_cache

        View::new(
            UserId::new("alice").unwrap(),
            mounts,
            Arc::new(FileCache::new(pool)),
            Arc::new(ChannelEventSink::new(8)),
        )
    }

    #[test]
    fn resolve_home_mount_strips_leading_slash() {
        let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new("alice"));
        let mount = Mount {
            path_prefix: StoragePath::root(),
            storage,
            metadata: None,
        };
        let view = build_view_with_mounts(vec![mount]);
        let (m, sp) = view
            .resolve(&UserPath::new("/photos/cat.jpg").unwrap())
            .unwrap();
        assert!(m.path_prefix.is_root());
        assert_eq!(sp.as_str(), "photos/cat.jpg");
    }

    #[test]
    fn resolve_root_user_path_yields_storage_root() {
        let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new("alice"));
        let mount = Mount {
            path_prefix: StoragePath::root(),
            storage,
            metadata: None,
        };
        let view = build_view_with_mounts(vec![mount]);
        let (_, sp) = view.resolve(&UserPath::root()).unwrap();
        assert!(sp.is_root());
    }

    #[test]
    fn resolve_picks_longest_matching_prefix() {
        let s1: Arc<dyn Storage> = Arc::new(MemoryStorage::new("home"));
        let s2: Arc<dyn Storage> = Arc::new(MemoryStorage::new("shared"));
        let mounts = vec![
            Mount {
                path_prefix: StoragePath::root(),
                storage: s1,
                metadata: None,
            },
            Mount {
                path_prefix: StoragePath::new("Shared").unwrap(),
                storage: s2,
                metadata: None,
            },
        ];
        let view = build_view_with_mounts(mounts);
        let (m, sp) = view
            .resolve(&UserPath::new("/Shared/joe/photo.jpg").unwrap())
            .unwrap();
        assert_eq!(m.storage.id(), "memory::shared");
        assert_eq!(sp.as_str(), "joe/photo.jpg");
    }

    #[test]
    fn resolve_no_match_errors() {
        // Empty mounts list — pathological but the wire shape is set.
        let view = build_view_with_mounts(vec![]);
        let r = view.resolve(&UserPath::new("/a").unwrap());
        assert!(matches!(r, Err(FsError::MountNotFound)));
    }
}
