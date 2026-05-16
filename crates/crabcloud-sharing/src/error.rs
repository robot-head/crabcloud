//! Errors returned by the Shares service.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ShareError {
    #[error("share not found")]
    NotFound,

    #[error("forbidden")]
    Forbidden,

    #[error("recipient unknown")]
    RecipientUnknown,

    #[error("invalid share type")]
    InvalidShareType,

    #[error("bad permissions bitmask")]
    BadPermissions,

    #[error("invalid expireDate format: {0}")]
    InvalidExpireDate(&'static str),

    #[error("re-share rejected: only the owner of a file can share it")]
    ReshareRejected,

    #[error("path not owned by requester (or missing)")]
    PathNotOwned,

    #[error("not implemented in this version (deferred to SP8)")]
    NotImplemented,

    #[error(transparent)]
    DbError(#[from] sqlx::Error),
}

impl ShareError {
    /// HTTP status code that best maps to this error. Used by the OCS
    /// handler layer; kept here so error→status is one consistent table.
    pub fn http_status(&self) -> u16 {
        match self {
            ShareError::NotFound => 404,
            ShareError::Forbidden | ShareError::ReshareRejected | ShareError::PathNotOwned => 403,
            ShareError::RecipientUnknown => 404,
            ShareError::InvalidShareType
            | ShareError::BadPermissions
            | ShareError::InvalidExpireDate(_) => 400,
            ShareError::NotImplemented => 501,
            ShareError::DbError(_) => 500,
        }
    }
}
