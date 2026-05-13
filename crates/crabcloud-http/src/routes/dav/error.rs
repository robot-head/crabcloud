//! `DavError` — protocol-aware error type that converts to the right HTTP
//! status + (for some variants) a small XML body.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use crabcloud_filecache::FileCacheError;
use crabcloud_fs::FsError;
use crabcloud_storage::StorageError;

#[derive(Debug, thiserror::Error)]
pub enum DavError {
    #[error("not found")]
    NotFound,
    #[error("forbidden")]
    Forbidden,
    #[error("conflict")]
    Conflict,
    #[error("precondition failed")]
    PreconditionFailed,
    #[error("locked")]
    Locked,
    #[error("range not satisfiable")]
    RangeNotSatisfiable { file_size: u64 },
    #[error("propfind-finite-depth")]
    PropfindFiniteDepth,
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("internal: {0}")]
    Internal(String),
    #[error("storage: {0}")]
    Storage(#[from] StorageError),
    #[error("filecache: {0}")]
    FileCache(#[from] FileCacheError),
    #[error("fs: {0}")]
    Fs(#[from] FsError),
}

impl IntoResponse for DavError {
    fn into_response(self) -> Response {
        use axum::http::header;
        match self {
            DavError::NotFound => (StatusCode::NOT_FOUND, "").into_response(),
            DavError::Forbidden => (StatusCode::FORBIDDEN, "").into_response(),
            DavError::Conflict => (StatusCode::CONFLICT, "").into_response(),
            DavError::PreconditionFailed => (StatusCode::PRECONDITION_FAILED, "").into_response(),
            DavError::Locked => (StatusCode::LOCKED, "").into_response(),
            DavError::RangeNotSatisfiable { file_size } => (
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(header::CONTENT_RANGE, format!("bytes */{file_size}"))],
                "",
            )
                .into_response(),
            DavError::PropfindFiniteDepth => {
                let body = r#"<?xml version="1.0" encoding="utf-8"?><d:error xmlns:d="DAV:"><d:propfind-finite-depth/></d:error>"#;
                (
                    StatusCode::FORBIDDEN,
                    [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
                    body,
                )
                    .into_response()
            }
            DavError::BadRequest(m) => (StatusCode::BAD_REQUEST, m).into_response(),
            DavError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m).into_response(),
            DavError::Storage(StorageError::NotFound) => {
                (StatusCode::NOT_FOUND, "").into_response()
            }
            DavError::Storage(StorageError::AlreadyExists) => {
                (StatusCode::METHOD_NOT_ALLOWED, "").into_response()
            }
            DavError::Storage(StorageError::NotEmpty) => (StatusCode::CONFLICT, "").into_response(),
            DavError::Storage(StorageError::PermissionDenied) => {
                (StatusCode::FORBIDDEN, "").into_response()
            }
            DavError::Storage(StorageError::InvalidPath(m)) => {
                (StatusCode::BAD_REQUEST, m).into_response()
            }
            DavError::Storage(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("storage error: {e}"),
            )
                .into_response(),
            DavError::FileCache(FileCacheError::NotFound) => {
                (StatusCode::NOT_FOUND, "").into_response()
            }
            DavError::FileCache(FileCacheError::Storage(StorageError::NotFound)) => {
                (StatusCode::NOT_FOUND, "").into_response()
            }
            DavError::FileCache(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("filecache error: {e}"),
            )
                .into_response(),
            DavError::Fs(FsError::NotFound) => (StatusCode::NOT_FOUND, "").into_response(),
            DavError::Fs(FsError::InvalidPath(m)) => (StatusCode::BAD_REQUEST, m).into_response(),
            DavError::Fs(FsError::CrossMount) => (StatusCode::BAD_GATEWAY, "").into_response(),
            DavError::Fs(FsError::Storage(StorageError::NotFound)) => {
                (StatusCode::NOT_FOUND, "").into_response()
            }
            DavError::Fs(FsError::Storage(StorageError::PermissionDenied)) => {
                (StatusCode::FORBIDDEN, "").into_response()
            }
            DavError::Fs(FsError::FileCache(FileCacheError::NotFound)) => {
                (StatusCode::NOT_FOUND, "").into_response()
            }
            DavError::Fs(e) => {
                (StatusCode::INTERNAL_SERVER_ERROR, format!("fs error: {e}")).into_response()
            }
        }
    }
}

pub type DavResult<T> = Result<T, DavError>;
