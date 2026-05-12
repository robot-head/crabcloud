//! Admin user-administration endpoints under `/ocs/v2.php/cloud/users`.
//!
//! All handlers gated by the [`AdminUser`] extractor (401 anonymous, 403
//! non-admin). Self-action guards on delete/disable/password-rotation
//! prevent the calling admin from accidentally locking themselves out.
//! Structural last-admin guards prevent removing the final admin.

use crate::extractors::auth::AdminUser;
use crate::extractors::format::OcsFormat;
use crate::OcsError;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Form;
use crabcloud_core::{AppState, Error as CoreError};
use crabcloud_ocs::{render, OcsResponse, OcsVersion};
use crabcloud_users::{Email, GroupId, User, UserId, UserListFilter, UsersError};
use serde::{Deserialize, Serialize};

// --- shared helpers ---------------------------------------------------------

fn ocs_ok<T: Serialize>(payload: T, fmt: crabcloud_ocs::Format) -> Response {
    let envelope = OcsResponse::ok(payload, OcsVersion::V2);
    let (body, ct) = render(&envelope, fmt);
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
    (StatusCode::OK, headers, body).into_response()
}

fn users_err(e: UsersError, fmt: crabcloud_ocs::Format) -> OcsError {
    OcsError::new(CoreError::Users(e), OcsVersion::V2, fmt)
}

fn not_found(fmt: crabcloud_ocs::Format) -> OcsError {
    OcsError::new(CoreError::NotFound, OcsVersion::V2, fmt)
}

fn bad_request(msg: impl Into<String>, fmt: crabcloud_ocs::Format) -> OcsError {
    OcsError::new(CoreError::BadRequest(msg.into()), OcsVersion::V2, fmt)
}

/// Lookup-then-fail-with-404 helper. Returns `Ok(())` if the row exists,
/// `Err(NotFound)` otherwise. Used as the first line of every `{uid}`-path
/// handler to keep the bootstrap virtual admin invisible.
async fn require_real_user(
    state: &AppState,
    uid: &UserId,
    fmt: crabcloud_ocs::Format,
) -> Result<(), OcsError> {
    let exists = state
        .users
        .user_store()
        .exists_in_storage(uid)
        .await
        .map_err(|e| users_err(e, fmt))?;
    if !exists {
        return Err(not_found(fmt));
    }
    Ok(())
}

/// Returns `Ok(())` if `uid` isn't the only member of the `admin` group.
async fn require_not_last_admin(
    state: &AppState,
    uid: &UserId,
    fmt: crabcloud_ocs::Format,
) -> Result<(), OcsError> {
    let admin_gid = GroupId::new("admin").map_err(|e| users_err(e, fmt))?;
    let admins = state
        .users
        .group_store()
        .members_of(&admin_gid)
        .await
        .map_err(|e| users_err(e, fmt))?;
    if admins.len() == 1 && admins[0] == *uid {
        return Err(bad_request("at least one admin must remain", fmt));
    }
    Ok(())
}

// --- list (GET /cloud/users) ------------------------------------------------

const DEFAULT_LIMIT: u32 = 100;
const MAX_LIMIT: u32 = 500;

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
}
fn default_limit() -> u32 {
    DEFAULT_LIMIT
}

#[derive(Debug, Serialize)]
struct ListPayload {
    users: Vec<String>,
}

pub async fn list_users(
    State(state): State<AppState>,
    _admin: AdminUser,
    fmt: OcsFormat,
    Query(q): Query<ListQuery>,
) -> Result<Response, OcsError> {
    let limit = q.limit.clamp(1, MAX_LIMIT);
    let filter = UserListFilter {
        search: q.search.as_deref().filter(|s| !s.is_empty()),
        limit,
        offset: q.offset,
    };
    let rows = state
        .users
        .user_store()
        .list_users(filter)
        .await
        .map_err(|e| users_err(e, fmt.0))?;
    Ok(ocs_ok(
        ListPayload {
            users: rows.into_iter().map(|u| u.uid.into_inner()).collect(),
        },
        fmt.0,
    ))
}

// --- get_user (GET /cloud/users/{uid}) --------------------------------------

#[derive(Debug, Serialize)]
struct UserPayload {
    id: String,
    #[serde(rename = "display-name")]
    display_name: String,
    email: Option<String>,
    groups: Vec<String>,
    enabled: bool,
    #[serde(rename = "last-login")]
    last_login: u64,
}

pub async fn get_user(
    State(state): State<AppState>,
    _admin: AdminUser,
    fmt: OcsFormat,
    Path(uid): Path<String>,
) -> Result<Response, OcsError> {
    let uid = UserId::new(&uid).map_err(|e| users_err(e, fmt.0))?;
    require_real_user(&state, &uid, fmt.0).await?;
    let user = state
        .users
        .user_store()
        .lookup(&uid)
        .await
        .map_err(|e| users_err(e, fmt.0))?
        .ok_or_else(|| not_found(fmt.0))?;
    let groups = state
        .users
        .group_store()
        .groups_of(&uid)
        .await
        .map_err(|e| users_err(e, fmt.0))?;
    Ok(ocs_ok(
        UserPayload {
            id: user.uid.into_inner(),
            display_name: user.display_name,
            email: user.email.map(|e| e.as_str().to_string()),
            groups: groups.into_iter().map(|g| g.as_str().to_string()).collect(),
            enabled: user.enabled,
            last_login: user.last_seen,
        },
        fmt.0,
    ))
}

// --- create_user (POST /cloud/users) ----------------------------------------

/// Minimal `application/x-www-form-urlencoded` percent-decode.
/// Translates `+` to space, `%HH` to the corresponding byte, and replaces any
/// invalid UTF-8 with the replacement char (the decoded bytes only need to be
/// usable as a String — invalid input is treated as user error downstream).
fn pct_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'+' {
            out.push(b' ');
            i += 1;
        } else if b == b'%' && i + 2 < bytes.len() {
            let h = (bytes[i + 1] as char).to_digit(16);
            let l = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (h, l) {
                out.push(((h << 4) | l) as u8);
                i += 3;
            } else {
                out.push(b);
                i += 1;
            }
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Manual decode of the create-user form body.
///
/// `axum::Form`'s `serde_urlencoded` backend does NOT support `Vec<T>` from
/// repeated keys, so we hand-parse to collect zero or more `groups[]=<gid>`
/// entries alongside the scalar fields.
fn parse_create_user_body(body: &str) -> Result<CreateUserForm, String> {
    let mut userid: Option<String> = None;
    let mut password: Option<String> = None;
    let mut email: Option<String> = None;
    let mut display_name: Option<String> = None;
    let mut groups: Vec<String> = Vec::new();
    // Walk the body ourselves so repeated `groups[]=` keys are preserved.
    // serde_urlencoded (axum's default Form backend) collapses duplicates
    // into a single value, which collides with Nextcloud's array syntax.
    for raw_pair in body.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = match raw_pair.split_once('=') {
            Some((k, v)) => (k, v),
            None => (raw_pair, ""),
        };
        let k = pct_decode(k);
        let v = pct_decode(v);
        match k.as_str() {
            "userid" => userid = Some(v),
            "password" => password = Some(v),
            "email" => email = Some(v),
            "displayName" => display_name = Some(v),
            "groups[]" => groups.push(v),
            _ => {}
        }
    }
    Ok(CreateUserForm {
        userid: userid.ok_or_else(|| "missing userid".to_string())?,
        password: password.ok_or_else(|| "missing password".to_string())?,
        email,
        display_name,
        groups,
    })
}

#[derive(Debug)]
pub struct CreateUserForm {
    pub userid: String,
    pub password: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub groups: Vec<String>,
}

#[derive(Debug, Serialize)]
struct CreateUserPayload {
    id: String,
}

pub async fn create_user(
    State(state): State<AppState>,
    admin: AdminUser,
    fmt: OcsFormat,
    body: String,
) -> Result<Response, OcsError> {
    let form = parse_create_user_body(&body).map_err(|m| bad_request(m, fmt.0))?;
    let uid = UserId::new(&form.userid).map_err(|e| users_err(e, fmt.0))?;

    // Validate groups exist (resolve-before-write to avoid partial creates).
    let mut group_ids: Vec<GroupId> = Vec::with_capacity(form.groups.len());
    for raw in &form.groups {
        let gid = GroupId::new(raw).map_err(|e| users_err(e, fmt.0))?;
        let exists = state
            .users
            .group_store()
            .lookup(&gid)
            .await
            .map_err(|e| users_err(e, fmt.0))?
            .is_some();
        if !exists {
            return Err(bad_request(format!("unknown group: {raw}"), fmt.0));
        }
        group_ids.push(gid);
    }

    let email = match form.email.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => Some(Email::parse(s).map_err(|e| users_err(e, fmt.0))?),
        None => None,
    };
    let display_name = form
        .display_name
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| form.userid.clone());
    let hash = state
        .users
        .verifier()
        .hash(&form.password)
        .map_err(|e| users_err(e, fmt.0))?;

    let new_user = User {
        uid: uid.clone(),
        display_name,
        email,
        enabled: true,
        last_seen: 0,
    };
    state
        .users
        .user_store()
        .create(&new_user, Some(&hash))
        .await
        .map_err(|e| users_err(e, fmt.0))?;
    for gid in &group_ids {
        state
            .users
            .group_store()
            .add_to_group(&uid, gid)
            .await
            .map_err(|e| users_err(e, fmt.0))?;
    }

    ::tracing::info!(
        actor = %admin.0.user_id,
        action = "create_user",
        target_uid = %uid,
        "admin OCS write"
    );
    Ok(ocs_ok(
        CreateUserPayload {
            id: uid.into_inner(),
        },
        fmt.0,
    ))
}

// --- edit_user (PUT /cloud/users/{uid}) -------------------------------------

#[derive(Debug, Deserialize)]
pub struct EditUserForm {
    pub key: String,
    pub value: String,
}

pub async fn edit_user(
    State(state): State<AppState>,
    admin: AdminUser,
    fmt: OcsFormat,
    Path(uid): Path<String>,
    Form(form): Form<EditUserForm>,
) -> Result<Response, OcsError> {
    let uid = UserId::new(&uid).map_err(|e| users_err(e, fmt.0))?;
    require_real_user(&state, &uid, fmt.0).await?;

    match form.key.as_str() {
        "password" => {
            if uid.as_str() == admin.0.user_id {
                return Err(bad_request(
                    "use the self-service PUT /cloud/user endpoint to rotate your own password",
                    fmt.0,
                ));
            }
            state
                .users
                .set_password(&uid, &form.value)
                .await
                .map_err(|e| users_err(e, fmt.0))?;
            ::tracing::info!(
                actor = %admin.0.user_id,
                action = "set_password",
                target_uid = %uid,
                "admin OCS write"
            );
        }
        "displayname" => {
            state
                .users
                .user_store()
                .set_display_name(&uid, &form.value)
                .await
                .map_err(|e| users_err(e, fmt.0))?;
            ::tracing::info!(
                actor = %admin.0.user_id,
                action = "set_display_name",
                target_uid = %uid,
                "admin OCS write"
            );
        }
        "email" => {
            let new = if form.value.is_empty() {
                None
            } else {
                Some(form.value.as_str())
            };
            state
                .users
                .user_store()
                .set_email(&uid, new)
                .await
                .map_err(|e| users_err(e, fmt.0))?;
            ::tracing::info!(
                actor = %admin.0.user_id,
                action = "set_email",
                target_uid = %uid,
                "admin OCS write"
            );
        }
        other => {
            return Err(bad_request(format!("unknown key: {other}"), fmt.0));
        }
    }

    Ok(ocs_ok(serde_json::json!({}), fmt.0))
}

// --- delete_user (DELETE /cloud/users/{uid}) --------------------------------

pub async fn delete_user(
    State(state): State<AppState>,
    admin: AdminUser,
    fmt: OcsFormat,
    Path(uid): Path<String>,
) -> Result<Response, OcsError> {
    let uid = UserId::new(&uid).map_err(|e| users_err(e, fmt.0))?;
    require_real_user(&state, &uid, fmt.0).await?;
    if uid.as_str() == admin.0.user_id {
        return Err(bad_request("cannot delete the calling admin", fmt.0));
    }
    require_not_last_admin(&state, &uid, fmt.0).await?;
    state
        .users
        .delete_user(&uid)
        .await
        .map_err(|e| users_err(e, fmt.0))?;
    ::tracing::info!(
        actor = %admin.0.user_id,
        action = "delete_user",
        target_uid = %uid,
        "admin OCS write"
    );
    Ok(ocs_ok(serde_json::json!({}), fmt.0))
}

// --- enable_user / disable_user (PUT /cloud/users/{uid}/{enable,disable}) ---

pub async fn enable_user(
    State(state): State<AppState>,
    admin: AdminUser,
    fmt: OcsFormat,
    Path(uid): Path<String>,
) -> Result<Response, OcsError> {
    let uid = UserId::new(&uid).map_err(|e| users_err(e, fmt.0))?;
    require_real_user(&state, &uid, fmt.0).await?;
    state
        .users
        .user_store()
        .set_enabled(&uid, true)
        .await
        .map_err(|e| users_err(e, fmt.0))?;
    ::tracing::info!(
        actor = %admin.0.user_id,
        action = "enable_user",
        target_uid = %uid,
        "admin OCS write"
    );
    Ok(ocs_ok(serde_json::json!({}), fmt.0))
}

pub async fn disable_user(
    State(state): State<AppState>,
    admin: AdminUser,
    fmt: OcsFormat,
    Path(uid): Path<String>,
) -> Result<Response, OcsError> {
    let uid = UserId::new(&uid).map_err(|e| users_err(e, fmt.0))?;
    require_real_user(&state, &uid, fmt.0).await?;
    if uid.as_str() == admin.0.user_id {
        return Err(bad_request("cannot disable the calling admin", fmt.0));
    }
    require_not_last_admin(&state, &uid, fmt.0).await?;
    state
        .users
        .disable_user(&uid)
        .await
        .map_err(|e| users_err(e, fmt.0))?;
    ::tracing::info!(
        actor = %admin.0.user_id,
        action = "disable_user",
        target_uid = %uid,
        "admin OCS write"
    );
    Ok(ocs_ok(serde_json::json!({}), fmt.0))
}

// --- user-groups sub-resource (GET/POST/DELETE /cloud/users/{uid}/groups) ---

#[derive(Debug, Serialize)]
struct UserGroupsPayload {
    groups: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct AddGroupForm {
    pub groupid: String,
}

#[derive(Debug, Deserialize)]
pub struct RemoveGroupQuery {
    pub groupid: String,
}

pub async fn list_user_groups(
    State(state): State<AppState>,
    _admin: AdminUser,
    fmt: OcsFormat,
    Path(uid): Path<String>,
) -> Result<Response, OcsError> {
    let uid = UserId::new(&uid).map_err(|e| users_err(e, fmt.0))?;
    require_real_user(&state, &uid, fmt.0).await?;
    let groups = state
        .users
        .group_store()
        .groups_of(&uid)
        .await
        .map_err(|e| users_err(e, fmt.0))?;
    Ok(ocs_ok(
        UserGroupsPayload {
            groups: groups.into_iter().map(|g| g.as_str().to_string()).collect(),
        },
        fmt.0,
    ))
}

pub async fn add_user_to_group(
    State(state): State<AppState>,
    admin: AdminUser,
    fmt: OcsFormat,
    Path(uid): Path<String>,
    Form(form): Form<AddGroupForm>,
) -> Result<Response, OcsError> {
    let uid = UserId::new(&uid).map_err(|e| users_err(e, fmt.0))?;
    require_real_user(&state, &uid, fmt.0).await?;
    let gid = GroupId::new(&form.groupid).map_err(|e| users_err(e, fmt.0))?;
    let exists = state
        .users
        .group_store()
        .lookup(&gid)
        .await
        .map_err(|e| users_err(e, fmt.0))?
        .is_some();
    if !exists {
        return Err(bad_request(
            format!("unknown group: {}", form.groupid),
            fmt.0,
        ));
    }
    state
        .users
        .group_store()
        .add_to_group(&uid, &gid)
        .await
        .map_err(|e| users_err(e, fmt.0))?;
    ::tracing::info!(
        actor = %admin.0.user_id,
        action = "add_to_group",
        target_uid = %uid,
        target_gid = %gid,
        "admin OCS write"
    );
    Ok(ocs_ok(serde_json::json!({}), fmt.0))
}

pub async fn remove_user_from_group(
    State(state): State<AppState>,
    admin: AdminUser,
    fmt: OcsFormat,
    Path(uid): Path<String>,
    Query(q): Query<RemoveGroupQuery>,
) -> Result<Response, OcsError> {
    let uid = UserId::new(&uid).map_err(|e| users_err(e, fmt.0))?;
    require_real_user(&state, &uid, fmt.0).await?;
    let gid = GroupId::new(&q.groupid).map_err(|e| users_err(e, fmt.0))?;
    let exists = state
        .users
        .group_store()
        .lookup(&gid)
        .await
        .map_err(|e| users_err(e, fmt.0))?
        .is_some();
    if !exists {
        return Err(bad_request(format!("unknown group: {}", q.groupid), fmt.0));
    }
    if gid.as_str() == "admin" {
        if uid.as_str() == admin.0.user_id {
            return Err(bad_request(
                "cannot remove the calling admin from the admin group",
                fmt.0,
            ));
        }
        require_not_last_admin(&state, &uid, fmt.0).await?;
    }
    state
        .users
        .group_store()
        .remove_from_group(&uid, &gid)
        .await
        .map_err(|e| users_err(e, fmt.0))?;
    ::tracing::info!(
        actor = %admin.0.user_id,
        action = "remove_from_group",
        target_uid = %uid,
        target_gid = %gid,
        "admin OCS write"
    );
    Ok(ocs_ok(serde_json::json!({}), fmt.0))
}

#[cfg(test)]
mod tests {
    use crate::router::build_router;
    use crate::session::{encode_cookie, COOKIE_NAME};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use crabcloud_config::test_support::minimal_sqlite_config;
    use crabcloud_core::{AppState, AppStateBuilder};
    use crabcloud_users::{
        AuthTokenType, BcryptVerifier, GroupId, GroupStore, PasswordVerifier, SqlGroupStore,
        User as UserRow, UserId,
    };
    use secrecy::ExposeSecret;
    use tempfile::tempdir;
    use tower::ServiceExt;

    async fn make_state(db_path: std::path::PathBuf) -> AppState {
        AppStateBuilder::new(minimal_sqlite_config(db_path))
            .build()
            .await
            .unwrap()
    }

    async fn seed_user(state: &AppState, uid: &str, password: &str, is_admin: bool) {
        let hash = BcryptVerifier::new().hash(password).unwrap();
        state
            .users
            .user_store()
            .create(
                &UserRow {
                    uid: UserId::new(uid).unwrap(),
                    display_name: format!("{uid} display"),
                    email: None,
                    enabled: true,
                    last_seen: 0,
                },
                Some(&hash),
            )
            .await
            .unwrap();
        if is_admin {
            let groups = SqlGroupStore::new(state.pool.clone());
            groups
                .add_to_group(&UserId::new(uid).unwrap(), &GroupId::new("admin").unwrap())
                .await
                .unwrap();
        }
    }

    async fn seed_login(state: &AppState, uid: &str) -> String {
        let ap = state.users.app_passwords().unwrap().clone();
        let (_row, raw) = ap
            .mint(
                &UserId::new(uid).unwrap(),
                uid,
                "test-session",
                AuthTokenType::Session,
                false,
            )
            .await
            .unwrap();
        let cookie_value =
            encode_cookie(raw.expose(), state.config.secret.expose_secret().as_bytes());
        format!("{COOKIE_NAME}={cookie_value}")
    }

    // --- list_users ---------------------------------------------------------

    #[tokio::test]
    async fn list_users_as_admin_returns_uids_sorted() {
        let dir = tempdir().unwrap();
        let state = make_state(dir.path().join("u.db")).await;
        seed_user(&state, "admin", "hunter2", true).await;
        seed_user(&state, "alice", "x", false).await;
        seed_user(&state, "bob", "x", false).await;
        let cookie = seed_login(&state, "admin").await;
        let app = build_router(state, axum::Router::new());

        let req = Request::builder()
            .uri("/ocs/v2.php/cloud/users?format=json")
            .header("ocs-apirequest", "true")
            .header("cookie", cookie)
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 16 * 1024)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let users = parsed["ocs"]["data"]["users"].as_array().unwrap();
        let uids: Vec<&str> = users.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(uids, vec!["admin", "alice", "bob"]);
    }

    #[tokio::test]
    async fn list_users_as_non_admin_returns_403() {
        let dir = tempdir().unwrap();
        let state = make_state(dir.path().join("u.db")).await;
        seed_user(&state, "alice", "x", false).await;
        let cookie = seed_login(&state, "alice").await;
        let app = build_router(state, axum::Router::new());

        let req = Request::builder()
            .uri("/ocs/v2.php/cloud/users?format=json")
            .header("ocs-apirequest", "true")
            .header("cookie", cookie)
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn list_users_anonymous_returns_401() {
        let dir = tempdir().unwrap();
        let state = make_state(dir.path().join("u.db")).await;
        let app = build_router(state, axum::Router::new());

        let req = Request::builder()
            .uri("/ocs/v2.php/cloud/users?format=json")
            .header("ocs-apirequest", "true")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // --- get_user -----------------------------------------------------------

    #[tokio::test]
    async fn get_user_returns_full_record() {
        let dir = tempdir().unwrap();
        let state = make_state(dir.path().join("u.db")).await;
        seed_user(&state, "admin", "hunter2", true).await;
        seed_user(&state, "alice", "x", false).await;
        let cookie = seed_login(&state, "admin").await;
        let app = build_router(state, axum::Router::new());

        let req = Request::builder()
            .uri("/ocs/v2.php/cloud/users/alice?format=json")
            .header("ocs-apirequest", "true")
            .header("cookie", cookie)
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 16 * 1024)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["ocs"]["data"]["id"], "alice");
        assert_eq!(parsed["ocs"]["data"]["enabled"], true);
    }

    #[tokio::test]
    async fn get_virtual_admin_returns_404() {
        // Build state with bootstrap_admin set; do NOT promote.
        let dir = tempdir().unwrap();
        let mut cfg = minimal_sqlite_config(dir.path().join("u.db"));
        let hash = BcryptVerifier::new().hash("bootpw").unwrap();
        cfg.bootstrap_admin = Some(crabcloud_config::BootstrapAdminConfig {
            username: "vadmin".into(),
            password_hash: hash,
        });
        let state = AppStateBuilder::new(cfg).build().await.unwrap();
        // Seed a separate real admin to drive the call.
        seed_user(&state, "admin", "hunter2", true).await;
        let cookie = seed_login(&state, "admin").await;
        let app = build_router(state, axum::Router::new());

        let req = Request::builder()
            .uri("/ocs/v2.php/cloud/users/vadmin?format=json")
            .header("ocs-apirequest", "true")
            .header("cookie", cookie)
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // --- create_user --------------------------------------------------------

    #[tokio::test]
    async fn create_user_with_valid_body_succeeds() {
        let dir = tempdir().unwrap();
        let state = make_state(dir.path().join("u.db")).await;
        seed_user(&state, "admin", "hunter2", true).await;
        let cookie = seed_login(&state, "admin").await;
        let app = build_router(state.clone(), axum::Router::new());

        let req = Request::builder()
            .method("POST")
            .uri("/ocs/v2.php/cloud/users?format=json")
            .header("ocs-apirequest", "true")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("cookie", cookie)
            .body(Body::from(
                "userid=newbie&password=newpass&displayName=Newbie",
            ))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let created = state
            .users
            .user_store()
            .lookup(&UserId::new("newbie").unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(created.display_name, "Newbie");
    }

    #[tokio::test]
    async fn create_user_with_unknown_group_returns_400_and_creates_nothing() {
        let dir = tempdir().unwrap();
        let state = make_state(dir.path().join("u.db")).await;
        seed_user(&state, "admin", "hunter2", true).await;
        let cookie = seed_login(&state, "admin").await;
        let app = build_router(state.clone(), axum::Router::new());

        let req = Request::builder()
            .method("POST")
            .uri("/ocs/v2.php/cloud/users?format=json")
            .header("ocs-apirequest", "true")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("cookie", cookie)
            .body(Body::from(
                "userid=newbie&password=newpass&groups%5B%5D=nope",
            ))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(state
            .users
            .user_store()
            .lookup(&UserId::new("newbie").unwrap())
            .await
            .unwrap()
            .is_none());
    }

    // --- delete_user --------------------------------------------------------

    #[tokio::test]
    async fn delete_user_cascades_tokens_and_memberships() {
        let dir = tempdir().unwrap();
        let state = make_state(dir.path().join("u.db")).await;
        seed_user(&state, "admin", "hunter2", true).await;
        seed_user(&state, "alice", "x", false).await;
        let ap = state.users.app_passwords().unwrap().clone();
        let (_row, raw) = ap
            .mint(
                &UserId::new("alice").unwrap(),
                "alice",
                "DAV",
                AuthTokenType::AppPassword,
                false,
            )
            .await
            .unwrap();
        let cookie = seed_login(&state, "admin").await;
        let app = build_router(state.clone(), axum::Router::new());

        let req = Request::builder()
            .method("DELETE")
            .uri("/ocs/v2.php/cloud/users/alice?format=json")
            .header("ocs-apirequest", "true")
            .header("cookie", cookie)
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        // user gone
        assert!(state
            .users
            .user_store()
            .lookup(&UserId::new("alice").unwrap())
            .await
            .unwrap()
            .is_none());
        // token revoked
        assert!(matches!(
            ap.verify(raw.expose()).await,
            Err(crabcloud_users::UsersError::TokenNotFound)
        ));
    }

    #[tokio::test]
    async fn delete_self_returns_400() {
        let dir = tempdir().unwrap();
        let state = make_state(dir.path().join("u.db")).await;
        seed_user(&state, "admin", "hunter2", true).await;
        let cookie = seed_login(&state, "admin").await;
        let app = build_router(state, axum::Router::new());

        let req = Request::builder()
            .method("DELETE")
            .uri("/ocs/v2.php/cloud/users/admin?format=json")
            .header("ocs-apirequest", "true")
            .header("cookie", cookie)
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // NOTE: `delete_last_admin_returns_400` is documented in the plan as not
    // reachable through ordinary admin flow at the HTTP level (the self-guard
    // fires before the last-admin guard when the sole admin tries to delete
    // themselves; a non-admin can't drive the call because AdminUser blocks).
    // The structural guard is exercised by the disable cascade tests in Batch B.

    // --- disable_user -------------------------------------------------------

    #[tokio::test]
    async fn disable_user_revokes_tokens() {
        let dir = tempdir().unwrap();
        let state = make_state(dir.path().join("u.db")).await;
        seed_user(&state, "admin", "hunter2", true).await;
        seed_user(&state, "alice", "x", false).await;
        let ap = state.users.app_passwords().unwrap().clone();
        let (_row, raw) = ap
            .mint(
                &UserId::new("alice").unwrap(),
                "alice",
                "DAV",
                AuthTokenType::AppPassword,
                false,
            )
            .await
            .unwrap();
        // Pre-disable: token authenticates a GET /cloud/user.
        let app_pre = build_router(state.clone(), axum::Router::new());
        let pre = Request::builder()
            .uri("/ocs/v2.php/cloud/user?format=json")
            .header("ocs-apirequest", "true")
            .header("authorization", format!("Bearer {}", raw.expose()))
            .body(Body::empty())
            .unwrap();
        let pre_resp = app_pre.oneshot(pre).await.unwrap();
        assert_eq!(pre_resp.status(), StatusCode::OK);

        let cookie = seed_login(&state, "admin").await;
        let app = build_router(state, axum::Router::new());
        let req = Request::builder()
            .method("PUT")
            .uri("/ocs/v2.php/cloud/users/alice/disable?format=json")
            .header("ocs-apirequest", "true")
            .header("cookie", cookie)
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Post-disable: same Bearer is 401.
        let post = Request::builder()
            .uri("/ocs/v2.php/cloud/user?format=json")
            .header("ocs-apirequest", "true")
            .header("authorization", format!("Bearer {}", raw.expose()))
            .body(Body::empty())
            .unwrap();
        let post_resp = app.oneshot(post).await.unwrap();
        assert_eq!(post_resp.status(), StatusCode::UNAUTHORIZED);
    }

    // NOTE: `disable_last_admin_returns_400` is documented in the plan as not
    // reachable at the HTTP level (same reasoning as delete_last_admin — the
    // self-guard fires before the structural guard when the sole admin is
    // also the caller). Structural guard is exercised via Batch B helpers.

    // --- edit_user (password rotation) --------------------------------------

    #[tokio::test]
    async fn admin_password_rotation_cascades_target_tokens() {
        let dir = tempdir().unwrap();
        let state = make_state(dir.path().join("u.db")).await;
        seed_user(&state, "admin", "hunter2", true).await;
        seed_user(&state, "alice", "old", false).await;
        let ap = state.users.app_passwords().unwrap().clone();
        let (_row, raw) = ap
            .mint(
                &UserId::new("alice").unwrap(),
                "alice",
                "DAV",
                AuthTokenType::AppPassword,
                false,
            )
            .await
            .unwrap();
        let cookie = seed_login(&state, "admin").await;
        let app = build_router(state, axum::Router::new());

        let req = Request::builder()
            .method("PUT")
            .uri("/ocs/v2.php/cloud/users/alice?format=json")
            .header("ocs-apirequest", "true")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("cookie", cookie.clone())
            .body(Body::from("key=password&value=newpw"))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Alice's existing token now fails.
        assert!(matches!(
            ap.verify(raw.expose()).await,
            Err(crabcloud_users::UsersError::TokenNotFound)
        ));

        // Admin's own session still works.
        let self_req = Request::builder()
            .uri("/ocs/v2.php/cloud/user?format=json")
            .header("ocs-apirequest", "true")
            .header("cookie", cookie)
            .body(Body::empty())
            .unwrap();
        let self_resp = app.oneshot(self_req).await.unwrap();
        assert_eq!(self_resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn admin_password_rotation_of_self_returns_400() {
        let dir = tempdir().unwrap();
        let state = make_state(dir.path().join("u.db")).await;
        seed_user(&state, "admin", "hunter2", true).await;
        let cookie = seed_login(&state, "admin").await;
        let app = build_router(state, axum::Router::new());

        let req = Request::builder()
            .method("PUT")
            .uri("/ocs/v2.php/cloud/users/admin?format=json")
            .header("ocs-apirequest", "true")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("cookie", cookie)
            .body(Body::from("key=password&value=newpw"))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // --- user-groups --------------------------------------------------------

    #[tokio::test]
    async fn list_user_groups_returns_membership() {
        let dir = tempdir().unwrap();
        let state = make_state(dir.path().join("u.db")).await;
        seed_user(&state, "admin", "hunter2", true).await;
        let cookie = seed_login(&state, "admin").await;
        let app = build_router(state, axum::Router::new());

        let req = Request::builder()
            .uri("/ocs/v2.php/cloud/users/admin/groups?format=json")
            .header("ocs-apirequest", "true")
            .header("cookie", cookie)
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 16 * 1024)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let groups = parsed["ocs"]["data"]["groups"].as_array().unwrap();
        assert!(groups.iter().any(|v| v.as_str() == Some("admin")));
    }

    #[tokio::test]
    async fn add_user_to_unknown_group_returns_400() {
        let dir = tempdir().unwrap();
        let state = make_state(dir.path().join("u.db")).await;
        seed_user(&state, "admin", "hunter2", true).await;
        seed_user(&state, "alice", "x", false).await;
        let cookie = seed_login(&state, "admin").await;
        let app = build_router(state, axum::Router::new());

        let req = Request::builder()
            .method("POST")
            .uri("/ocs/v2.php/cloud/users/alice/groups?format=json")
            .header("ocs-apirequest", "true")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("cookie", cookie)
            .body(Body::from("groupid=phantom"))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn remove_self_from_admin_group_returns_400() {
        let dir = tempdir().unwrap();
        let state = make_state(dir.path().join("u.db")).await;
        seed_user(&state, "admin", "hunter2", true).await;
        let cookie = seed_login(&state, "admin").await;
        let app = build_router(state, axum::Router::new());

        let req = Request::builder()
            .method("DELETE")
            .uri("/ocs/v2.php/cloud/users/admin/groups?groupid=admin&format=json")
            .header("ocs-apirequest", "true")
            .header("cookie", cookie)
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
