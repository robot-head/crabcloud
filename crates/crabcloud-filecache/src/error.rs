//! Error types for `crabcloud-filecache`.

use crabcloud_storage::{StorageError, StoragePath};

#[derive(Debug, thiserror::Error)]
pub enum FileCacheError {
    #[error("not found")]
    NotFound,
    #[error("ancestor missing: {0}")]
    AncestorMissing(StoragePath),
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("db error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("invalid state: {0}")]
    Invalid(String),
}

pub type FileCacheResult<T> = Result<T, FileCacheError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_error_wraps() {
        let e: FileCacheError = sqlx::Error::RowNotFound.into();
        assert!(matches!(e, FileCacheError::Db(_)));
    }

    #[test]
    fn storage_error_wraps() {
        let e: FileCacheError = StorageError::NotFound.into();
        assert!(matches!(e, FileCacheError::Storage(_)));
    }

    #[test]
    fn ancestor_missing_holds_path() {
        let p = StoragePath::new("a/b").unwrap();
        let e = FileCacheError::AncestorMissing(p.clone());
        match e {
            FileCacheError::AncestorMissing(got) => assert_eq!(got, p),
            _ => panic!("wrong variant"),
        }
    }
}

// Display formatting check happens implicitly through the `thiserror` derives.
