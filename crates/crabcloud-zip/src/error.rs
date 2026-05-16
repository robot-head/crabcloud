//! Error types for `crabcloud-zip`.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum WalkError {
    #[error("folder too large ({count} entries, {bytes} bytes)")]
    TooLarge { count: u64, bytes: u64 },
    #[error(transparent)]
    View(#[from] crabcloud_fs::FsError),
}

#[derive(Debug, Error)]
pub enum ZipError {
    #[error(transparent)]
    Walk(#[from] WalkError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
}
