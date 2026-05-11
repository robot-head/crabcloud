//! Unified `Error` type for the core surface.
//!
//! Each kind has a HTTP status mapping (used by Phase 3's HTTP layer) that lives
//! here as a pure function — no axum types are pulled in.

use rustcloud_cache::CacheError;
use rustcloud_config::{FileConfigError, LoadError};
use rustcloud_db::DbError;

/// Unified error type used throughout the core surface. Each variant maps to a
/// specific HTTP status via [`Error::http_status`].
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Resource not found.
    #[error("not found")]
    NotFound,
    /// Caller is not authenticated.
    #[error("unauthorized")]
    Unauthorized,
    /// Caller is authenticated but not permitted.
    #[error("forbidden")]
    Forbidden,
    /// Caller-provided data was malformed; message is safe to surface.
    #[error("bad request: {0}")]
    BadRequest(String),
    /// State conflict (e.g. duplicate key); message is safe to surface.
    #[error("conflict: {0}")]
    Conflict(String),
    /// Resource is locked (WebDAV-style 423).
    #[error("locked")]
    Locked,
    /// OCS-protocol-level failure with an explicit status code and message.
    #[error("OCS error {code}: {message}")]
    Ocs {
        /// OCS-level status code to surface.
        code: u16,
        /// Human-readable message to include in the envelope.
        message: String,
    },
    /// Wrapped configuration load error.
    #[error(transparent)]
    Config(#[from] LoadError),
    /// Wrapped configuration validation error.
    #[error(transparent)]
    ConfigValidation(#[from] FileConfigError),
    /// Wrapped database error.
    #[error(transparent)]
    Db(#[from] DbError),
    /// Wrapped cache backend error.
    #[error(transparent)]
    Cache(#[from] CacheError),
    /// Catch-all for unexpected internal failures. The wrapped `anyhow::Error`
    /// is logged but not exposed to clients.
    #[error("internal error: {0:#}")]
    Internal(anyhow::Error),
}

impl Error {
    /// HTTP status code Phase 3's response layer will use. Internal/Db errors
    /// map to 500; auth issues to 401/403; validation to 400.
    pub fn http_status(&self) -> u16 {
        match self {
            Error::NotFound => 404,
            Error::Unauthorized => 401,
            Error::Forbidden => 403,
            Error::BadRequest(_) => 400,
            Error::Conflict(_) => 409,
            Error::Locked => 423,
            Error::Ocs { code, .. } => *code,
            Error::Config(_) | Error::ConfigValidation(_) => 500,
            Error::Db(_) => 500,
            Error::Cache(_) => 500,
            Error::Internal(_) => 500,
        }
    }

    /// A short, safe message that is OK to expose to clients. Internal errors
    /// produce a generic message; specific errors expose their reason.
    pub fn client_message(&self) -> String {
        match self {
            Error::NotFound => "Not Found".into(),
            Error::Unauthorized => "Unauthorized".into(),
            Error::Forbidden => "Forbidden".into(),
            Error::BadRequest(m) => m.clone(),
            Error::Conflict(m) => m.clone(),
            Error::Locked => "Locked".into(),
            Error::Ocs { message, .. } => message.clone(),
            Error::Config(_)
            | Error::ConfigValidation(_)
            | Error::Db(_)
            | Error::Cache(_)
            | Error::Internal(_) => "Internal Server Error".into(),
        }
    }
}

/// Convenience alias for `Result<T, Error>` used throughout the core API.
pub type CoreResult<T> = Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_status_mapping() {
        assert_eq!(Error::NotFound.http_status(), 404);
        assert_eq!(Error::Unauthorized.http_status(), 401);
        assert_eq!(Error::Forbidden.http_status(), 403);
        assert_eq!(Error::BadRequest("x".into()).http_status(), 400);
        assert_eq!(Error::Conflict("x".into()).http_status(), 409);
        assert_eq!(Error::Locked.http_status(), 423);
        assert_eq!(
            Error::Ocs {
                code: 418,
                message: "teapot".into()
            }
            .http_status(),
            418
        );
    }

    #[test]
    fn internal_errors_hide_details_in_client_message() {
        let e = Error::Internal(anyhow::anyhow!(
            "postgres exploded: rows=42, table=oc_users"
        ));
        assert_eq!(e.client_message(), "Internal Server Error");
        // Display still shows the chain (for logging).
        assert!(format!("{e:#}").contains("postgres exploded"));
    }

    #[test]
    fn bad_request_message_passes_through() {
        let e = Error::BadRequest("missing field 'email'".into());
        assert_eq!(e.client_message(), "missing field 'email'");
    }

    #[test]
    fn from_db_error_works() {
        let dberr = DbError::InvalidUrl("nope".into());
        let e: Error = dberr.into();
        assert!(matches!(e, Error::Db(_)));
        assert_eq!(e.http_status(), 500);
    }
}
