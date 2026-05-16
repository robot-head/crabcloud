//! Preview-pipeline error type. Variants are mapped to deterministic HTTP
//! status codes at the handler boundary (see `crabcloud-http` Batch B).

use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;

/// Errors emitted by the preview pipeline. `Clone` is required so the
/// per-key dedup [`OnceCell`](tokio::sync::OnceCell) can hand the same
/// result to every concurrent waiter; non-cloneable inner errors are
/// wrapped in `Arc`.
#[derive(Debug, Error, Clone)]
pub enum PreviewError {
    #[error("mime not supported: {0}")]
    Unsupported(String),
    #[error("requested size {0} is above the maximum supported ladder rung")]
    SizeOutOfRange(u32),
    #[error("source image too large ({width}x{height}, max {max} pixels)")]
    SourceTooLarge { width: u32, height: u32, max: u32 },
    #[error("decode failed: {0}")]
    Decode(String),
    #[error("encode failed: {0}")]
    Encode(String),
    #[error("PDF render failed: {0}")]
    PdfRender(String),
    #[error("source path not found: {0:?}")]
    SourceNotFound(PathBuf),
    #[error(transparent)]
    Io(#[from] Arc<std::io::Error>),
    #[error(transparent)]
    Fs(#[from] Arc<crabcloud_fs::FsError>),
}

impl From<std::io::Error> for PreviewError {
    fn from(value: std::io::Error) -> Self {
        PreviewError::Io(Arc::new(value))
    }
}

impl From<crabcloud_fs::FsError> for PreviewError {
    fn from(value: crabcloud_fs::FsError) -> Self {
        PreviewError::Fs(Arc::new(value))
    }
}
