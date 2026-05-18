//! `#[server]` functions exposed by the UI app. Bodies execute on the server
//! only; the macro generates client stubs that POST to the matching endpoint.
//!
//! The legacy URL paths (`/index.php/login`, `/status.php`) are preserved via
//! explicit `endpoint` attributes so external Nextcloud-compatible clients
//! keep working.

pub mod activity;
pub mod notification_prefs;
pub mod public_link;
pub mod search;
pub mod trash;
pub mod versions;

use dioxus::fullstack::get;
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// Nextcloud-compatible status probe payload returned by `GET /status.php`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatusInfo {
    pub installed: bool,
    pub maintenance: bool,
    #[serde(rename = "needsDbUpgrade")]
    pub needs_db_upgrade: bool,
    pub version: String,
    pub versionstring: String,
    pub edition: String,
    pub productname: String,
    #[serde(rename = "extendedSupport")]
    pub extended_support: bool,
}

/// `GET /status.php` — Nextcloud-compatible probe used by clients (and CI
/// readiness checks) to identify the server. The endpoint URL and HTTP method
/// are fixed so legacy clients keep working; `#[get]` overrides the
/// `#[server]` macro's default of POST.
#[get("/status.php")]
pub async fn status() -> Result<StatusInfo, ServerFnError> {
    use dioxus::fullstack::FullstackContext;
    let state = FullstackContext::current()
        .and_then(|c| c.extension::<crabcloud_core::AppState>())
        .ok_or_else(|| ServerFnError::new("AppState extension missing"))?;
    Ok(StatusInfo {
        installed: state.config.installed,
        maintenance: false,
        needs_db_upgrade: false,
        version: state.config.version.clone(),
        versionstring: state.config.versionstring.clone(),
        edition: String::new(),
        productname: "Nextcloud".to_string(),
        extended_support: false,
    })
}

/// `POST /index.php/login` — login via the users service. Mints a session-
/// kind `AuthToken` for the user, stashes a pending Set-Cookie on the
/// `SessionHandle`, and mutates the ephemeral blob (user_id, CSRF, 2FA).
///
/// The cookie payload is the raw token; the SessionLayer HMAC-signs it.
#[server(endpoint = "index.php/login", prefix = "")]
pub async fn login(
    username: String,
    password: String,
    remember: Option<bool>,
) -> Result<(), ServerFnError> {
    use crabcloud_users::AuthTokenType;
    use dioxus::fullstack::FullstackContext;

    let fs =
        FullstackContext::current().ok_or_else(|| ServerFnError::new("not running on server"))?;
    let state = fs
        .extension::<crabcloud_core::AppState>()
        .ok_or_else(|| ServerFnError::new("AppState extension missing"))?;
    let session = fs
        .extension::<crabcloud_http::SessionHandle>()
        .ok_or_else(|| ServerFnError::new("session extension missing"))?;

    let user = state
        .users
        .verify(&username, &password)
        .await
        .map_err(|e| {
            ::tracing::warn!(username = %username, error = %e, "login verify failed");
            ServerFnError::new("unauthorized")
        })?;

    let ap = state
        .users
        .app_passwords()
        .ok_or_else(|| ServerFnError::new("app_passwords missing on AppState"))?
        .clone();

    // Best-effort user-agent extraction via FullstackContext's request parts.
    let user_agent = fs
        .parts_mut()
        .headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("Browser")
        .to_string();

    let (row, raw) = ap
        .mint(
            &user.uid,
            &username,
            &user_agent,
            AuthTokenType::Session,
            remember.unwrap_or(false),
        )
        .await
        .map_err(|e| {
            ::tracing::warn!(error = %e, "session token mint failed");
            ServerFnError::new("internal")
        })?;

    let uid_str = user.uid.as_str().to_string();
    session
        .mutate(|s| {
            s.user_id = Some(uid_str);
            s.rotate_csrf();
            s.two_factor_passed = true;
        })
        .await;
    // Pass the freshly-minted row's id so the SessionLayer saves this
    // request's blob (incl. the rotated csrf_token + two_factor_passed) under
    // the NEW token id. Without this the AuthLayer's `token_id_opt` is None
    // for the login request and the blob would be dropped on the floor.
    session
        .set_pending_cookie(crabcloud_http::PendingCookie::Set {
            raw_token: raw.expose().to_string(),
            token_id: row.id,
            max_age_secs: 30 * 60,
        })
        .await;
    Ok(())
}

/// Polling endpoint info returned by `/index.php/login/v2`. The Nextcloud
/// client treats `token` as opaque and POSTs it back to `endpoint` until the
/// flow completes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginV2Poll {
    pub token: String,
    pub endpoint: String,
}

/// Bootstrap payload returned by `POST /index.php/login/v2`. `login` is the
/// URL the client opens in the user's browser; `poll` is the long-poll
/// channel the client uses to retrieve the eventual app password.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginV2StartResponse {
    pub poll: LoginV2Poll,
    pub login: String,
}

/// TTL on the cached login/v2 flow + poll records. Matches Nextcloud's
/// 20-minute window before the client is expected to retry start.
#[cfg(feature = "server")]
pub(crate) const LOGIN_V2_TTL_SECS: u64 = 20 * 60;

#[cfg(feature = "server")]
fn login_v2_random_id() -> String {
    use rand::Rng;
    let mut buf = [0u8; 32];
    rand::rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

#[cfg(feature = "server")]
fn server_base(state: &crabcloud_core::AppState) -> String {
    if let Some(u) = state.config.overwrite_cli_url.clone() {
        return u;
    }
    if let Some(d) = state.config.trusted_domains.first() {
        return format!("https://{d}");
    }
    format!("http://{}", state.config.bind_address)
}

/// `POST /index.php/login/v2` — Nextcloud-client bootstrap. Issues a
/// random `poll_id` (returned as `poll.token`) and a separate random
/// `flow_id` (encoded into the `login` URL). The client opens the login
/// URL in a browser, the user authenticates and authorizes; meanwhile the
/// client long-polls `/index.php/login/v2/poll` with the token.
#[server(endpoint = "index.php/login/v2", prefix = "")]
pub async fn login_v2_start() -> Result<LoginV2StartResponse, ServerFnError> {
    use dioxus::fullstack::FullstackContext;
    use std::time::Duration;

    let fs =
        FullstackContext::current().ok_or_else(|| ServerFnError::new("not running on server"))?;
    let state = fs
        .extension::<crabcloud_core::AppState>()
        .ok_or_else(|| ServerFnError::new("AppState extension missing"))?;

    let poll_id = login_v2_random_id();
    let flow_id = login_v2_random_id();
    let base = server_base(&state);

    let cache = state.cache.clone();
    let inst = &state.config.instanceid;
    let poll_key = format!("{inst}:login_v2:poll:{poll_id}");
    let flow_key = format!("{inst}:login_v2:flow:{flow_id}");
    // Empty value in the poll slot means "issued but not yet authorized".
    // The authorize fn will overwrite it with the JSON payload containing
    // the raw token + loginName.
    let flow_record = serde_json::to_vec(&serde_json::json!({ "poll_id": poll_id })).unwrap();
    cache
        .set(&poll_key, b"", Some(Duration::from_secs(LOGIN_V2_TTL_SECS)))
        .await
        .map_err(|e| ServerFnError::new(format!("cache: {e}")))?;
    cache
        .set(
            &flow_key,
            &flow_record,
            Some(Duration::from_secs(LOGIN_V2_TTL_SECS)),
        )
        .await
        .map_err(|e| ServerFnError::new(format!("cache: {e}")))?;

    Ok(LoginV2StartResponse {
        poll: LoginV2Poll {
            token: poll_id,
            endpoint: format!("{base}/index.php/login/v2/poll"),
        },
        login: format!("{base}/index.php/login/v2/flow/{flow_id}"),
    })
}

/// Body of `POST /index.php/login/v2/poll`. The Nextcloud client retries on
/// any non-200 response and stops on the first 200.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginV2PollRequest {
    pub token: String,
}

/// Successful poll response, returned only after a browser session has
/// authorized the flow. The `appPassword` is the raw token (shown once).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginV2PollResponse {
    pub server: String,
    #[serde(rename = "loginName")]
    pub login_name: String,
    #[serde(rename = "appPassword")]
    pub app_password: String,
}

/// `POST /index.php/login/v2/poll` — long-poll for the freshly-minted app
/// password. Returns `not_found` (via `ServerFnError`) until the flow has
/// been authorized via `/index.php/login/v2/authorize`. On hit, the poll
/// record is consumed so a second poll for the same token fails.
///
/// Note: `ServerFnError` doesn't carry an HTTP status so "not yet
/// authorized" surfaces as 500 rather than 404. Nextcloud clients retry on
/// any non-200 so this is acceptable.
#[server(endpoint = "index.php/login/v2/poll", prefix = "")]
pub async fn login_v2_poll(req: LoginV2PollRequest) -> Result<LoginV2PollResponse, ServerFnError> {
    use dioxus::fullstack::FullstackContext;

    let fs =
        FullstackContext::current().ok_or_else(|| ServerFnError::new("not running on server"))?;
    let state = fs
        .extension::<crabcloud_core::AppState>()
        .ok_or_else(|| ServerFnError::new("AppState extension missing"))?;

    let cache = state.cache.clone();
    let inst = &state.config.instanceid;
    let key = format!("{inst}:login_v2:poll:{}", req.token);
    let raw = cache
        .get(&key)
        .await
        .map_err(|e| ServerFnError::new(format!("cache: {e}")))?
        .ok_or_else(|| ServerFnError::new("not_found"))?;
    if raw.is_empty() {
        return Err(ServerFnError::new("not_found"));
    }
    let _ = cache.del(&key).await;

    let payload: serde_json::Value = serde_json::from_slice(&raw)
        .map_err(|e| ServerFnError::new(format!("cache decode: {e}")))?;
    let login_name = payload["loginName"]
        .as_str()
        .ok_or_else(|| ServerFnError::new("malformed flow record"))?
        .to_string();
    let app_password = payload["appPassword"]
        .as_str()
        .ok_or_else(|| ServerFnError::new("malformed flow record"))?
        .to_string();
    Ok(LoginV2PollResponse {
        server: server_base(&state),
        login_name,
        app_password,
    })
}

/// Summary of an `oc_authtoken` row shown in the security settings UI.
/// `kind` is the [`crabcloud_users::AuthTokenType`] discriminator
/// (`0` = Session, `1` = AppPassword). `current` is `true` for the row
/// backing the requesting session cookie.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthTokenSummary {
    pub id: i64,
    pub name: String,
    pub kind: i32,
    pub last_activity: u64,
    pub current: bool,
}

/// Response from [`create_app_password`]. The plaintext `raw_token` is the
/// only chance the caller has to capture the secret — it is not retrievable
/// afterwards.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatedAppPassword {
    pub id: i64,
    pub name: String,
    pub raw_token: String,
}

/// `POST /settings/security/list` — return every `oc_authtoken` row owned
/// by the requesting user. Session-only (cookie auth) to keep this surface
/// off the Bearer / Basic paths used by sync clients.
#[server(endpoint = "settings/security/list", prefix = "")]
pub async fn list_app_passwords() -> Result<Vec<AuthTokenSummary>, ServerFnError> {
    use dioxus::fullstack::FullstackContext;
    let fs =
        FullstackContext::current().ok_or_else(|| ServerFnError::new("not running on server"))?;
    let state = fs
        .extension::<crabcloud_core::AppState>()
        .ok_or_else(|| ServerFnError::new("AppState missing"))?;
    let ctx = fs
        .extension::<crabcloud_http::AuthContext>()
        .ok_or_else(|| ServerFnError::new("unauthorized"))?;
    if ctx.method != crabcloud_http::AuthMethod::Session {
        return Err(ServerFnError::new("session-only"));
    }
    let ap = state
        .users
        .app_passwords()
        .ok_or_else(|| ServerFnError::new("app_passwords missing"))?
        .clone();
    let rows = ap
        .list(&ctx.user_id)
        .await
        .map_err(|e| ServerFnError::new(format!("list: {e}")))?;
    Ok(rows
        .into_iter()
        .map(|r| AuthTokenSummary {
            current: r.id == ctx.token_id,
            id: r.id,
            name: r.name,
            kind: r.kind.as_i32(),
            last_activity: r.last_activity,
        })
        .collect())
}

/// `POST /settings/security/create` — mint a fresh `AppPassword`-kind
/// token for the requesting user, returning the plaintext exactly once.
/// Session-only.
#[server(endpoint = "settings/security/create", prefix = "")]
pub async fn create_app_password(name: String) -> Result<CreatedAppPassword, ServerFnError> {
    use crabcloud_users::AuthTokenType;
    use dioxus::fullstack::FullstackContext;
    let fs =
        FullstackContext::current().ok_or_else(|| ServerFnError::new("not running on server"))?;
    let state = fs
        .extension::<crabcloud_core::AppState>()
        .ok_or_else(|| ServerFnError::new("AppState missing"))?;
    let ctx = fs
        .extension::<crabcloud_http::AuthContext>()
        .ok_or_else(|| ServerFnError::new("unauthorized"))?;
    if ctx.method != crabcloud_http::AuthMethod::Session {
        return Err(ServerFnError::new("session-only"));
    }
    let ap = state
        .users
        .app_passwords()
        .ok_or_else(|| ServerFnError::new("app_passwords missing"))?
        .clone();
    let (row, raw) = ap
        .mint(
            &ctx.user_id,
            &ctx.login_name,
            &name,
            AuthTokenType::AppPassword,
            false,
        )
        .await
        .map_err(|e| ServerFnError::new(format!("mint: {e}")))?;
    Ok(CreatedAppPassword {
        id: row.id,
        name: row.name,
        raw_token: raw.expose().to_string(),
    })
}

/// `POST /settings/security/revoke` — revoke a specific token row by id.
/// Session-only and refuses to revoke rows owned by another user.
#[server(endpoint = "settings/security/revoke", prefix = "")]
pub async fn revoke_app_password(id: i64) -> Result<(), ServerFnError> {
    use dioxus::fullstack::FullstackContext;
    let fs =
        FullstackContext::current().ok_or_else(|| ServerFnError::new("not running on server"))?;
    let state = fs
        .extension::<crabcloud_core::AppState>()
        .ok_or_else(|| ServerFnError::new("AppState missing"))?;
    let ctx = fs
        .extension::<crabcloud_http::AuthContext>()
        .ok_or_else(|| ServerFnError::new("unauthorized"))?;
    if ctx.method != crabcloud_http::AuthMethod::Session {
        return Err(ServerFnError::new("session-only"));
    }
    let ap = state
        .users
        .app_passwords()
        .ok_or_else(|| ServerFnError::new("app_passwords missing"))?
        .clone();
    // Authorize: the row must exist AND belong to the calling user.
    // Return the same error for "unknown" and "not yours" to avoid a
    // probe-the-id-space enumeration oracle.
    let row = ap
        .lookup_by_id(id)
        .await
        .map_err(|e| ServerFnError::new(format!("lookup: {e}")))?
        .ok_or_else(|| ServerFnError::new("not your token"))?;
    if row.uid != ctx.user_id {
        return Err(ServerFnError::new("not your token"));
    }
    ap.revoke(id)
        .await
        .map_err(|e| ServerFnError::new(format!("revoke: {e}")))
}

/// `POST /settings/security/destroy-others` — revoke every token row owned
/// by the requesting user except the one backing this session. Session-only.
#[server(endpoint = "settings/security/destroy-others", prefix = "")]
pub async fn destroy_other_sessions() -> Result<(), ServerFnError> {
    use dioxus::fullstack::FullstackContext;
    let fs =
        FullstackContext::current().ok_or_else(|| ServerFnError::new("not running on server"))?;
    let state = fs
        .extension::<crabcloud_core::AppState>()
        .ok_or_else(|| ServerFnError::new("AppState missing"))?;
    let ctx = fs
        .extension::<crabcloud_http::AuthContext>()
        .ok_or_else(|| ServerFnError::new("unauthorized"))?;
    if ctx.method != crabcloud_http::AuthMethod::Session {
        return Err(ServerFnError::new("session-only"));
    }
    let ap = state
        .users
        .app_passwords()
        .ok_or_else(|| ServerFnError::new("app_passwords missing"))?
        .clone();
    ap.revoke_other_sessions(&ctx.user_id, ctx.token_id)
        .await
        .map_err(|e| ServerFnError::new(format!("revoke_others: {e}")))
}

/// `POST /index.php/login/v2/authorize` — invoked by the
/// `/index.php/login/v2/flow/<id>` page after the user clicks Authorize.
/// Must be authenticated via `AuthMethod::Session` (cookie). Mints a fresh
/// `AppPassword`-kind token and hands it to the polling channel by
/// overwriting the cached poll record.
#[server(endpoint = "index.php/login/v2/authorize", prefix = "")]
pub async fn login_v2_authorize(flow_id: String) -> Result<(), ServerFnError> {
    use crabcloud_users::AuthTokenType;
    use dioxus::fullstack::FullstackContext;

    let fs =
        FullstackContext::current().ok_or_else(|| ServerFnError::new("not running on server"))?;
    let state = fs
        .extension::<crabcloud_core::AppState>()
        .ok_or_else(|| ServerFnError::new("AppState extension missing"))?;
    let ctx = fs
        .extension::<crabcloud_http::AuthContext>()
        .ok_or_else(|| ServerFnError::new("unauthorized"))?;
    if ctx.method != crabcloud_http::AuthMethod::Session {
        return Err(ServerFnError::new(
            "must be authenticated via session cookie",
        ));
    }

    let cache = state.cache.clone();
    let inst = &state.config.instanceid;
    let flow_key = format!("{inst}:login_v2:flow:{flow_id}");
    let raw = cache
        .get(&flow_key)
        .await
        .map_err(|e| ServerFnError::new(format!("cache: {e}")))?
        .ok_or_else(|| ServerFnError::new("flow_not_found"))?;
    let payload: serde_json::Value = serde_json::from_slice(&raw)
        .map_err(|e| ServerFnError::new(format!("cache decode: {e}")))?;
    let poll_id = payload["poll_id"]
        .as_str()
        .ok_or_else(|| ServerFnError::new("malformed flow record"))?
        .to_string();

    // Drop the parts guard before any `.await` (it's a RwLock write guard).
    let user_agent = {
        let parts = fs.parts_mut();
        parts
            .headers
            .get(axum::http::header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "Client".to_string())
    };

    let ap = state
        .users
        .app_passwords()
        .ok_or_else(|| ServerFnError::new("app_passwords missing"))?
        .clone();
    let (_row, raw_token) = ap
        .mint(
            &ctx.user_id,
            &ctx.login_name,
            &user_agent,
            AuthTokenType::AppPassword,
            false,
        )
        .await
        .map_err(|e| ServerFnError::new(format!("mint: {e}")))?;

    let poll_key = format!("{inst}:login_v2:poll:{poll_id}");
    let payload = serde_json::json!({
        "loginName": ctx.login_name.as_str(),
        "appPassword": raw_token.expose(),
    });
    let bytes = serde_json::to_vec(&payload).unwrap();
    cache
        .set(
            &poll_key,
            &bytes,
            Some(std::time::Duration::from_secs(LOGIN_V2_TTL_SECS)),
        )
        .await
        .map_err(|e| ServerFnError::new(format!("cache: {e}")))?;
    let _ = cache.del(&flow_key).await;
    Ok(())
}

/// Single entry in a `list_dir` response. Shape is what the UI needs;
/// server-side it's filled from `crabcloud_storage::DirEntry`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileEntry {
    pub name: String,
    /// Full UserPath, e.g. `/photos/cat.jpg`.
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime_ms: i64,
    pub mime: Option<String>,
    pub etag: String,
    /// Filecache row id for owner-side rows. Used by the Files UI to build
    /// `/api/files/preview/{fileid}?size=N` URLs for previewable mimes.
    /// `None` for directories, share-mount entries (the recipient doesn't
    /// hold a fileid pointer into the owner's cache), and rows where the
    /// filecache lookup did not return a row.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub fileid: Option<i64>,
    /// Owner uid when this row sits at the recipient-facing root of an
    /// incoming share mount; `None` for ordinary home-mount entries.
    /// Populated by `list_dir` from the mount's `MountMetadata.owner_uid`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shared_by: Option<String>,
    /// Outgoing-share count for owner-side rows. The Files UI renders a
    /// `🔗 N` chip next to entries with `share_count > 0`. Bulk-computed
    /// per listing via `Shares::share_counts_for`.
    #[serde(default)]
    pub share_count: i64,
}

/// Resolve the per-request `AppState` + caller `UserId` from the
/// `FullstackContext`. Shared by every authenticated server fn in the
/// Files surface — uses the `AuthContext` extension installed by
/// `AuthLayer` (any auth method) rather than peeking at the session
/// snapshot directly.
#[cfg(feature = "server")]
async fn require_user() -> Result<(crabcloud_core::AppState, crabcloud_users::UserId), ServerFnError>
{
    use dioxus::fullstack::FullstackContext;
    let fs =
        FullstackContext::current().ok_or_else(|| ServerFnError::new("not running on server"))?;
    let state = fs
        .extension::<crabcloud_core::AppState>()
        .ok_or_else(|| ServerFnError::new("AppState extension missing"))?;
    let auth = fs
        .extension::<crabcloud_http::AuthContext>()
        .ok_or_else(|| ServerFnError::new("unauthorized"))?;
    Ok((state, auth.user_id.clone()))
}

/// `POST /api/files/list` — list a directory. Returns sorted entries
/// (directories first, then files; alphabetical within each group,
/// case-insensitive). JSON body: `{ "path": "/photos" }`.
///
/// Auth: any method (Bearer / Basic / Session) recognized by `AuthLayer`
/// — the Files surface is browser-cookie-only in practice but the
/// extractor is method-agnostic so integration tests can drive it with
/// bearer tokens, same as the WebDAV surface.
#[server(endpoint = "api/files/list", prefix = "")]
pub async fn list_dir(path: String) -> Result<Vec<FileEntry>, ServerFnError> {
    use crabcloud_fs::UserPath;

    let (state, uid) = require_user().await?;

    let user_path =
        UserPath::new(path).map_err(|e| ServerFnError::new(format!("invalid path: {e}")))?;
    let view = state
        .view_for(&uid)
        .await
        .map_err(|e| ServerFnError::new(format!("view: {e}")))?;
    let raw = view.list_with_meta(&user_path).await.map_err(map_fs_err)?;

    let mut out: Vec<FileEntry> = raw
        .into_iter()
        .map(|le| {
            let mut dto = dir_entry_to_dto(&user_path, le.entry);
            if let Some(md) = &le.mount_metadata {
                if matches!(md.kind, crabcloud_fs::MountKind::Share) {
                    dto.shared_by = md.owner_uid.clone();
                }
            }
            dto
        })
        .collect();

    // Decorate owner-side rows with their outgoing-share count via one
    // batched query. Look up fileids by `(home_storage_id, full_path)`
    // for entries that did NOT come in as a share-mount synthetic entry
    // (those belong to the owner, not to `uid`). Missing filecache rows
    // are tolerated — drop the row from the count lookup and default to 0.
    let owner_storage = state
        .storage_factory
        .home_storage(&uid)
        .await
        .map_err(map_fs_err)?;
    let owner_sid = owner_storage.id().to_string();
    let mut idx_to_fileid: Vec<(usize, i64)> = Vec::with_capacity(out.len());
    for (i, e) in out.iter_mut().enumerate() {
        if e.shared_by.is_some() {
            continue;
        }
        let storage_path_str = e.path.trim_start_matches('/');
        let sp = match crabcloud_storage::StoragePath::new(storage_path_str) {
            Ok(p) => p,
            Err(_) => continue,
        };
        if let Ok(Some(row)) = state.filecache.lookup(&owner_sid, &sp).await {
            idx_to_fileid.push((i, row.fileid));
            // Decorate the DTO with the fileid so the UI can build
            // `/api/files/preview/{fileid}` thumbnail URLs. Directories
            // and share-mount entries leave this `None`.
            e.fileid = Some(row.fileid);
        }
    }
    let fileids: Vec<i64> = idx_to_fileid.iter().map(|(_, f)| *f).collect();
    if !fileids.is_empty() {
        let counts = state
            .shares
            .share_counts_for(&uid, &fileids)
            .await
            .map_err(|e| ServerFnError::new(format!("share counts: {e}")))?;
        for (i, fid) in idx_to_fileid {
            if let Some(n) = counts.get(&fid).copied() {
                out[i].share_count = n;
            }
        }
    }

    out.sort_by(|a, b| match (b.is_dir, a.is_dir) {
        (true, false) => std::cmp::Ordering::Greater,
        (false, true) => std::cmp::Ordering::Less,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    Ok(out)
}

/// `POST /api/files/mkdir` — create a directory at `path`. Returns the
/// new directory's metadata as a `FileEntry`.
#[server(endpoint = "api/files/mkdir", prefix = "")]
pub async fn mkdir(path: String) -> Result<FileEntry, ServerFnError> {
    use crabcloud_fs::UserPath;
    let (state, uid) = require_user().await?;
    let user_path =
        UserPath::new(&path).map_err(|e| ServerFnError::new(format!("invalid_path: {e}")))?;
    let view = state.view_for(&uid).await.map_err(map_fs_err)?;
    let meta = view.mkdir(&user_path).await.map_err(map_fs_err)?;
    Ok(metadata_to_entry(&user_path, meta))
}

/// `POST /api/files/rename` — move/rename `from` to `to`. Returns the
/// destination entry's fresh metadata.
#[server(endpoint = "api/files/rename", prefix = "")]
pub async fn rename(from: String, to: String) -> Result<FileEntry, ServerFnError> {
    use crabcloud_fs::UserPath;
    let (state, uid) = require_user().await?;
    let from_path =
        UserPath::new(&from).map_err(|e| ServerFnError::new(format!("invalid_path: {e}")))?;
    let to_path =
        UserPath::new(&to).map_err(|e| ServerFnError::new(format!("invalid_path: {e}")))?;
    let view = state.view_for(&uid).await.map_err(map_fs_err)?;
    view.rename(&from_path, &to_path)
        .await
        .map_err(map_fs_err)?;
    let meta = view.stat(&to_path).await.map_err(map_fs_err)?;
    Ok(metadata_to_entry(&to_path, meta))
}

/// `POST /api/files/delete` — delete every path in `paths`. Best-effort
/// sequential: the first error short-circuits the rest.
#[server(endpoint = "api/files/delete", prefix = "")]
pub async fn delete(paths: Vec<String>) -> Result<(), ServerFnError> {
    use crabcloud_fs::UserPath;
    let (state, uid) = require_user().await?;
    let view = state.view_for(&uid).await.map_err(map_fs_err)?;
    for path in paths {
        let user_path =
            UserPath::new(&path).map_err(|e| ServerFnError::new(format!("invalid_path: {e}")))?;
        view.delete(&user_path).await.map_err(map_fs_err)?;
    }
    Ok(())
}

#[server(endpoint = "api/files/move", prefix = "")]
pub async fn move_paths(
    paths: Vec<String>,
    dest_dir: String,
) -> Result<Vec<FileEntry>, ServerFnError> {
    use crabcloud_fs::UserPath;
    let (state, uid) = require_user().await?;
    let dest =
        UserPath::new(&dest_dir).map_err(|e| ServerFnError::new(format!("invalid_path: {e}")))?;
    let view = state.view_for(&uid).await.map_err(map_fs_err)?;
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        let from =
            UserPath::new(&path).map_err(|e| ServerFnError::new(format!("invalid_path: {e}")))?;
        let leaf = path.rsplit('/').next().unwrap_or("");
        if leaf.is_empty() {
            return Err(ServerFnError::new("invalid_path: empty leaf"));
        }
        let to = dest
            .join(leaf)
            .map_err(|e| ServerFnError::new(format!("invalid_path: {e}")))?;
        view.rename(&from, &to).await.map_err(map_fs_err)?;
        let meta = view.stat(&to).await.map_err(map_fs_err)?;
        out.push(metadata_to_entry(&to, meta));
    }
    Ok(out)
}

#[cfg(feature = "server")]
fn metadata_to_entry(
    user_path: &crabcloud_fs::UserPath,
    meta: crabcloud_storage::FileMetadata,
) -> FileEntry {
    use std::time::UNIX_EPOCH;
    let is_dir = matches!(meta.kind, crabcloud_storage::FileKind::Directory);
    let mtime_ms = meta
        .mtime
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    FileEntry {
        name: user_path
            .as_str()
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_string(),
        path: user_path.as_str().to_string(),
        is_dir,
        size: meta.size,
        mtime_ms,
        mime: (!is_dir).then(|| meta.mimetype.as_str().to_string()),
        etag: meta.etag.as_str().to_string(),
        fileid: None,
        shared_by: None,
        share_count: 0,
    }
}

#[cfg(feature = "server")]
fn dir_entry_to_dto(
    parent: &crabcloud_fs::UserPath,
    entry: crabcloud_storage::DirEntry,
) -> FileEntry {
    use std::time::UNIX_EPOCH;
    let full_path = parent
        .join(&entry.name)
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|_| entry.name.clone());
    let mtime_ms = entry
        .metadata
        .mtime
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let is_dir = matches!(entry.metadata.kind, crabcloud_storage::FileKind::Directory);
    FileEntry {
        name: entry.name,
        path: full_path,
        is_dir,
        size: entry.metadata.size,
        mtime_ms,
        mime: (!is_dir).then(|| entry.metadata.mimetype.as_str().to_string()),
        etag: entry.metadata.etag.as_str().to_string(),
        fileid: None,
        shared_by: None,
        share_count: 0,
    }
}

/// One result row in the [`share_recipient_search`] response. `id` is the
/// raw uid (for User candidates) or gid (for Group candidates); `kind`
/// distinguishes the two for the UI; `share_type_int` is the value the
/// caller must send back in the OCS `POST shares` `shareType` field
/// (matches `crabcloud_sharing::ShareType` discriminants).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecipientCandidate {
    pub id: String,
    pub display_name: String,
    pub kind: String,
    pub share_type_int: i16,
}

/// `POST /api/files/share_recipient_search` — autocomplete back-end for
/// the Share modal's recipient picker. Returns up to 10 results unioned
/// from the user + group stores, filtered by case-insensitive substring
/// match against `uid|displayname` / `gid|displayname`. Empty /
/// whitespace-only `q` returns an empty list without hitting the
/// database. Authenticated callers only.
///
/// We over-fetch on each side (8 users + 8 groups), interleave them as
/// users-first, then truncate to 10. The over-fetch guarantees that even
/// when one side has many matches, the other side still gets to surface
/// at least a few hits — a single-side `limit: 10` followed by `truncate(10)`
/// would have produced a users-only list whenever the user search
/// returned >= 10 hits.
#[server(endpoint = "api/files/share_recipient_search", prefix = "")]
pub async fn share_recipient_search(q: String) -> Result<Vec<RecipientCandidate>, ServerFnError> {
    use crabcloud_users::{GroupListFilter, UserListFilter};
    let (state, _uid) = require_user().await?;
    let trimmed = q.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    const TOTAL: usize = 10;
    const PER_SIDE: u32 = 8;
    let users = state
        .users
        .user_store()
        .list_users(UserListFilter {
            search: Some(trimmed),
            limit: PER_SIDE,
            offset: 0,
        })
        .await
        .map_err(|e| ServerFnError::new(format!("user search: {e}")))?;
    let groups = state
        .users
        .group_store()
        .list_groups(GroupListFilter {
            search: Some(trimmed),
            limit: PER_SIDE,
            offset: 0,
        })
        .await
        .map_err(|e| ServerFnError::new(format!("group search: {e}")))?;
    let mut out: Vec<RecipientCandidate> = users
        .into_iter()
        .map(|u| RecipientCandidate {
            id: u.uid.as_str().to_string(),
            display_name: u.display_name,
            kind: "user".into(),
            share_type_int: 0,
        })
        .collect();
    out.extend(groups.into_iter().map(|g| RecipientCandidate {
        id: g.gid.as_str().to_string(),
        display_name: g.display_name,
        kind: "group".into(),
        share_type_int: 1,
    }));
    out.truncate(TOTAL);
    Ok(out)
}

/// `POST /api/files/count_incoming_shares` — returns how many accepted
/// incoming shares the caller is currently the recipient of. Used by
/// the sidebar's "Shared with you" chip to grey itself out when zero.
#[server(endpoint = "api/files/count_incoming_shares", prefix = "")]
pub async fn count_incoming_shares() -> Result<i64, ServerFnError> {
    let (state, uid) = require_user().await?;
    let rows = state
        .shares
        .list_incoming(&uid)
        .await
        .map_err(|e| ServerFnError::new(format!("list_incoming: {e}")))?;
    Ok(rows.len() as i64)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UploadBeginResponse {
    pub upload_id: String,
}

#[server(endpoint = "api/files/upload_begin", prefix = "")]
pub async fn upload_begin(dest_path: String) -> Result<UploadBeginResponse, ServerFnError> {
    use crabcloud_fs::UserPath;
    let (state, uid) = require_user().await?;
    let dest =
        UserPath::new(&dest_path).map_err(|e| ServerFnError::new(format!("invalid_path: {e}")))?;
    let uploads = state.uploads_for(&uid).await.map_err(map_fs_err)?;
    let handle = uploads.begin(&dest).await.map_err(map_fs_err)?;
    Ok(UploadBeginResponse {
        upload_id: handle.upload_id,
    })
}

#[cfg(feature = "server")]
fn map_fs_err(err: crabcloud_fs::FsError) -> ServerFnError {
    use crabcloud_fs::FsError;
    match err {
        FsError::NotFound => ServerFnError::new("not_found"),
        FsError::InvalidPath(m) => ServerFnError::new(format!("invalid_path: {m}")),
        FsError::CrossMount => ServerFnError::new("cross_mount"),
        FsError::MountNotFound => ServerFnError::new("mount_not_found"),
        FsError::Storage(s) => ServerFnError::new(format!("storage: {s}")),
        FsError::FileCache(c) => ServerFnError::new(format!("filecache: {c}")),
        FsError::Upload(m) => ServerFnError::new(format!("upload: {m}")),
        FsError::Forbidden => ServerFnError::new("forbidden"),
        FsError::Conflict => ServerFnError::new("conflict"),
        FsError::Unsupported => ServerFnError::new("unsupported"),
        FsError::CrossStorage => ServerFnError::new("cross_storage"),
        FsError::Trash(m) => ServerFnError::new(format!("trash: {m}")),
    }
}
