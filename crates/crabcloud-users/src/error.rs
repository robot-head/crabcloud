//! Error type for the users crate.

#[derive(Debug, thiserror::Error)]
pub enum UsersError {
    #[error("user not found")]
    NotFound,
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("account disabled")]
    Disabled,
    #[error("invalid uid: {0}")]
    InvalidUid(String),
    #[error("invalid email: {0}")]
    InvalidEmail(String),
    #[error("invalid display name: {0}")]
    InvalidDisplayName(String),
    #[error("uid already exists")]
    UidAlreadyExists,
    #[error("email already taken")]
    EmailAlreadyTaken,
    #[error("backend is read-only")]
    ReadOnly,
    #[error("password rejected: {0}")]
    PasswordTooWeak(&'static str),
    #[error(transparent)]
    Db(#[from] crabcloud_db::DbError),
    #[error(transparent)]
    Cache(#[from] crabcloud_cache::CacheError),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

pub type UsersResult<T> = Result<T, UsersError>;
