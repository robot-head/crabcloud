//! WebDAV method handlers. Each handler is dispatched by HTTP method via
//! `dispatch_files` (axum's `any` route).

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use crabcloud_core::AppState;
use crabcloud_storage::FileKind;
use futures::StreamExt as _;
use tokio_util::io::ReaderStream;

use crate::extractors::auth::AuthenticatedUser;
use crate::routes::dav::error::{DavError, DavResult};
use crate::routes::dav::extractor::resolve_target;
use crate::routes::dav::headers::{
    parse_if_match, parse_if_none_match_wildcard, parse_range, IfMatch,
};

/// Default Allow header listing methods SP5 supports.
const ALLOW_HEADER: &str =
    "OPTIONS, GET, HEAD, PUT, MKCOL, DELETE, MOVE, COPY, PROPFIND, PROPPATCH, LOCK, UNLOCK";

/// `OPTIONS /dav/files` — root capability probe (no user context).
pub async fn options_capability_root() -> Response {
    capability_response()
}

fn capability_response() -> Response {
    (
        StatusCode::OK,
        [
            (header::ALLOW, HeaderValue::from_static(ALLOW_HEADER)),
            (
                header::HeaderName::from_static("dav"),
                HeaderValue::from_static("1, 2, 3"),
            ),
            (
                header::HeaderName::from_static("ms-author-via"),
                HeaderValue::from_static("DAV"),
            ),
        ],
        "",
    )
        .into_response()
}

/// Dispatch by method for `/dav/files/{user}` (path is root).
pub async fn dispatch_files_root(
    State(state): State<AppState>,
    authed: AuthenticatedUser,
    headers: HeaderMap,
    Path(user): Path<String>,
    method: Method,
    body: Body,
) -> Result<Response, DavError> {
    dispatch_inner(state, authed, headers, user, String::new(), method, body).await
}

/// Dispatch for `/dav/files/{user}/{*path}`.
pub async fn dispatch_files(
    State(state): State<AppState>,
    authed: AuthenticatedUser,
    headers: HeaderMap,
    Path((user, path)): Path<(String, String)>,
    method: Method,
    body: Body,
) -> Result<Response, DavError> {
    dispatch_inner(state, authed, headers, user, path, method, body).await
}

async fn dispatch_inner(
    state: AppState,
    authed: AuthenticatedUser,
    headers: HeaderMap,
    url_user: String,
    url_path: String,
    method: Method,
    body: Body,
) -> Result<Response, DavError> {
    let (uid, user_path) = resolve_target(&authed, &url_user, &url_path)?;
    match method {
        Method::OPTIONS => Ok(capability_response()),
        Method::GET | Method::HEAD => {
            get_or_head(state, &uid, &user_path, &headers, method == Method::HEAD).await
        }
        Method::PUT => put(state, &uid, &user_path, &headers, body).await,
        m if m.as_str() == "MKCOL" => mkcol(state, &uid, &user_path).await,
        Method::DELETE => delete(state, &uid, &user_path).await,
        m if m.as_str() == "MOVE" => {
            crate::routes::dav::moves::move_(state, &uid, &user_path, &headers).await
        }
        m if m.as_str() == "COPY" => {
            crate::routes::dav::moves::copy(state, &uid, &user_path, &headers).await
        }
        m if m.as_str() == "PROPFIND" => {
            crate::routes::dav::propfind::handle(state, &uid, &user_path, &headers).await
        }
        m if m.as_str() == "PROPPATCH" => {
            crate::routes::dav::proppatch::handle(state, &uid, &user_path, body).await
        }
        // LOCK/UNLOCK land in batch F.
        m if matches!(m.as_str(), "LOCK" | "UNLOCK") => Err(DavError::BadRequest(format!(
            "{} not yet implemented",
            m.as_str()
        ))),
        _ => Ok((
            StatusCode::METHOD_NOT_ALLOWED,
            [(header::ALLOW, HeaderValue::from_static(ALLOW_HEADER))],
            "",
        )
            .into_response()),
    }
}

async fn get_or_head(
    state: AppState,
    uid: &crabcloud_users::UserId,
    user_path: &crabcloud_fs::UserPath,
    headers: &HeaderMap,
    head_only: bool,
) -> DavResult<Response> {
    let view = state.view_for(uid).await?;
    let meta = view.stat(user_path).await?;
    if matches!(meta.kind, FileKind::Directory) {
        return Err(DavError::BadRequest("GET on a directory".into()));
    }
    let etag = format!("\"{}\"", meta.etag.as_str());
    let last_mod = httpdate::fmt_http_date(meta.mtime);

    // Range handling.
    let range = parse_range(headers, meta.size)?;
    let (status, content_length, content_range, body) = match range {
        None => {
            let body = if head_only {
                Body::empty()
            } else {
                let reader = view.read(user_path).await?;
                Body::from_stream(ReaderStream::new(reader))
            };
            (StatusCode::OK, meta.size, None, body)
        }
        Some(r) => {
            let length = r.end - r.start;
            let cr = format!("bytes {}-{}/{}", r.start, r.end - 1, meta.size);
            let body = if head_only {
                Body::empty()
            } else {
                let reader = view.read_range(user_path, r).await?;
                Body::from_stream(ReaderStream::new(reader))
            };
            (StatusCode::PARTIAL_CONTENT, length, Some(cr), body)
        }
    };

    let mut resp = Response::builder()
        .status(status)
        .header(header::CONTENT_LENGTH, content_length.to_string())
        .header(header::CONTENT_TYPE, meta.mimetype.as_str())
        .header(header::ETAG, etag)
        .header(header::LAST_MODIFIED, last_mod)
        .header(header::ACCEPT_RANGES, "bytes");
    if let Some(cr) = content_range {
        resp = resp.header(header::CONTENT_RANGE, cr);
    }
    resp.body(body)
        .map_err(|e| DavError::Internal(format!("response build: {e}")))
}

async fn put(
    state: AppState,
    uid: &crabcloud_users::UserId,
    user_path: &crabcloud_fs::UserPath,
    headers: &HeaderMap,
    body: Body,
) -> DavResult<Response> {
    let view = state.view_for(uid).await?;

    // Conditional checks: resolve target IF needed.
    let if_match = parse_if_match(headers);
    let if_none_match_star = parse_if_none_match_wildcard(headers);
    let existing = view.stat(user_path).await.ok();
    match (&if_match, &existing) {
        (IfMatch::Wildcard, None) => return Err(DavError::PreconditionFailed),
        (IfMatch::Etag(want), Some(meta)) if meta.etag.as_str() != want => {
            return Err(DavError::PreconditionFailed);
        }
        (IfMatch::Etag(_), None) => return Err(DavError::PreconditionFailed),
        _ => {}
    }
    if if_none_match_star && existing.is_some() {
        return Err(DavError::PreconditionFailed);
    }

    let stream = body
        .into_data_stream()
        .map(|r| r.map_err(std::io::Error::other));
    let body_reader = tokio_util::io::StreamReader::new(stream);
    let pinned: std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>> = Box::pin(body_reader);

    let meta = view.put_file(user_path, pinned).await?;
    let status = if existing.is_some() {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::CREATED
    };
    let etag = format!("\"{}\"", meta.etag.as_str());
    let last_modified = httpdate::fmt_http_date(meta.mtime);
    Ok((
        status,
        [(header::ETAG, etag), (header::LAST_MODIFIED, last_modified)],
        "",
    )
        .into_response())
}

async fn mkcol(
    state: AppState,
    uid: &crabcloud_users::UserId,
    user_path: &crabcloud_fs::UserPath,
) -> DavResult<Response> {
    let view = state.view_for(uid).await?;
    view.mkdir(user_path).await?;
    Ok((StatusCode::CREATED, "").into_response())
}

async fn delete(
    state: AppState,
    uid: &crabcloud_users::UserId,
    user_path: &crabcloud_fs::UserPath,
) -> DavResult<Response> {
    let view = state.view_for(uid).await?;
    view.delete(user_path).await?;
    Ok((StatusCode::NO_CONTENT, "").into_response())
}
