//! Mount + MountResolver + StorageFactory traits.
//!
//! A `Mount` binds a user-facing path prefix to a `Storage` backend. The
//! `MountResolver` is queried per-request to get the active mounts for a
//! user. `StorageFactory` is the per-backend constructor (local FS, future
//! S3, external storage).

use crabcloud_storage::{Storage, StoragePath};
use crabcloud_users::UserId;
use std::sync::Arc;

use crate::error::FsResult;

#[derive(Clone)]
pub struct Mount {
    /// User-facing path prefix. Empty (`StoragePath::root()`) for the home
    /// mount. Non-empty for share / external storage mounts in future
    /// sub-projects (e.g., `"Shared"` for `/Shared/...`).
    pub path_prefix: StoragePath,
    pub storage: Arc<dyn Storage>,
}

#[async_trait::async_trait]
pub trait MountResolver: Send + Sync {
    async fn mounts_for(&self, uid: &UserId) -> FsResult<Vec<Mount>>;
}

#[async_trait::async_trait]
pub trait StorageFactory: Send + Sync {
    /// Per-user home storage. For LocalStorage: `<data_dir>/<uid>/files`.
    async fn home_storage(&self, uid: &UserId) -> FsResult<Arc<dyn Storage>>;
}
