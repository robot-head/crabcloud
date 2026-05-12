//! `LocalStorageFactory` — backs each user's home with `<data_dir>/<uid>/files`.
//! `data_dir` comes from `FileConfig.datadirectory` (the existing Nextcloud-
//! compatible field; spec called it `[storage] data_dir` but the existing
//! field has identical semantics).

use crabcloud_storage::local::LocalStorage;
use crabcloud_storage::Storage;
use crabcloud_users::UserId;
use std::path::PathBuf;
use std::sync::Arc;

use crate::error::{FsError, FsResult};
use crate::mount::StorageFactory;

pub struct LocalStorageFactory {
    data_dir: PathBuf,
}

impl LocalStorageFactory {
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }
}

#[async_trait::async_trait]
impl StorageFactory for LocalStorageFactory {
    async fn home_storage(&self, uid: &UserId) -> FsResult<Arc<dyn Storage>> {
        let home = self.data_dir.join(uid.as_str()).join("files");
        tokio::fs::create_dir_all(&home)
            .await
            .map_err(|e| FsError::Storage(crabcloud_storage::StorageError::Io(e)))?;
        let storage = LocalStorage::new(home).map_err(FsError::Storage)?;
        Ok(Arc::new(storage))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn home_storage_creates_path() {
        let dir = tempdir().unwrap();
        let factory = LocalStorageFactory::new(dir.path().to_path_buf());
        let uid = UserId::new("alice").unwrap();
        let storage = factory.home_storage(&uid).await.unwrap();
        // The storage's id is `local::<canonicalized-path>`. We verify the path
        // ends with `alice/files`.
        assert!(
            storage.id().ends_with("alice/files") || storage.id().ends_with(r"alice\files"),
            "unexpected storage id: {}",
            storage.id()
        );
    }

    #[tokio::test]
    async fn home_storage_idempotent() {
        let dir = tempdir().unwrap();
        let factory = LocalStorageFactory::new(dir.path().to_path_buf());
        let uid = UserId::new("alice").unwrap();
        let s1 = factory.home_storage(&uid).await.unwrap();
        let s2 = factory.home_storage(&uid).await.unwrap();
        assert_eq!(s1.id(), s2.id());
    }

    #[tokio::test]
    async fn home_storage_distinct_users_distinct_storages() {
        let dir = tempdir().unwrap();
        let factory = LocalStorageFactory::new(dir.path().to_path_buf());
        let alice = factory
            .home_storage(&UserId::new("alice").unwrap())
            .await
            .unwrap();
        let bob = factory
            .home_storage(&UserId::new("bob").unwrap())
            .await
            .unwrap();
        assert_ne!(alice.id(), bob.id());
    }
}
