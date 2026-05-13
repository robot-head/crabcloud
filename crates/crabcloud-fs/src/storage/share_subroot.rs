//! `SharedSubrootStorage` — a `Storage` wrapper that pins an owner's home
//! storage at a subroot (`owner_path`) and filters mutating operations through
//! a `SharePermissions` mask. The recipient sees `owner_path` as `/`; every
//! `StoragePath` flowing through this adapter is translated by
//! `owner_path.join(p)` before reaching the inner storage.
//!
//! Invariants matched against the SP7 spec §3.4 + §6:
//! - `id()` returns the inner storage id verbatim — filecache rows still
//!   route into the owner's namespace.
//! - Reads (`read`, `read_range`, `list`, `stat`, `exists`) are unconditional.
//! - `mkdir` requires bit 4 (create).
//! - `put_file` requires bit 2 (update) when the target already exists,
//!   bit 4 (create) when it doesn't. The `Storage` trait has no
//!   create-vs-overwrite flag, so we `exists()` first.
//! - `delete` requires bit 8.
//! - `rename` is within-mount (cross-mount moves get decomposed by the View
//!   layer) so it requires bit 2 (update) — relocates an existing entry.
//! - `copy` is within-mount but creates a NEW entry at the destination, so
//!   it requires bit 4 (create), not bit 2.

use async_trait::async_trait;
use crabcloud_sharing::SharePermissions;
use crabcloud_storage::{
    DirEntry, EventSink, FileMetadata, MultipartHandle, PartTag, Storage, StorageError,
    StoragePath, StorageResult,
};
use std::ops::Range;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::AsyncRead;

pub struct SharedSubrootStorage {
    inner: Arc<dyn Storage>,
    owner_path: StoragePath,
    permissions: SharePermissions,
}

impl SharedSubrootStorage {
    pub fn new(
        inner: Arc<dyn Storage>,
        owner_path: StoragePath,
        permissions: SharePermissions,
    ) -> Self {
        Self {
            inner,
            owner_path,
            permissions,
        }
    }

    fn translate(&self, recipient_relative: &StoragePath) -> StorageResult<StoragePath> {
        if recipient_relative.is_root() {
            return Ok(self.owner_path.clone());
        }
        if self.owner_path.is_root() {
            return Ok(recipient_relative.clone());
        }
        self.owner_path.join(recipient_relative.as_str())
    }
}

#[async_trait]
impl Storage for SharedSubrootStorage {
    fn id(&self) -> &str {
        self.inner.id()
    }

    async fn stat(&self, path: &StoragePath) -> StorageResult<FileMetadata> {
        self.inner.stat(&self.translate(path)?).await
    }

    async fn exists(&self, path: &StoragePath) -> StorageResult<bool> {
        self.inner.exists(&self.translate(path)?).await
    }

    async fn list(&self, path: &StoragePath) -> StorageResult<Vec<DirEntry>> {
        self.inner.list(&self.translate(path)?).await
    }

    async fn read(&self, path: &StoragePath) -> StorageResult<Pin<Box<dyn AsyncRead + Send>>> {
        self.inner.read(&self.translate(path)?).await
    }

    async fn read_range(
        &self,
        path: &StoragePath,
        range: Range<u64>,
    ) -> StorageResult<Pin<Box<dyn AsyncRead + Send>>> {
        self.inner.read_range(&self.translate(path)?, range).await
    }

    async fn put_file(
        &self,
        path: &StoragePath,
        body: Pin<Box<dyn AsyncRead + Send>>,
        sink: &dyn EventSink,
    ) -> StorageResult<FileMetadata> {
        let translated = self.translate(path)?;
        let existing = self.inner.exists(&translated).await?;
        let allowed = if existing {
            self.permissions.allows_update()
        } else {
            self.permissions.allows_create()
        };
        if !allowed {
            return Err(StorageError::PermissionDenied);
        }
        self.inner.put_file(&translated, body, sink).await
    }

    async fn mkdir(&self, path: &StoragePath, sink: &dyn EventSink) -> StorageResult<FileMetadata> {
        if !self.permissions.allows_create() {
            return Err(StorageError::PermissionDenied);
        }
        self.inner.mkdir(&self.translate(path)?, sink).await
    }

    async fn delete(&self, path: &StoragePath, sink: &dyn EventSink) -> StorageResult<()> {
        if !self.permissions.allows_delete() {
            return Err(StorageError::PermissionDenied);
        }
        self.inner.delete(&self.translate(path)?, sink).await
    }

    async fn rename(
        &self,
        from: &StoragePath,
        to: &StoragePath,
        sink: &dyn EventSink,
    ) -> StorageResult<()> {
        if !self.permissions.allows_update() {
            return Err(StorageError::PermissionDenied);
        }
        self.inner
            .rename(&self.translate(from)?, &self.translate(to)?, sink)
            .await
    }

    async fn copy(
        &self,
        from: &StoragePath,
        to: &StoragePath,
        sink: &dyn EventSink,
    ) -> StorageResult<()> {
        if !self.permissions.allows_create() {
            return Err(StorageError::PermissionDenied);
        }
        self.inner
            .copy(&self.translate(from)?, &self.translate(to)?, sink)
            .await
    }

    async fn begin_multipart(
        &self,
        target: &StoragePath,
        sink: &dyn EventSink,
    ) -> StorageResult<MultipartHandle> {
        let translated = self.translate(target)?;
        let existing = self.inner.exists(&translated).await?;
        let allowed = if existing {
            self.permissions.allows_update()
        } else {
            self.permissions.allows_create()
        };
        if !allowed {
            return Err(StorageError::PermissionDenied);
        }
        self.inner.begin_multipart(&translated, sink).await
    }

    async fn put_part(
        &self,
        handle: &MultipartHandle,
        part_number: u32,
        body: Pin<Box<dyn AsyncRead + Send>>,
    ) -> StorageResult<PartTag> {
        self.inner.put_part(handle, part_number, body).await
    }

    async fn commit_multipart(
        &self,
        handle: MultipartHandle,
        parts: Vec<PartTag>,
        sink: &dyn EventSink,
    ) -> StorageResult<FileMetadata> {
        self.inner.commit_multipart(handle, parts, sink).await
    }

    async fn abort_multipart(&self, handle: MultipartHandle) -> StorageResult<()> {
        self.inner.abort_multipart(handle).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crabcloud_storage::{memory::MemoryStorage, NoopEventSink};
    use std::io::Cursor;

    fn body(bytes: &[u8]) -> Pin<Box<dyn AsyncRead + Send>> {
        Box::pin(Cursor::new(bytes.to_vec()))
    }

    async fn seed_owner() -> Arc<dyn Storage> {
        let inner: Arc<dyn Storage> = Arc::new(MemoryStorage::new("alice"));
        inner
            .mkdir(
                &StoragePath::new("Vacation Photos").unwrap(),
                &NoopEventSink,
            )
            .await
            .unwrap();
        inner
            .put_file(
                &StoragePath::new("Vacation Photos/x.jpg").unwrap(),
                body(b"jpeg-bytes"),
                &NoopEventSink,
            )
            .await
            .unwrap();
        inner
    }

    fn wrap(inner: Arc<dyn Storage>, perms_wire: u32) -> SharedSubrootStorage {
        SharedSubrootStorage::new(
            inner,
            StoragePath::new("Vacation Photos").unwrap(),
            SharePermissions::from_wire(perms_wire),
        )
    }

    #[tokio::test]
    async fn id_passes_through_to_inner() {
        let inner = seed_owner().await;
        let id = inner.id().to_string();
        let s = wrap(inner, 0x0F);
        assert_eq!(s.id(), id);
    }

    #[tokio::test]
    async fn list_root_returns_wrapped_contents() {
        let s = wrap(seed_owner().await, 3);
        let entries = s.list(&StoragePath::root()).await.unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["x.jpg"]);
    }

    #[tokio::test]
    async fn write_existing_with_update_bit_succeeds() {
        let s = wrap(seed_owner().await, 3);
        let p = StoragePath::new("x.jpg").unwrap();
        let m = s
            .put_file(&p, body(b"newer"), &NoopEventSink)
            .await
            .unwrap();
        assert_eq!(m.size, 5);
    }

    #[tokio::test]
    async fn write_new_without_create_bit_denied() {
        let s = wrap(seed_owner().await, 3);
        let r = s
            .put_file(
                &StoragePath::new("new.jpg").unwrap(),
                body(b"abc"),
                &NoopEventSink,
            )
            .await;
        assert!(matches!(r, Err(StorageError::PermissionDenied)));
    }

    #[tokio::test]
    async fn mkdir_without_create_bit_denied() {
        let s = wrap(seed_owner().await, 3);
        let r = s
            .mkdir(&StoragePath::new("sub").unwrap(), &NoopEventSink)
            .await;
        assert!(matches!(r, Err(StorageError::PermissionDenied)));
    }

    #[tokio::test]
    async fn delete_without_delete_bit_denied() {
        let s = wrap(seed_owner().await, 3);
        let r = s
            .delete(&StoragePath::new("x.jpg").unwrap(), &NoopEventSink)
            .await;
        assert!(matches!(r, Err(StorageError::PermissionDenied)));
    }

    #[tokio::test]
    async fn full_perms_unblock_previously_denied_ops() {
        let s = wrap(seed_owner().await, 0x0F);
        s.put_file(
            &StoragePath::new("new.jpg").unwrap(),
            body(b"abc"),
            &NoopEventSink,
        )
        .await
        .unwrap();
        s.mkdir(&StoragePath::new("sub").unwrap(), &NoopEventSink)
            .await
            .unwrap();
        s.delete(&StoragePath::new("x.jpg").unwrap(), &NoopEventSink)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn rename_with_update_bit_succeeds() {
        let s = wrap(seed_owner().await, 3); // read + update
        s.rename(
            &StoragePath::new("x.jpg").unwrap(),
            &StoragePath::new("y.jpg").unwrap(),
            &NoopEventSink,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn rename_without_update_bit_denied() {
        let s = wrap(seed_owner().await, 1); // read only
        let r = s
            .rename(
                &StoragePath::new("x.jpg").unwrap(),
                &StoragePath::new("y.jpg").unwrap(),
                &NoopEventSink,
            )
            .await;
        assert!(matches!(r, Err(StorageError::PermissionDenied)));
    }

    #[tokio::test]
    async fn copy_without_create_bit_denied() {
        // read|update (3) — can edit existing entries but not create new ones.
        // Copy creates a new entry at the destination, so must be denied.
        let s = wrap(seed_owner().await, 3);
        let r = s
            .copy(
                &StoragePath::new("x.jpg").unwrap(),
                &StoragePath::new("x-copy.jpg").unwrap(),
                &NoopEventSink,
            )
            .await;
        assert!(matches!(r, Err(StorageError::PermissionDenied)));
    }

    #[tokio::test]
    async fn copy_with_create_bit_succeeds() {
        let s = wrap(seed_owner().await, 0x0F);
        s.copy(
            &StoragePath::new("x.jpg").unwrap(),
            &StoragePath::new("x-copy.jpg").unwrap(),
            &NoopEventSink,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn begin_multipart_new_without_create_bit_denied() {
        // read|update (3) — same as the put_file new-path case.
        let s = wrap(seed_owner().await, 3);
        let r = s
            .begin_multipart(&StoragePath::new("new.bin").unwrap(), &NoopEventSink)
            .await;
        assert!(matches!(r, Err(StorageError::PermissionDenied)));
    }

    #[tokio::test]
    async fn begin_multipart_existing_without_update_bit_denied() {
        let s = wrap(seed_owner().await, 1); // read only
        let r = s
            .begin_multipart(&StoragePath::new("x.jpg").unwrap(), &NoopEventSink)
            .await;
        assert!(matches!(r, Err(StorageError::PermissionDenied)));
    }

    #[tokio::test]
    async fn translate_does_not_leak_above_owner_path() {
        // Sanity check: a wrapped storage cannot reach sibling content.
        // Recipient asks to read at the root of their view; that's exactly
        // the owner's `Vacation Photos` directory.
        let inner = seed_owner().await;
        inner
            .put_file(
                &StoragePath::new("secret.txt").unwrap(),
                body(b"hush"),
                &NoopEventSink,
            )
            .await
            .unwrap();
        let s = wrap(inner, 3);
        let names: Vec<_> = s
            .list(&StoragePath::root())
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert!(!names.contains(&"secret.txt".to_string()));
    }
}
