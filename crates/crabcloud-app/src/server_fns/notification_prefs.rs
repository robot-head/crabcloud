//! Server fns for the per-user notification-preferences panel.
//!
//! `POST /api/notification_prefs/get` returns the user's current opt-in state
//! for each of the three notification event types (`share_created`,
//! `link_emailed`, `expiration_warning`). Default-true semantics: when no
//! row is stored, the user is considered opted in.
//!
//! `POST /api/notification_prefs/set` upserts the opt-in state for a single
//! event type. The `event_type` string is validated against the local
//! [`KNOWN_EVENT_TYPES`] whitelist; unknown values are rejected before any
//! DB write.
//!
//! `KNOWN_EVENT_TYPES` is hand-maintained here rather than imported from
//! `crabcloud_mail::EventType` because `crabcloud-mail` is deliberately kept
//! out of the WASM dep graph (it pulls the SMTP stack). To prevent the two
//! lists from silently drifting, a host-only parity test
//! (`parity_tests::known_event_types_match_crabcloud_mail_eventtype`) cross-
//! checks the local allowlist against `EventType::from_str` and fails loudly
//! if either side accepts a string the other doesn't.
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
/// keeps the set-validation in one place. Only referenced server-side by
/// `notification_prefs_set`, so we gate the constant on the `server`
/// feature to avoid an unused-item warning on the WASM build.
#[cfg(feature = "server")]
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
#[cfg(not(target_arch = "wasm32"))]
mod parity_tests {
    use super::KNOWN_EVENT_TYPES;

    /// Probe set: every event type the server allowlist knows about, plus
    /// some known-bad strings. Both sides must agree.
    const SHARED_PROBES: &[(&str, bool)] = &[
        ("share_created", true),
        ("link_emailed", true),
        ("expiration_warning", true),
        // Negative cases — neither side should accept these.
        ("password_reset", false),
        ("upload_completed", false),
        ("", false),
        ("SHARE_CREATED", false), // case-sensitive
    ];

    #[test]
    fn known_event_types_match_crabcloud_mail_eventtype() {
        for (probe, expected) in SHARED_PROBES {
            let client_accepts = KNOWN_EVENT_TYPES.contains(probe);
            let mail_accepts = crabcloud_mail::EventType::from_str(probe).is_some();
            assert_eq!(
                client_accepts, *expected,
                "client allowlist disagrees with probe expectation for {probe:?}",
            );
            assert_eq!(
                mail_accepts, *expected,
                "crabcloud_mail::EventType::from_str disagrees with probe expectation for {probe:?}",
            );
            assert_eq!(
                client_accepts, mail_accepts,
                "client allowlist ({client_accepts}) and crabcloud_mail ({mail_accepts}) disagree for {probe:?}",
            );
        }
    }
}
