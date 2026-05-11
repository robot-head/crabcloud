//! `#[server]` functions exposed by the UI app. Bodies execute on the server
//! only; the macro generates client stubs that POST to the matching endpoint.
//!
//! The legacy URL paths (`/index.php/login`, `/status.php`) are preserved via
//! explicit `endpoint` attributes so external Nextcloud-compatible clients
//! keep working.

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

/// `GET /status.php` — Nextcloud-compatible probe used by clients to identify
/// the server. The endpoint URL is fixed so legacy clients keep working.
#[server(endpoint = "/status.php")]
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

/// `POST /index.php/login` — bootstrap admin login. Verifies bcrypt-hashed
/// credentials and mutates the session via the request extension installed
/// by `crabcloud-http`'s `SessionLayer`.
#[server(endpoint = "/index.php/login")]
pub async fn login(username: String, password: String) -> Result<(), ServerFnError> {
    use dioxus::fullstack::FullstackContext;
    let fs =
        FullstackContext::current().ok_or_else(|| ServerFnError::new("not running on server"))?;

    let state = fs
        .extension::<crabcloud_core::AppState>()
        .ok_or_else(|| ServerFnError::new("AppState extension missing"))?;
    let session = fs
        .extension::<crabcloud_http::SessionHandle>()
        .ok_or_else(|| ServerFnError::new("session extension missing"))?;

    let admin = state
        .config
        .bootstrap_admin
        .as_ref()
        .ok_or_else(|| ServerFnError::new("unauthorized"))?;
    if admin.username != username {
        return Err(ServerFnError::new("unauthorized"));
    }
    let ok = bcrypt::verify(&password, &admin.password_hash)
        .map_err(|e| ServerFnError::new(format!("bcrypt verify: {e}")))?;
    if !ok {
        return Err(ServerFnError::new("unauthorized"));
    }

    session
        .mutate(|s| {
            s.user_id = Some(username.clone());
            s.rotate_csrf();
        })
        .await;
    Ok(())
}
