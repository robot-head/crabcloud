use thiserror::Error;

#[derive(Debug, Error)]
pub enum VersionsError {
    #[error("version row not found")]
    NotFound,
    #[error("version belongs to a different user")]
    WrongUser,
    #[error("source missing on disk")]
    SourceMissing,
    /// Concurrent writer in the same `version_mtime` second already
    /// wrote a row for this `(storage_id, fileid, version_mtime)`.
    /// `snapshot_if_needed` maps this to `Ok(None)` so the duplicate
    /// is a soft skip — both racers produce byte-identical copies of
    /// the same source bytes, so dropping one is lossless.
    #[error("duplicate snapshot row (concurrent writer)")]
    DuplicateSnapshot,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("db: {0}")]
    Db(#[from] sqlx::Error),
}
