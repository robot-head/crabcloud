use thiserror::Error;

#[derive(Debug, Error)]
pub enum ActivityError {
    #[error("row not found")]
    NotFound,
    #[error("db: {0}")]
    Db(#[from] sqlx::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

/// Wrapper error type returned by [`crate::ActivityEmitter::emit`] so
/// emitter crates can depend on a stable boundary type rather than the
/// concrete [`ActivityError`].
#[derive(Debug, Error)]
#[error("activity emit failed: {0}")]
pub struct ActivityEmitError(pub String);

impl From<ActivityError> for ActivityEmitError {
    fn from(e: ActivityError) -> Self {
        Self(e.to_string())
    }
}
