use thiserror::Error;

#[derive(Debug, Error)]
pub enum TrashError {
    #[error("trash entry not found")]
    NotFound,
    #[error("trash entry belongs to a different user")]
    WrongUser,
    #[error("restore destination collision could not be resolved")]
    RestoreCollision,
    #[error("source not found in user storage")]
    SourceMissing,
    #[error("cross-storage trash not supported in MVP")]
    CrossStorage,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("db: {0}")]
    Db(#[from] sqlx::Error),
    #[error("filecache: {0}")]
    FileCache(String),
}
