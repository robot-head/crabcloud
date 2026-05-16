//! `PublicLinkMountResolver` — anonymous public-link request resolver.
//!
//! Constructed once per incoming `/s/{token}` (or `/public.php/dav/files/{token}`)
//! request after the link has been authenticated and resolved to its
//! `(owner_uid, owner_path, permissions)` triple. Returns exactly one mount
//! per request: a `SharedSubrootStorage` rooted at the linked subtree with
//! the link's permission bits applied.
//!
//! The `uid` argument to `mounts_for` is ignored — the resolver was built
//! for a specific link and surfaces the same single mount regardless of who
//! asks. The caller typically passes a synthetic "anonymous" / owner-uid
//! placeholder so the `View` layer can continue to demand a `UserId`.
//!
//! Distinct from `ShareMountResolver`: that resolver layers share mounts on
//! top of a recipient's home mount (so logged-in users see their own files
//! plus shares). Public-link requests have no recipient — they get the
//! single share-rooted mount and nothing else.

use async_trait::async_trait;
use crabcloud_sharing::SharePermissions;
use crabcloud_storage::{Storage, StoragePath};
use crabcloud_users::UserId;
use std::sync::Arc;

use crate::error::FsResult;
use crate::mount::{Mount, MountKind, MountMetadata, MountResolver, StorageFactory};
use crate::storage::SharedSubrootStorage;

pub struct PublicLinkMountResolver {
    factory: Arc<dyn StorageFactory>,
    owner: UserId,
    owner_path: StoragePath,
    permissions: SharePermissions,
}

impl PublicLinkMountResolver {
    pub fn new(
        factory: Arc<dyn StorageFactory>,
        owner: UserId,
        owner_path: StoragePath,
        permissions: SharePermissions,
    ) -> Self {
        Self {
            factory,
            owner,
            owner_path,
            permissions,
        }
    }
}

#[async_trait]
impl MountResolver for PublicLinkMountResolver {
    async fn mounts_for(&self, _uid: &UserId) -> FsResult<Vec<Mount>> {
        let inner = self.factory.home_storage(&self.owner).await?;
        let wrapped: Arc<dyn Storage> = Arc::new(SharedSubrootStorage::new(
            inner,
            self.owner_path.clone(),
            self.permissions,
        ));
        Ok(vec![Mount {
            path_prefix: StoragePath::root(),
            storage: wrapped,
            metadata: Some(MountMetadata {
                kind: MountKind::Share,
                owner_uid: Some(self.owner.as_str().to_string()),
                permissions: Some(self.permissions),
            }),
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crabcloud_storage::{memory::MemoryStorage, NoopEventSink};
    use std::io::Cursor;
    use std::pin::Pin;
    use std::sync::Mutex;
    use tokio::io::AsyncRead;

    struct StubFactory {
        per_user: Mutex<Vec<(String, Arc<dyn Storage>)>>,
    }

    impl StubFactory {
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
    impl StorageFactory for StubFactory {
        async fn home_storage(&self, uid: &UserId) -> FsResult<Arc<dyn Storage>> {
            for (k, v) in self.per_user.lock().unwrap().iter() {
                if k == uid.as_str() {
                    return Ok(v.clone());
                }
            }
            Ok(Arc::new(MemoryStorage::new(uid.as_str())))
        }
    }

    fn body(bytes: &[u8]) -> Pin<Box<dyn AsyncRead + Send>> {
        Box::pin(Cursor::new(bytes.to_vec()))
    }

    async fn seeded_alice_home() -> Arc<dyn Storage> {
        let s: Arc<dyn Storage> = Arc::new(MemoryStorage::new("alice"));
        s.mkdir(&StoragePath::new("Photos").unwrap(), &NoopEventSink)
            .await
            .unwrap();
        s.put_file(
            &StoragePath::new("Photos/cat.jpg").unwrap(),
            body(b"meow"),
            &NoopEventSink,
        )
        .await
        .unwrap();
        s
    }

    #[tokio::test]
    async fn resolver_returns_single_mount_at_root_with_share_metadata() {
        let factory = Arc::new(StubFactory::new());
        factory.install("alice", seeded_alice_home().await);

        let resolver = PublicLinkMountResolver::new(
            factory,
            UserId::new("alice").unwrap(),
            StoragePath::new("Photos").unwrap(),
            SharePermissions::from_wire(1), // read-only link
        );
        // `mounts_for` ignores its arg — pass anything.
        let mounts = resolver
            .mounts_for(&UserId::new("anonymous").unwrap())
            .await
            .unwrap();
        assert_eq!(mounts.len(), 1);
        assert!(mounts[0].path_prefix.is_root());
        let md = mounts[0].metadata.as_ref().expect("share metadata present");
        assert_eq!(md.kind, MountKind::Share);
        assert_eq!(md.owner_uid.as_deref(), Some("alice"));
        assert_eq!(md.permissions, Some(SharePermissions::from_wire(1)));
    }

    #[tokio::test]
    async fn resolver_mount_views_owner_subroot_contents() {
        // The single mount must surface the linked subtree's contents at
        // its own root (recipient sees `Photos/cat.jpg` as `/cat.jpg`).
        let factory = Arc::new(StubFactory::new());
        factory.install("alice", seeded_alice_home().await);

        let resolver = PublicLinkMountResolver::new(
            factory,
            UserId::new("alice").unwrap(),
            StoragePath::new("Photos").unwrap(),
            SharePermissions::from_wire(1),
        );
        let mounts = resolver
            .mounts_for(&UserId::new("anonymous").unwrap())
            .await
            .unwrap();
        let entries = mounts[0].storage.list(&StoragePath::root()).await.unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["cat.jpg"]);
    }

    #[tokio::test]
    async fn resolver_create_only_link_hides_children() {
        // File-drop link: bit 4 set, bit 1 clear. The mount should still
        // exist (so the upload widget can post) but list/stat children
        // must be hidden by the wrapped SharedSubrootStorage.
        let factory = Arc::new(StubFactory::new());
        factory.install("alice", seeded_alice_home().await);

        let resolver = PublicLinkMountResolver::new(
            factory,
            UserId::new("alice").unwrap(),
            StoragePath::new("Photos").unwrap(),
            SharePermissions::from_wire(4),
        );
        let mounts = resolver
            .mounts_for(&UserId::new("anonymous").unwrap())
            .await
            .unwrap();
        let entries = mounts[0].storage.list(&StoragePath::root()).await.unwrap();
        assert!(
            entries.is_empty(),
            "file-drop mount must hide listing; got {entries:?}"
        );
    }
}
