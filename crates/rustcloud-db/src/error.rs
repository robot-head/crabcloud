#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("sqlx error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("invalid database URL: {0}")]
    InvalidUrl(String),
    #[error("migration error in namespace `{namespace}` version {version}: {message}")]
    Migration {
        namespace: String,
        version: i64,
        message: String,
    },
}

pub type DbResult<T> = Result<T, DbError>;
