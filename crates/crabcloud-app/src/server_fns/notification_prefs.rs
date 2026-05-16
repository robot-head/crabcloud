//! Server fns for the per-user notification-preferences panel.
//!
//! `POST /api/notification_prefs/get` returns the user's current opt-in state
//! for each of the three notification event types (`share_created`,
//! `link_emailed`, `expiration_warning`). Default-true semantics: when no
//! row is stored, the user is considered opted in.
//!
//! `POST /api/notification_prefs/set` upserts the opt-in state for a single
//! event type. The `event_type` string is validated against the same 3-event
//! whitelist that the rest of the codebase uses (see
//! `crabcloud_mail::EventType`).
//!
//! Both endpoints are session-only — this surface is for the browser
//! settings page, not for sync clients — and require an authenticated user.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// Snapshot of a user's notification opt-in state across the 3 supported
/// event types. `true` means "send email"; `false` means "user opted out".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotificationPrefsDto {
    /// Email me when someone shares a file or folder with me.
    pub share_created: bool,
    /// Send me a copy of email-share confirmations I trigger.
    pub link_emailed: bool,
    /// Warn me before one of my public links expires.
    pub expiration_warning: bool,
}

/// The 3 event-type strings this surface accepts. Centralising the list
/// keeps the set/validation/UI in lock-step.
pub const KNOWN_EVENT_TYPES: &[&str] = &["share_created", "link_emailed", "expiration_warning"];

/// `POST /api/notification_prefs/get` — return the calling user's opt-in
/// state for all 3 known event types. Default = true when no row exists.
#[server(endpoint = "api/notification_prefs/get", prefix = "")]
pub async fn notification_prefs_get() -> Result<NotificationPrefsDto, ServerFnError> {
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
    let uid = ctx.user_id.as_str();
    let prefs = &state.notification_prefs;
    let share_created = prefs
        .get(uid, "share_created")
        .await
        .map_err(|e| ServerFnError::new(format!("get share_created: {e}")))?;
    let link_emailed = prefs
        .get(uid, "link_emailed")
        .await
        .map_err(|e| ServerFnError::new(format!("get link_emailed: {e}")))?;
    let expiration_warning = prefs
        .get(uid, "expiration_warning")
        .await
        .map_err(|e| ServerFnError::new(format!("get expiration_warning: {e}")))?;
    Ok(NotificationPrefsDto {
        share_created,
        link_emailed,
        expiration_warning,
    })
}

/// `POST /api/notification_prefs/set` — upsert the calling user's opt-in
/// state for a single event type. The `event_type` is validated against
/// [`KNOWN_EVENT_TYPES`]; unknown values are rejected with a clear error
/// rather than silently written through to the DB.
#[server(endpoint = "api/notification_prefs/set", prefix = "")]
pub async fn notification_prefs_set(
    event_type: String,
    enabled: bool,
) -> Result<(), ServerFnError> {
    use dioxus::fullstack::FullstackContext;
    if !KNOWN_EVENT_TYPES.contains(&event_type.as_str()) {
        return Err(ServerFnError::new(format!(
            "unknown event_type: {event_type:?} (expected one of {KNOWN_EVENT_TYPES:?})"
        )));
    }
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
    state
        .notification_prefs
        .set(ctx.user_id.as_str(), &event_type, enabled)
        .await
        .map_err(|e| ServerFnError::new(format!("set {event_type}: {e}")))?;
    Ok(())
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    #[test]
    fn known_event_types_match_spec() {
        assert_eq!(
            KNOWN_EVENT_TYPES,
            &["share_created", "link_emailed", "expiration_warning"],
        );
    }
}
