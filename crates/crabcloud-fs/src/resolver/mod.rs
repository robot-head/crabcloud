//! `HomeMountResolver` — the 4c default. One home mount per user, anchored
//! at the root. Forward-design: future resolvers can layer share + external
//! mounts on top.

pub mod local;
pub mod public_link;
pub mod share;

pub use public_link::PublicLinkMountResolver;
pub use share::{FileCacheLookup, ShareMountResolver, SharesLookup};

use crabcloud_storage::StoragePath;
use crabcloud_users::UserId;
use std::sync::Arc;

use crate::error::FsResult;
use crate::mount::{Mount, MountResolver, StorageFactory};

pub struct HomeMountResolver {
    factory: Arc<dyn StorageFactory>,
}

impl HomeMountResolver {
    pub fn new(factory: Arc<dyn StorageFactory>) -> Self {
        Self { factory }
    }
}

#[async_trait::async_trait]
impl MountResolver for HomeMountResolver {
    async fn mounts_for(&self, uid: &UserId) -> FsResult<Vec<Mount>> {
        let storage = self.factory.home_storage(uid).await?;
        Ok(vec![Mount {
            path_prefix: StoragePath::root(),
            storage,
            metadata: None,
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crabcloud_storage::{memory::MemoryStorage, Storage};

    struct MemoryFactory;

    #[async_trait::async_trait]
    impl StorageFactory for MemoryFactory {
        async fn home_storage(&self, uid: &UserId) -> FsResult<Arc<dyn Storage>> {
            Ok(Arc::new(MemoryStorage::new(uid.as_str())))
        }
    }

    #[tokio::test]
    async fn home_resolver_returns_single_mount_at_root() {
        let resolver = HomeMountResolver::new(Arc::new(MemoryFactory));
        let uid = UserId::new("alice").unwrap();
        let mounts = resolver.mounts_for(&uid).await.unwrap();
        assert_eq!(mounts.len(), 1);
        assert!(mounts[0].path_prefix.is_root());
        assert_eq!(mounts[0].storage.id(), "memory::alice");
    }
}
