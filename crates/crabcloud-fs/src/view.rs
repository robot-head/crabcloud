//! `View` — per-user filesystem façade. Resolves user paths to
//! `(Mount, StoragePath)` via longest-prefix match; reads route through
//! the `FileCache`; writes go to storage with events emitted via the
//! shared `ChannelEventSink`.

use crate::error::{FsError, FsResult};
use crate::mount::Mount;
use crate::path::UserPath;
use crabcloud_filecache::FileCache;
use crabcloud_storage::{ChannelEventSink, DirEntry, FileMetadata, StoragePath};
use crabcloud_users::UserId;
use std::ops::Range;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::AsyncRead;

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
        let meta = self.filecache.stat(&mount.storage, &storage_path).await?;
        Ok(meta)
    }

    /// Cached directory listing.
    pub async fn list(&self, user_path: &UserPath) -> FsResult<Vec<DirEntry>> {
        let (mount, storage_path) = self.resolve(user_path)?;
        let entries = self.filecache.list(&mount.storage, &storage_path).await?;
        Ok(entries)
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
