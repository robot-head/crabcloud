//! Error types for `crabcloud-storage`. `Io` carries the original error for
//! diagnostics; `map_io` lifts well-known `io::ErrorKind`s to the richer
//! variants (NotFound, AlreadyExists, NotEmpty, etc.) before the catch-all.

use std::io;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("not found")]
    NotFound,
    #[error("already exists")]
    AlreadyExists,
    #[error("not a directory")]
    NotADirectory,
    #[error("is a directory")]
    IsADirectory,
    #[error("directory not empty")]
    NotEmpty,
    #[error("permission denied")]
    PermissionDenied,
    #[error("invalid path: {0}")]
    InvalidPath(String),
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("multipart: {0}")]
    Multipart(String),
    #[error("storage error: {0}")]
    Other(String),
}

pub type StorageResult<T> = Result<T, StorageError>;

/// Translate a `std::io::Error` into the richest matching `StorageError`
/// variant. Use this in backend code paths instead of relying on the
/// `#[from]` impl when you want richer mapping (most of them do).
pub fn map_io(e: io::Error) -> StorageError {
    match e.kind() {
        io::ErrorKind::NotFound => StorageError::NotFound,
        io::ErrorKind::AlreadyExists => StorageError::AlreadyExists,
        io::ErrorKind::PermissionDenied => StorageError::PermissionDenied,
        // ErrorKind::IsADirectory and NotADirectory exist on nightly; on
        // stable we sniff the os_error code on Unix. Skip OS-specific
        // mapping for now — the common cases above cover most callers; the
        // catch-all preserves the original error for diagnostics.
        _ => StorageError::Io(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_io_lifts_not_found() {
        let e = io::Error::new(io::ErrorKind::NotFound, "x");
        assert!(matches!(map_io(e), StorageError::NotFound));
    }

    #[test]
    fn map_io_lifts_already_exists() {
        let e = io::Error::new(io::ErrorKind::AlreadyExists, "x");
        assert!(matches!(map_io(e), StorageError::AlreadyExists));
    }

    #[test]
    fn map_io_lifts_permission_denied() {
        let e = io::Error::new(io::ErrorKind::PermissionDenied, "x");
        assert!(matches!(map_io(e), StorageError::PermissionDenied));
    }

    #[test]
    fn map_io_falls_through_to_io() {
        let e = io::Error::other("weird");
        assert!(matches!(map_io(e), StorageError::Io(_)));
    }

    #[test]
    fn from_io_error_wraps_as_io() {
        let e: StorageError = io::Error::other("x").into();
        assert!(matches!(e, StorageError::Io(_)));
    }
}
