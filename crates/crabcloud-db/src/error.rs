/// Errors produced by the database layer (pool connect, migrations, queries).
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// Wrapped `sqlx` error from the underlying pool or driver.
    #[error("sqlx error: {0}")]
    Sqlx(#[from] sqlx::Error),
    /// A required connection-string component (host, path, etc.) was missing or malformed.
    #[error("invalid database URL: {0}")]
    InvalidUrl(String),
    /// A migration failed; the namespace + version pinpoint the failing entry.
    #[error("migration error in namespace `{namespace}` version {version}: {message}")]
    Migration {
        /// Namespace of the failing migration set.
        namespace: String,
        /// Version number of the failing migration.
        version: i64,
        /// Underlying error message.
        message: String,
    },
}

/// Convenience alias for `Result<T, DbError>`.
pub type DbResult<T> = Result<T, DbError>;
