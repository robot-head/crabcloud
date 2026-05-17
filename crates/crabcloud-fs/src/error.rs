//! Error types for `crabcloud-fs`.

use crabcloud_filecache::FileCacheError;
use crabcloud_storage::StorageError;

#[derive(Debug, thiserror::Error)]
pub enum FsError {
    #[error("not found")]
    NotFound,
    #[error("invalid path: {0}")]
    InvalidPath(String),
    #[error("no mount matches user path")]
    MountNotFound,
    #[error("cross-mount operation not supported in this sub-project")]
    CrossMount,
    #[error("forbidden")]
    Forbidden,
    #[error("conflict")]
    Conflict,
    #[error("operation not supported")]
    Unsupported,
    #[error("cross-storage trash not supported in MVP")]
    CrossStorage,
    #[error("storage: {0}")]
    Storage(#[from] StorageError),
    #[error("filecache: {0}")]
    FileCache(#[from] FileCacheError),
    #[error("trash: {0}")]
    Trash(String),
    #[error("upload: {0}")]
    Upload(String),
}

pub type FsResult<T> = Result<T, FsError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_storage_error_wraps_as_storage() {
        let e: FsError = StorageError::NotFound.into();
        assert!(matches!(e, FsError::Storage(_)));
    }

    #[test]
    fn from_filecache_error_wraps() {
        let e: FsError = FileCacheError::NotFound.into();
        assert!(matches!(e, FsError::FileCache(_)));
    }

    #[test]
    fn cross_mount_message() {
        let s = format!("{}", FsError::CrossMount);
        assert!(s.contains("cross-mount"));
    }
}
