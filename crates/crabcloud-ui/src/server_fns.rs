//! `#[server]` functions exposed by the UI app. Bodies execute on the server
//! only; the macro generates client stubs that POST to the matching endpoint.
//!
//! The legacy URL paths (`/index.php/login`, `/status.php`) are preserved via
//! explicit `endpoint` attributes so external Nextcloud-compatible clients
//! keep working.

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
    use rand::RngCore;
    let mut buf = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut buf);
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
        "loginName": ctx.user_id.as_str(),
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
