use thiserror::Error;

#[derive(Debug, Error)]
pub enum SearchError {
    #[error("db: {0}")]
    Db(#[from] sqlx::Error),
    #[error("filecache: {0}")]
    FileCache(#[from] crabcloud_filecache::FileCacheError),
}
