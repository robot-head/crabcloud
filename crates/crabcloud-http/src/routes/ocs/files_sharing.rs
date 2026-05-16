//! OCS `apps/files_sharing/api/v1/` endpoints — user + group share CRUD.
//!
//! Nextcloud's wire shapes (spec §7). All five handlers stay in this module
//! and share the local `share_to_json`, `ocs_envelope`, and `from_share_error`
//! helpers. The handlers compute the requester's home storage id via
//! `state.storage_factory` (the `Shares` service is storage-agnostic and
//! takes the id as a string).

use crate::auth_context::AuthContext;
use crate::extractors::format::OcsFormat;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Form};
use chrono::NaiveDate;
use crabcloud_core::AppState;
use crabcloud_ocs::{render, Format, OcsResponse, OcsStatus, OcsVersion};
use crabcloud_sharing::{CreateShareRequest, ShareError, ShareRow, ShareType, UpdateShareFields};
use crabcloud_users::UserId;
use serde::Deserialize;
use serde_json::Value;

pub fn router() -> axum::Router<AppState> {
    axum::Router::new()
        .route("/shares", post(create_handler).get(list_handler))
        .route(
            "/shares/{id}",
            get(get_handler).put(update_handler).delete(delete_handler),
        )
}

// --- envelope helpers ------------------------------------------------------

fn http_status_from(code: u16) -> StatusCode {
    StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
}

fn ocs_status_for_http(code: u16) -> OcsStatus {
    match code {
        200 => OcsStatus::Ok,
        201 => OcsStatus::Created,
        400 => OcsStatus::BadRequest,
        401 => OcsStatus::Unauthorized,
        403 => OcsStatus::Forbidden,
        // 404 → Nextcloud's NotFound (998). Everything else we don't have
        // a dedicated OcsStatus for (501, 5xx, anything custom) falls
        // through to UnknownError (999); the HTTP code itself remains
        // distinct on the response line so clients can branch on it.
        404 => OcsStatus::NotFound,
        _ => OcsStatus::UnknownError,
    }
}

/// Wrap `data` in `{ ocs: { meta, data } }` (or XML equivalent). HTTP status
/// is `code`; OCS-envelope `statuscode` mirrors it via `OcsStatus`.
fn ocs_envelope(code: u16, message: &str, data: Value, fmt: Format) -> Response {
    let status = ocs_status_for_http(code);
    let envelope = OcsResponse {
        status,
        message: message.to_string(),
        data,
        version: OcsVersion::V2,
    };
    let (body, ct) = render(&envelope, fmt);
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
    (http_status_from(code), headers, body).into_response()
}

fn from_share_error(err: ShareError, fmt: Format) -> Response {
    ocs_envelope(err.http_status(), &err.to_string(), Value::Null, fmt)
}

// --- share -> wire JSON ----------------------------------------------------

async fn display_name_of(state: &AppState, raw_uid: &str) -> String {
    let Ok(uid) = UserId::new(raw_uid) else {
        return raw_uid.to_string();
    };
    match state.users.user_store().lookup(&uid).await {
        Ok(Some(u)) if !u.display_name.is_empty() => u.display_name,
        _ => raw_uid.to_string(),
    }
}

async fn group_display_name_of(state: &AppState, raw_gid: &str) -> String {
    let Ok(gid) = crabcloud_users::GroupId::new(raw_gid) else {
        return raw_gid.to_string();
    };
    match state.users.group_store().lookup(&gid).await {
        Ok(Some(g)) if !g.display_name.is_empty() => g.display_name,
        _ => raw_gid.to_string(),
    }
}

async fn share_to_json(row: &ShareRow, state: &AppState) -> Value {
    // For Link rows, `share_with` is `None` and `share_with_displayname` is
    // Nextcloud's literal `"(Public link)"`. We deliberately do NOT surface
    // the password hash or plaintext on the wire — clients only learn
    // whether a password is set via the `share_with` masking convention
    // (Nextcloud puts the bcrypt hash there in upstream PHP; we follow the
    // safer "null + boolean" shape so handlers cannot accidentally leak).
    let (share_with_value, share_with_displayname) = match row.share_type {
        ShareType::Link => (Value::Null, "(Public link)".to_string()),
        ShareType::Group => {
            let dn = match row.share_with.as_deref() {
                Some(gid) => group_display_name_of(state, gid).await,
                None => String::new(),
            };
            (
                Value::String(row.share_with.clone().unwrap_or_default()),
                dn,
            )
        }
        ShareType::User => {
            let dn = match row.share_with.as_deref() {
                Some(s) => display_name_of(state, s).await,
                None => String::new(),
            };
            (
                Value::String(row.share_with.clone().unwrap_or_default()),
                dn,
            )
        }
    };
    let displayname_owner = display_name_of(state, &row.uid_owner).await;
    let storage_id = match UserId::new(&row.uid_owner) {
        Ok(uid) => match state.storage_factory.home_storage(&uid).await {
            Ok(s) => s.id().to_string(),
            Err(_) => String::new(),
        },
        Err(_) => String::new(),
    };
    let share_type_int: i16 = row.share_type.into();
    let url = row
        .token
        .as_deref()
        .map(|t| Value::String(format!("/s/{t}")))
        .unwrap_or(Value::Null);
    serde_json::json!({
        "id": row.id.to_string(),
        "share_type": share_type_int,
        "share_with": share_with_value,
        "share_with_displayname": share_with_displayname,
        "uid_owner": row.uid_owner,
        "uid_initiator": row.uid_initiator,
        "displayname_owner": displayname_owner,
        "item_type": row.item_type.as_db_str(),
        "item_source": row.item_source,
        "file_source": row.file_source,
        "file_target": row.file_target,
        "path": row.file_target,
        "permissions": row.permissions.as_u32(),
        "stime": row.stime,
        "expiration": row
            .expiration
            .map(|t| t.naive_utc().date().format("%Y-%m-%d").to_string()),
        "token": row.token,
        "url": url,
        "password_set": row.password_hash.is_some(),
        "parent": row.parent,
        "storage_id": storage_id,
        "mail_send": 0,
    })
}

// --- POST /shares ----------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CreateShareForm {
    path: String,
    #[serde(rename = "shareType")]
    share_type: i16,
    #[serde(rename = "shareWith")]
    share_with: Option<String>,
    permissions: Option<u32>,
    password: Option<String>,
    #[serde(rename = "expireDate")]
    expire_date: Option<String>,
}

async fn create_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    fmt: OcsFormat,
    Form(form): Form<CreateShareForm>,
) -> Response {
    let st = match ShareType::try_from(form.share_type) {
        Ok(st) => st,
        Err(_) => return from_share_error(ShareError::InvalidShareType, fmt.0),
    };
    let home_sid = match state.storage_factory.home_storage(&ctx.user_id).await {
        Ok(s) => s.id().to_string(),
        Err(_) => return from_share_error(ShareError::PathNotOwned, fmt.0),
    };
    // Link-only: default permissions to 1 (read) per Nextcloud's OCS contract.
    // User/group shares fall through to the service's existing
    // `permissions & 1 == 0 -> BadPermissions` gate when omitted.
    let permissions = form.permissions.unwrap_or(1);
    // Parse expireDate up-front so a malformed value short-circuits to 400
    // without hitting the service. Empty string on create is treated as
    // "no expiration" (same as omitted) — the create path has no notion of
    // "clear", since the row is new.
    let expire_date = match form.expire_date.as_deref() {
        None | Some("") => None,
        Some(s) => match NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            Ok(d) => Some(d),
            Err(_) => {
                return from_share_error(
                    ShareError::InvalidExpireDate("expected YYYY-MM-DD"),
                    fmt.0,
                )
            }
        },
    };
    // Empty password string on create is the same as "no password".
    let password = match form.password.as_deref() {
        None | Some("") => None,
        Some(s) => Some(s.to_string()),
    };
    let req = CreateShareRequest {
        requester: ctx.user_id.as_str().to_string(),
        home_storage_id: home_sid,
        path: form.path,
        share_type: st,
        share_with: form.share_with.unwrap_or_default(),
        permissions,
        password,
        expire_date,
    };
    match state.shares.create(req).await {
        Ok(row) => {
            let data = share_to_json(&row, &state).await;
            ocs_envelope(200, "OK", data, fmt.0)
        }
        Err(e) => from_share_error(e, fmt.0),
    }
}

// --- GET /shares (list) ----------------------------------------------------

#[derive(Debug, Deserialize)]
struct ListQuery {
    path: Option<String>,
    shared_with_me: Option<bool>,
    subfiles: Option<bool>,
}

async fn list_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    fmt: OcsFormat,
    Query(q): Query<ListQuery>,
) -> Response {
    if q.subfiles.unwrap_or(false) {
        return from_share_error(ShareError::NotImplemented, fmt.0);
    }
    let rows = if let Some(path) = q.path {
        let home_sid = match state.storage_factory.home_storage(&ctx.user_id).await {
            Ok(s) => s.id().to_string(),
            Err(_) => return from_share_error(ShareError::PathNotOwned, fmt.0),
        };
        state
            .shares
            .list_for_owner_path(&ctx.user_id, &home_sid, &path)
            .await
    } else if q.shared_with_me.unwrap_or(false) {
        state.shares.list_incoming(&ctx.user_id).await
    } else {
        state.shares.list_outgoing(&ctx.user_id).await
    };
    match rows {
        Ok(rs) => {
            let mut out = Vec::with_capacity(rs.len());
            for r in &rs {
                out.push(share_to_json(r, &state).await);
            }
            ocs_envelope(200, "OK", Value::Array(out), fmt.0)
        }
        Err(e) => from_share_error(e, fmt.0),
    }
}

// --- GET /shares/{id} ------------------------------------------------------

async fn get_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    fmt: OcsFormat,
    Path(id): Path<i64>,
) -> Response {
    let row = match state.shares.get(id).await {
        Ok(Some(r)) => r,
        Ok(None) => return from_share_error(ShareError::NotFound, fmt.0),
        Err(e) => return from_share_error(e, fmt.0),
    };
    let requester = ctx.user_id.as_str();
    let is_owner = row.uid_owner == requester;
    let is_direct = matches!(
        (&row.share_type, row.share_with.as_deref()),
        (ShareType::User, Some(s)) if s == requester
    );
    let is_group_recipient =
        if let (ShareType::Group, Some(gname)) = (&row.share_type, row.share_with.as_deref()) {
            match state.users.groups_of(&ctx.user_id).await {
                Ok(groups) => groups.iter().any(|g| g.as_str() == gname),
                Err(_) => false,
            }
        } else {
            false
        };
    let is_admin = state.users.is_admin(&ctx.user_id).await.unwrap_or(false);
    // 404 (not 403) on unauthorized — Nextcloud avoids leaking existence.
    if !(is_owner || is_direct || is_group_recipient || is_admin) {
        return from_share_error(ShareError::NotFound, fmt.0);
    }
    let data = share_to_json(&row, &state).await;
    ocs_envelope(200, "OK", data, fmt.0)
}

// --- PUT /shares/{id} ------------------------------------------------------

#[derive(Debug, Deserialize)]
struct UpdateShareForm {
    permissions: Option<u32>,
    #[serde(rename = "expireDate")]
    expire_date: Option<String>,
    password: Option<String>,
    note: Option<String>,
}

async fn update_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    fmt: OcsFormat,
    Path(id): Path<i64>,
    Form(form): Form<UpdateShareForm>,
) -> Response {
    // expireDate tristate:
    //   field absent           -> None         (don't touch)
    //   present, empty string  -> Some(None)   (clear expiration)
    //   present, valid date    -> Some(Some(d))
    //   present, malformed     -> 400 InvalidExpireDate
    let expire = match form.expire_date.as_deref() {
        None => None,
        Some("") => Some(None),
        Some(s) => match NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            Ok(d) => Some(Some(d)),
            Err(_) => {
                return from_share_error(
                    ShareError::InvalidExpireDate("expected YYYY-MM-DD"),
                    fmt.0,
                )
            }
        },
    };
    // password tristate — same shape as expireDate. Empty string clears.
    let password = match form.password.as_deref() {
        None => None,
        Some("") => Some(None),
        Some(s) => Some(Some(s.to_string())),
    };
    let fields = UpdateShareFields {
        permissions: form.permissions,
        expire_date: expire,
        password,
        note: form.note,
    };
    match state.shares.update(id, &ctx.user_id, fields).await {
        Ok(row) => {
            let data = share_to_json(&row, &state).await;
            ocs_envelope(200, "OK", data, fmt.0)
        }
        Err(e) => from_share_error(e, fmt.0),
    }
}

// --- DELETE /shares/{id} ---------------------------------------------------

async fn delete_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    fmt: OcsFormat,
    Path(id): Path<i64>,
) -> Response {
    match state.shares.delete(id, &ctx.user_id).await {
        Ok(()) => ocs_envelope(200, "OK", Value::Null, fmt.0),
        Err(e) => from_share_error(e, fmt.0),
    }
}
