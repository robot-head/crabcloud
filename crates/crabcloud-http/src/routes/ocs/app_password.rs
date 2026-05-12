//! `GET /ocs/v2.php/core/getapppassword` (Session-only) — mints a fresh
//! `AppPassword` token bound to the current uid, intended for use as a
//! browser→DAV bridge (the browser passes the result on to the WebDAV client
//! in the same session).
//!
//! `DELETE /ocs/v2.php/core/apppassword` (any auth) — revokes the current
//! request's own token row. Idempotent: errors from the underlying store
//! collapse to a 200 so clients aren't punished for stale state.

use crate::auth_context::{AuthContext, AuthMethod};
use crate::extractors::format::OcsFormat;
use crate::OcsError;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use crabcloud_core::{AppState, Error as CoreError};
use crabcloud_ocs::{render, OcsResponse, OcsVersion};
use crabcloud_users::AuthTokenType;
use serde::Serialize;

#[derive(Debug, Serialize)]
struct AppPasswordPayload {
    apppassword: String,
}

fn unauth(fmt: crabcloud_ocs::Format) -> OcsError {
    OcsError::new(CoreError::Unauthorized, OcsVersion::V2, fmt)
}

fn forbidden(fmt: crabcloud_ocs::Format) -> OcsError {
    OcsError::new(CoreError::Forbidden, OcsVersion::V2, fmt)
}

/// `GET /ocs/v2.php/core/getapppassword` — Session-only. Bearer/Basic
/// callers get 403 so a desktop client can't promote one app password into
/// a second one without going through the browser.
pub async fn get_app_password(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    fmt: OcsFormat,
) -> Result<Response, OcsError> {
    if ctx.method != AuthMethod::Session {
        return Err(forbidden(fmt.0));
    }
    let ap = state
        .users
        .app_passwords()
        .ok_or_else(|| unauth(fmt.0))?
        .clone();
    let (_row, raw) = ap
        .mint(
            &ctx.user_id,
            &ctx.login_name,
            "Browser bridge",
            AuthTokenType::AppPassword,
            false,
        )
        .await
        .map_err(|e| OcsError::new(CoreError::Users(e), OcsVersion::V2, fmt.0))?;

    let payload = AppPasswordPayload {
        apppassword: raw.expose().to_string(),
    };
    let envelope = OcsResponse::ok(payload, OcsVersion::V2);
    let (body, ct) = render(&envelope, fmt.0);
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
    Ok((StatusCode::OK, headers, body).into_response())
}

/// `DELETE /ocs/v2.php/core/apppassword` — revokes the token row that
/// authenticated the current request. Works from any auth method (a desktop
/// client calling DELETE with its own Bearer token disconnects itself).
pub async fn delete_app_password(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    fmt: OcsFormat,
) -> Result<Response, OcsError> {
    let ap = state
        .users
        .app_passwords()
        .ok_or_else(|| unauth(fmt.0))?
        .clone();
    let _ = ap.revoke(ctx.token_id).await;
    let envelope = OcsResponse::ok(serde_json::json!({}), OcsVersion::V2);
    let (body, ct) = render(&envelope, fmt.0);
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
    Ok((StatusCode::OK, headers, body).into_response())
}
