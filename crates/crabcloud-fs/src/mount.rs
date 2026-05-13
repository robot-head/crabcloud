//! Mount + MountResolver + StorageFactory traits.
//!
//! A `Mount` binds a user-facing path prefix to a `Storage` backend. The
//! `MountResolver` is queried per-request to get the active mounts for a
//! user. `StorageFactory` is the per-backend constructor (local FS, future
//! S3, external storage).

use crabcloud_sharing::SharePermissions;
use crabcloud_storage::{Storage, StoragePath};
use crabcloud_users::UserId;
use std::sync::Arc;

use crate::error::FsResult;

/// Discriminates a `Mount` between the user's home mount and a share mount
/// surfaced by a higher-level resolver. View / DTO decoration (SP7+) uses
/// this plus `owner_uid` to decorate share-rooted entries with `shared_by`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MountKind {
    Home,
    Share,
}

/// Per-mount metadata. `None` on the home mount; `Some` on share / external
/// mounts. `owner_uid` is the resource owner (alice when bob is consuming a
/// share); `permissions` is the recipient-facing permission mask.
#[derive(Clone, Debug)]
pub struct MountMetadata {
    pub kind: MountKind,
    pub owner_uid: Option<String>,
    pub permissions: Option<SharePermissions>,
}

#[derive(Clone)]
pub struct Mount {
    /// User-facing path prefix. Empty (`StoragePath::root()`) for the home
    /// mount. Non-empty for share / external storage mounts in future
    /// sub-projects (e.g., `"Shared"` for `/Shared/...`).
    pub path_prefix: StoragePath,
    pub storage: Arc<dyn Storage>,
    /// `None` for the home mount; `Some` for share / external mounts.
    pub metadata: Option<MountMetadata>,
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
