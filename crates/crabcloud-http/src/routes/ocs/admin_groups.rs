//! Admin group-administration endpoints under `/ocs/v2.php/cloud/groups`.
//!
//! All handlers gated by the [`AdminUser`] extractor (401 anonymous, 403
//! non-admin). The `admin` group itself is structural and cannot be deleted.

use crate::extractors::auth::AdminUser;
use crate::extractors::format::OcsFormat;
use crate::OcsError;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Form;
use crabcloud_core::{AppState, Error as CoreError};
use crabcloud_ocs::{render, OcsResponse, OcsVersion};
use crabcloud_users::{Group, GroupId, GroupListFilter, UsersError};
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
    groups: Vec<String>,
}

/// `GET /ocs/v2.php/cloud/groups` — paginated, case-insensitive substring
/// search across `gid OR displayname`.
pub async fn list_groups(
    State(state): State<AppState>,
    _admin: AdminUser,
    fmt: OcsFormat,
    Query(q): Query<ListQuery>,
) -> Result<Response, OcsError> {
    let limit = q.limit.clamp(1, MAX_LIMIT);
    let filter = GroupListFilter {
        search: q.search.as_deref().filter(|s| !s.is_empty()),
        limit,
        offset: q.offset,
    };
    let rows = state
        .users
        .group_store()
        .list_groups(filter)
        .await
        .map_err(|e| users_err(e, fmt.0))?;
    Ok(ocs_ok(
        ListPayload {
            groups: rows
                .into_iter()
                .map(|g| g.gid.as_str().to_string())
                .collect(),
        },
        fmt.0,
    ))
}

#[derive(Debug, Deserialize)]
pub struct CreateGroupForm {
    pub groupid: String,
    #[serde(default)]
    pub displayname: Option<String>,
}

#[derive(Debug, Serialize)]
struct CreateGroupPayload {
    id: String,
}

/// `POST /ocs/v2.php/cloud/groups` — duplicate-group returns 409 Conflict.
pub async fn create_group(
    State(state): State<AppState>,
    admin: AdminUser,
    fmt: OcsFormat,
    Form(form): Form<CreateGroupForm>,
) -> Result<Response, OcsError> {
    let gid = GroupId::new(&form.groupid).map_err(|e| users_err(e, fmt.0))?;
    // Pre-check: existing group -> 409.
    if state
        .users
        .group_store()
        .lookup(&gid)
        .await
        .map_err(|e| users_err(e, fmt.0))?
        .is_some()
    {
        return Err(OcsError::new(
            CoreError::Conflict(format!("group already exists: {}", form.groupid)),
            OcsVersion::V2,
            fmt.0,
        ));
    }
    let display = form
        .displayname
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| form.groupid.clone());
    state
        .users
        .group_store()
        .create(&Group {
            gid: gid.clone(),
            display_name: display,
        })
        .await
        .map_err(|e| users_err(e, fmt.0))?;
    ::tracing::info!(
        actor = %admin.0.user_id,
        action = "create_group",
        target_gid = %gid,
        "admin OCS write"
    );
    Ok(ocs_ok(
        CreateGroupPayload {
            id: gid.as_str().to_string(),
        },
        fmt.0,
    ))
}

#[derive(Debug, Serialize)]
struct MembersPayload {
    users: Vec<String>,
}

/// `GET /ocs/v2.php/cloud/groups/{gid}` — 404 on unknown gid.
pub async fn list_group_members(
    State(state): State<AppState>,
    _admin: AdminUser,
    fmt: OcsFormat,
    Path(gid): Path<String>,
) -> Result<Response, OcsError> {
    let gid = GroupId::new(&gid).map_err(|e| users_err(e, fmt.0))?;
    if state
        .users
        .group_store()
        .lookup(&gid)
        .await
        .map_err(|e| users_err(e, fmt.0))?
        .is_none()
    {
        return Err(not_found(fmt.0));
    }
    let members = state
        .users
        .group_store()
        .members_of(&gid)
        .await
        .map_err(|e| users_err(e, fmt.0))?;
    Ok(ocs_ok(
        MembersPayload {
            users: members
                .into_iter()
                .map(|u| u.as_str().to_string())
                .collect(),
        },
        fmt.0,
    ))
}

/// `DELETE /ocs/v2.php/cloud/groups/{gid}` — structural guard: deleting the
/// `admin` group returns 400; unknown gid returns 404.
pub async fn delete_group(
    State(state): State<AppState>,
    admin: AdminUser,
    fmt: OcsFormat,
    Path(gid): Path<String>,
) -> Result<Response, OcsError> {
    let gid = GroupId::new(&gid).map_err(|e| users_err(e, fmt.0))?;
    if gid.as_str() == "admin" {
        return Err(bad_request("the admin group is structural", fmt.0));
    }
    if state
        .users
        .group_store()
        .lookup(&gid)
        .await
        .map_err(|e| users_err(e, fmt.0))?
        .is_none()
    {
        return Err(not_found(fmt.0));
    }
    state
        .users
        .group_store()
        .delete(&gid)
        .await
        .map_err(|e| users_err(e, fmt.0))?;
    ::tracing::info!(
        actor = %admin.0.user_id,
        action = "delete_group",
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

    async fn seed_admin(state: &AppState, uid: &str) {
        let hash = BcryptVerifier::new().hash("hunter2").unwrap();
        state
            .users
            .user_store()
            .create(
                &UserRow {
                    uid: UserId::new(uid).unwrap(),
                    display_name: uid.into(),
                    email: None,
                    enabled: true,
                    last_seen: 0,
                },
                Some(&hash),
            )
            .await
            .unwrap();
        let groups = SqlGroupStore::new(state.pool.clone());
        groups
            .add_to_group(&UserId::new(uid).unwrap(), &GroupId::new("admin").unwrap())
            .await
            .unwrap();
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

    #[tokio::test]
    async fn list_groups_returns_seeded_admin() {
        let dir = tempdir().unwrap();
        let state = make_state(dir.path().join("g.db")).await;
        seed_admin(&state, "admin").await;
        let cookie = seed_login(&state, "admin").await;
        let app = build_router(state, axum::Router::new());

        let req = Request::builder()
            .uri("/ocs/v2.php/cloud/groups?format=json")
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
    async fn create_group_then_list_members_empty() {
        let dir = tempdir().unwrap();
        let state = make_state(dir.path().join("g.db")).await;
        seed_admin(&state, "admin").await;
        let cookie = seed_login(&state, "admin").await;
        let app = build_router(state, axum::Router::new());

        let create = Request::builder()
            .method("POST")
            .uri("/ocs/v2.php/cloud/groups?format=json")
            .header("ocs-apirequest", "true")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("cookie", cookie.clone())
            .body(Body::from("groupid=developers&displayname=Devs"))
            .unwrap();
        let create_resp = app.clone().oneshot(create).await.unwrap();
        assert_eq!(create_resp.status(), StatusCode::OK);

        let list = Request::builder()
            .uri("/ocs/v2.php/cloud/groups/developers?format=json")
            .header("ocs-apirequest", "true")
            .header("cookie", cookie)
            .body(Body::empty())
            .unwrap();
        let list_resp = app.oneshot(list).await.unwrap();
        assert_eq!(list_resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(list_resp.into_body(), 16 * 1024)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["ocs"]["data"]["users"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn delete_admin_group_returns_400() {
        let dir = tempdir().unwrap();
        let state = make_state(dir.path().join("g.db")).await;
        seed_admin(&state, "admin").await;
        let cookie = seed_login(&state, "admin").await;
        let app = build_router(state, axum::Router::new());

        let req = Request::builder()
            .method("DELETE")
            .uri("/ocs/v2.php/cloud/groups/admin?format=json")
            .header("ocs-apirequest", "true")
            .header("cookie", cookie)
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_duplicate_group_returns_409() {
        let dir = tempdir().unwrap();
        let state = make_state(dir.path().join("g.db")).await;
        seed_admin(&state, "admin").await;
        let cookie = seed_login(&state, "admin").await;
        let app = build_router(state, axum::Router::new());

        // First create succeeds.
        let req1 = Request::builder()
            .method("POST")
            .uri("/ocs/v2.php/cloud/groups?format=json")
            .header("ocs-apirequest", "true")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("cookie", cookie.clone())
            .body(Body::from("groupid=developers"))
            .unwrap();
        let resp1 = app.clone().oneshot(req1).await.unwrap();
        assert_eq!(resp1.status(), StatusCode::OK);

        // Second create returns 409.
        let req2 = Request::builder()
            .method("POST")
            .uri("/ocs/v2.php/cloud/groups?format=json")
            .header("ocs-apirequest", "true")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("cookie", cookie)
            .body(Body::from("groupid=developers"))
            .unwrap();
        let resp2 = app.oneshot(req2).await.unwrap();
        assert_eq!(resp2.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn list_unknown_group_members_returns_404() {
        let dir = tempdir().unwrap();
        let state = make_state(dir.path().join("g.db")).await;
        seed_admin(&state, "admin").await;
        let cookie = seed_login(&state, "admin").await;
        let app = build_router(state, axum::Router::new());

        let req = Request::builder()
            .uri("/ocs/v2.php/cloud/groups/phantom?format=json")
            .header("ocs-apirequest", "true")
            .header("cookie", cookie)
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
