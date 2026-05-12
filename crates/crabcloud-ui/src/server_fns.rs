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

    let (_row, raw) = ap
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
    session
        .set_pending_cookie(crabcloud_http::PendingCookie::Set {
            raw_token: raw.expose().to_string(),
            max_age_secs: 30 * 60,
        })
        .await;
    Ok(())
}
