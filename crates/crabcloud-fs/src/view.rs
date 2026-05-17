//! `View` — per-user filesystem façade. Resolves user paths to
//! `(Mount, StoragePath)` via longest-prefix match; reads route through
//! the `FileCache`; writes go to storage with events emitted via the
//! shared `ChannelEventSink`.

use crate::error::{FsError, FsResult};
use crate::mount::{Mount, MountMetadata};
use crate::path::UserPath;
use crabcloud_filecache::FileCache;
use crabcloud_storage::{
    ChannelEventSink, DirEntry, EventSink, FileMetadata, Storage, StoragePath,
};
use crabcloud_users::UserId;
use std::ops::Range;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::AsyncRead;

/// Map a `TrashError` from the trash service into an `FsError`. The
/// trash error variants are MVP-tight, so most map to dedicated
/// `FsError` variants; anything else flattens into `FsError::Trash`.
fn map_trash_err(e: crabcloud_trash::TrashError) -> FsError {
    use crabcloud_trash::TrashError::*;
    match e {
        NotFound | SourceMissing => FsError::NotFound,
        WrongUser => FsError::Forbidden,
        RestoreCollision => FsError::Conflict,
        Io(e) => FsError::Storage(crabcloud_storage::StorageError::Io(e)),
        Db(e) => FsError::Trash(format!("db: {e}")),
        FileCache(s) => FsError::Trash(format!("filecache: {s}")),
    }
}

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
    /// Trash bin service. SP12: `View::delete` routes through here
    /// (soft-delete); `View::hard_delete` bypasses it.
    pub(crate) trash: Arc<crabcloud_trash::Trash>,
}

impl View {
    pub fn new(
        uid: UserId,
        mounts: Vec<Mount>,
        filecache: Arc<FileCache>,
        storage_sink: Arc<ChannelEventSink>,
        trash: Arc<crabcloud_trash::Trash>,
    ) -> Self {
        Self {
            uid,
            mounts,
            filecache,
            storage_sink,
            trash,
        }
    }

    pub fn uid(&self) -> &UserId {
        &self.uid
    }

    pub fn mounts(&self) -> &[Mount] {
        &self.mounts
    }

    /// Compute the `(Arc<dyn Storage>, StoragePath)` pair the filecache
    /// is keyed on for a given `user_path`. For ordinary home mounts this
    /// returns the home storage and the mount-relative storage path
    /// verbatim; for share-mount wrappers (`SharedSubrootStorage`) this
    /// translates through `Storage::inner_storage` so callers reach the
    /// owner-side cache row instead of a non-existent recipient-rooted
    /// row. Lets DAV adapters that talk to the filecache directly (e.g.
    /// PROPFIND for `oc:id` / favorites) stay correct under share mounts.
    pub fn cache_key_for(&self, user_path: &UserPath) -> FsResult<(Arc<dyn Storage>, StoragePath)> {
        let (mount, storage_path) = self.resolve(user_path)?;
        cache_key_for(&mount.storage, &storage_path)
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

    /// Soft-delete: routes through the trash service. Authed UI / DAV /
    /// OCS surfaces all reach this entry point. The trash service owns
    /// the on-disk rename; we still emit a `StorageEvent::Deleted` so
    /// the filecache scanner sees the file disappear from
    /// `<datadir>/<uid>/files/...` and removes its row.
    ///
    /// Share / public-link mounts (where the bytes live under another
    /// user's home storage) follow the spec §2 decision #7 path:
    /// the trash row's `user` column is the DELETER (`self.uid`), the
    /// `location` mirrors the deleter's view of the path, and the
    /// bytes are STREAMED into the deleter's `files_trashbin/files/`
    /// before the share-mount storage's `delete` runs. If `delete`
    /// fails (e.g. `PermissionDenied` on a read-only share), the
    /// trash file is rolled back so the storage layer's 403 still
    /// surfaces verbatim — `delete_read_link_returns_403` keeps
    /// passing.
    ///
    /// Directories on share mounts recurse: the whole subtree is copied
    /// into the deleter's trashbin under a single trash row of type
    /// `Dir`, then the source tree is removed from the share-owner's
    /// storage. On any failure during the copy, the partial trash
    /// destination is rolled back and the source is left intact.
    pub async fn delete(&self, user_path: &UserPath) -> FsResult<()> {
        let (mount, storage_path) = self.resolve(user_path)?;
        if mount.metadata.is_some() {
            return self
                .delete_via_share_mount(mount, storage_path, user_path)
                .await;
        }
        // Look up the type and best-effort fileid_legacy from
        // filecache before the bytes move. Errors here are
        // non-blocking — the soft_delete itself can still succeed.
        let (cache_storage, cache_path) = cache_key_for(&mount.storage, &storage_path)?;
        let row = self
            .filecache
            .lookup(cache_storage.id(), &cache_path)
            .await
            .ok()
            .flatten();
        let kind = match row.as_ref().map(|r| r.kind) {
            Some(crabcloud_storage::FileKind::Directory) => crabcloud_trash::TrashType::Dir,
            _ => crabcloud_trash::TrashType::File,
        };
        let fileid_legacy = row.as_ref().map(|r| r.fileid);

        self.trash
            .soft_delete(self.uid.as_str(), user_path.as_str(), kind, fileid_legacy)
            .await
            .map_err(map_trash_err)?;

        // Tell the scanner the bytes are gone from the user's home
        // storage so the filecache row is removed. The trash bin
        // storage is intentionally not in the filecache for SP12 MVP
        // (per spec §2 decision #4 the storage row is lazily created
        // for future use, but no scanner wires it up in Batch A).
        self.storage_sink
            .emit(crabcloud_storage::StorageEvent::Deleted {
                storage_id: mount.storage.id().to_string(),
                path: storage_path,
            })
            .await;
        Ok(())
    }

    /// Share-mount soft-delete: stream the file into the deleter's
    /// trashbin, then ask the share-mount storage to delete the source.
    /// Used by [`Self::delete`] when the resolved mount belongs to a
    /// share or public-link (i.e. `mount.metadata.is_some()`).
    ///
    /// Permission enforcement is delegated to the share-mount storage
    /// backend: if the deleter lacks delete on the share, the backend's
    /// `delete` returns `PermissionDenied` and we roll back the trash
    /// file before surfacing that error. The deleter never gets a
    /// trash row for bytes they couldn't legitimately delete.
    async fn delete_via_share_mount(
        &self,
        mount: &Mount,
        storage_path: StoragePath,
        user_path: &UserPath,
    ) -> FsResult<()> {
        // Look up the type before reading. Directory support on share
        // mounts is intentionally out of scope for MVP — recursive
        // streaming + per-entry trash rows would balloon the implementation;
        // we fall back to hard delete with a warn so the case is visible
        // in logs. Files are the common case.
        let (cache_storage, cache_path) = cache_key_for(&mount.storage, &storage_path)?;
        let row = self
            .filecache
            .lookup(cache_storage.id(), &cache_path)
            .await
            .ok()
            .flatten();
        // Filecache might not have a row for share-mount entries that
        // were never scanned (no scanner is wired up for share mounts in
        // SP12 MVP — the owner-side scanner sees them, but writes from
        // the owner-side aren't always reflected in tests that bypass
        // the scanner). Fall back to a storage `stat` to decide
        // file-vs-directory so the dir branch can fire for trees the
        // cache hasn't seen.
        let kind = match row.as_ref().map(|r| r.kind) {
            Some(crabcloud_storage::FileKind::Directory) => crabcloud_trash::TrashType::Dir,
            Some(crabcloud_storage::FileKind::File) => crabcloud_trash::TrashType::File,
            None => match mount.storage.stat(&storage_path).await {
                Ok(meta) => match meta.kind {
                    crabcloud_storage::FileKind::Directory => crabcloud_trash::TrashType::Dir,
                    crabcloud_storage::FileKind::File => crabcloud_trash::TrashType::File,
                },
                // If stat fails (e.g. read-only file-drop), assume File
                // and let the downstream read/delete surface the right
                // error — same behavior as the file-only path used to.
                Err(_) => crabcloud_trash::TrashType::File,
            },
        };
        let fileid_legacy = row.as_ref().map(|r| r.fileid);
        if matches!(kind, crabcloud_trash::TrashType::Dir) {
            return self
                .delete_directory_via_share_mount(mount, storage_path, user_path, fileid_legacy)
                .await;
        }

        // Read the source bytes from the share-mount storage. Reads on
        // a share mount don't go through the permission gate for delete,
        // so this succeeds for any link/share that grants read. (Read-
        // only shares are the read-only case we explicitly preserve a
        // 403 for: see the rollback after the delete attempt below.)
        let reader = mount.storage.read(&storage_path).await?;

        // Stream into the deleter's trashbin. Trash row records the
        // deleter as `user` and the deleter's view path as `location`.
        let basename = match std::path::Path::new(user_path.as_str())
            .file_name()
            .and_then(|s| s.to_str())
        {
            Some(b) => b.to_string(),
            None => return Err(FsError::InvalidPath(user_path.as_str().into())),
        };
        let location = match std::path::Path::new(user_path.as_str())
            .parent()
            .and_then(|p| p.to_str())
        {
            Some("") | None => "/".to_string(),
            Some(parent) => parent.to_string(),
        };

        let trash_id = self
            .trash
            .soft_delete_from_reader(
                self.uid.as_str(),
                &location,
                &basename,
                kind,
                fileid_legacy,
                reader,
            )
            .await
            .map_err(map_trash_err)?;

        // Now delete the source. If this fails — typically `PermissionDenied`
        // for a read-only share — roll back the trash entry so we don't
        // leak a copy of bytes the user couldn't legitimately remove.
        if let Err(e) = mount
            .storage
            .delete(&storage_path, &*self.storage_sink)
            .await
        {
            if let Err(purge_err) = self.trash.purge(self.uid.as_str(), trash_id).await {
                tracing::warn!(
                    error = %purge_err,
                    trash_id,
                    deleter = %self.uid.as_str(),
                    "share-mount delete rolled back, but trash purge of staged copy failed"
                );
            }
            return Err(e.into());
        }

        Ok(())
    }

    /// Share-mount directory soft-delete: recursively copy the subtree
    /// into the deleter's trashbin under a single `Dir` trash row, then
    /// remove the source from the share-owner's storage.
    ///
    /// Permission enforcement still belongs to the share-mount storage:
    /// the source `delete` runs through `mount.storage` so a read-only
    /// share surfaces `PermissionDenied` and we roll back the staged
    /// trash tree before returning. Like the file case the deleter
    /// never gets a trash row for a tree they couldn't legitimately
    /// remove.
    ///
    /// Local-only for SP12 MVP: the source path is computed as
    /// `<datadir>/<owner_uid>/files/<owner_storage_path>`, mirroring
    /// `LocalStorage`'s on-disk layout. Non-local share-mount backends
    /// (S3 etc.) aren't in scope — the spec already calls out cross-
    /// storage trash as deferred for non-local. Falls back to a
    /// `tracing::warn!` hard-delete if `owner_uid` isn't recorded on
    /// the mount (defensive — every share-mount the resolver builds
    /// sets it, but the public `Mount` struct allows `None`).
    async fn delete_directory_via_share_mount(
        &self,
        mount: &Mount,
        storage_path: StoragePath,
        user_path: &UserPath,
        fileid_legacy: Option<i64>,
    ) -> FsResult<()> {
        let Some(metadata) = mount.metadata.as_ref() else {
            // Shouldn't happen — caller verified is_some() — but be loud
            // if it ever does.
            tracing::warn!(
                deleter = %self.uid.as_str(),
                path = %user_path.as_str(),
                "share-mount directory delete: mount has no metadata; hard-deleting"
            );
            mount
                .storage
                .delete(&storage_path, &*self.storage_sink)
                .await?;
            return Ok(());
        };
        let Some(owner_uid) = metadata.owner_uid.as_deref() else {
            tracing::warn!(
                deleter = %self.uid.as_str(),
                path = %user_path.as_str(),
                "share-mount directory delete: mount metadata missing owner_uid; hard-deleting"
            );
            mount
                .storage
                .delete(&storage_path, &*self.storage_sink)
                .await?;
            return Ok(());
        };

        // Translate recipient-relative storage_path back to the owner-
        // side path via the SharedSubrootStorage wrapper's
        // `inner_storage` accessor. If the storage isn't wrapped (e.g.
        // an unwrapped share-owner home reused verbatim — not the path
        // any current resolver takes, but the trait permits it) we
        // assume the recipient path == owner path.
        let owner_relative = match mount.storage.inner_storage() {
            Some((_, prefix)) => {
                if storage_path.is_root() {
                    prefix.clone()
                } else if prefix.is_root() {
                    storage_path.clone()
                } else {
                    prefix.join(storage_path.as_str())?
                }
            }
            None => storage_path.clone(),
        };

        let basename = match std::path::Path::new(user_path.as_str())
            .file_name()
            .and_then(|s| s.to_str())
        {
            Some(b) => b.to_string(),
            None => return Err(FsError::InvalidPath(user_path.as_str().into())),
        };
        let location = match std::path::Path::new(user_path.as_str())
            .parent()
            .and_then(|p| p.to_str())
        {
            Some("") | None => "/".to_string(),
            Some(parent) => parent.to_string(),
        };

        // Permission gate: enforce the recipient's delete-bit BEFORE
        // staging any bytes. We can't rely on `mount.storage.delete()`
        // alone here because the underlying `Storage::delete` is empty-
        // dir only (returns `NotEmpty` for a populated tree), so a
        // read-write share with a non-empty subtree would error
        // `NotEmpty` for the wrong reason. Consult the recipient's
        // permission mask directly.
        if let Some(perms) = metadata.permissions {
            if !perms.allows_delete() {
                return Err(FsError::Storage(
                    crabcloud_storage::StorageError::PermissionDenied,
                ));
            }
        }

        let src_abs = self
            .trash
            .datadir()
            .join(owner_uid)
            .join("files")
            .join(owner_relative.as_str());

        // Stream the tree into bob's trashbin under one Dir row. On
        // any failure the trash side has already rolled back its
        // partial destination.
        let trash_id = self
            .trash
            .soft_delete_directory_from_path(
                self.uid.as_str(),
                &location,
                &basename,
                &src_abs,
                fileid_legacy,
            )
            .await
            .map_err(map_trash_err)?;

        // Source removal. `Storage::delete` only handles empty dirs,
        // so for the populated subtree we go straight to the local FS
        // — the spec is local-first for share-mount trash and the
        // source path was derived from `<datadir>/<owner_uid>/files/...`
        // a few lines above. If removal fails partway, roll back the
        // staged trash entry so we don't leak a copy of bytes that are
        // still partly present at the source.
        if let Err(e) = tokio::fs::remove_dir_all(&src_abs).await {
            if let Err(purge_err) = self.trash.purge(self.uid.as_str(), trash_id).await {
                tracing::warn!(
                    error = %purge_err,
                    trash_id,
                    deleter = %self.uid.as_str(),
                    "share-mount dir delete rolled back, but trash purge of staged copy failed"
                );
            }
            return Err(FsError::Storage(crabcloud_storage::StorageError::Io(e)));
        }

        // Emit a Deleted event on the deleter's behalf so the
        // filecache scanner removes the owner-side row. Path is the
        // owner-relative path under the owner's home storage; the
        // event's `storage_id` is the inner (owner) storage's id, NOT
        // the wrapper's recipient-rooted id.
        let event_storage_id = match mount.storage.inner_storage() {
            Some((inner, _)) => inner.id().to_string(),
            None => mount.storage.id().to_string(),
        };
        self.storage_sink
            .emit(crabcloud_storage::StorageEvent::Deleted {
                storage_id: event_storage_id,
                path: owner_relative,
            })
            .await;
        Ok(())
    }

    /// Hard-delete: removes the file without creating a trash entry.
    /// Use only when the caller has explicit authority to skip trash —
    /// anonymous public-link DELETE (Batch B switches public-link
    /// handlers here), the trash sweeper itself, etc.
    pub async fn hard_delete(&self, user_path: &UserPath) -> FsResult<()> {
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
        let (pool, datadir) = rt.block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let cfg =
                crabcloud_config::test_support::minimal_sqlite_config(dir.path().join("v.db"));
            let datadir = cfg.datadirectory.clone();
            std::mem::forget(dir);
            let pool = DbPool::connect(&cfg).await.unwrap();
            let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
            runner.register(core_set());
            runner.run().await.unwrap();
            (pool, datadir)
        });
        let _ = MemoryCache::new(); // anchor crabcloud_cache
        let trash = Arc::new(crabcloud_trash::Trash::new(Arc::new(pool.clone()), datadir));

        View::new(
            UserId::new("alice").unwrap(),
            mounts,
            Arc::new(FileCache::new(pool)),
            Arc::new(ChannelEventSink::new(8)),
            trash,
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
