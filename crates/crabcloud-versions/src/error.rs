use thiserror::Error;

#[derive(Debug, Error)]
pub enum VersionsError {
    #[error("version row not found")]
    NotFound,
    #[error("version belongs to a different user")]
    WrongUser,
    #[error("source missing on disk")]
    SourceMissing,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("db: {0}")]
    Db(#[from] sqlx::Error),
}
