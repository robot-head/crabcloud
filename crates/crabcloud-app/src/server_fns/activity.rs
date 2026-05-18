//! `#[server]` functions for the Dioxus activity-feed UI. Mirrors the
//! OCS surface (`/ocs/v2.php/apps/activity/api/v2/...`) but with typed
//! inputs / outputs the UI can call directly without round-tripping
//! through the OCS JSON envelope.
//!
//! Auth: the request runs through the production `AuthLayer`, so the
//! `AuthContext` extension is always present for authenticated callers
//! and the [`super::require_user`] helper hands the body a
//! `(AppState, UserId)` pair. Unauthenticated callers fall through
//! anonymous and the helper short-circuits with `unauthorized`.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// One activity row, returned by [`list_activity`]. Mirrors the OCS
/// `ActivityRowDto` field-for-field — the pre-rendered English
/// `subject` is included alongside the raw `subject_id` +
/// `subject_params` so the UI can either render its own template or
/// fall back to the server-rendered string. The internal
/// `affected_user` column is dropped (the authed uid is implicit on
/// this surface).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActivityRowDto {
    pub id: i64,
    pub actor: String,
    pub event_type: String,
    pub subject_id: String,
    pub subject_params: serde_json::Value,
    pub subject: String,
    pub object_type: String,
    pub object_id: Option<i64>,
    pub occurred_at: i64,
    pub last_seen_at: i64,
    pub count: i32,
}

/// Response payload for [`list_activity`]. `next_since` is the id of
/// the last (smallest, since the list is descending) row in the page;
/// callers pass it back as `since` for the next page. `None` when the
/// page is empty — callers should stop polling when they see no items.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListActivityResponse {
    pub items: Vec<ActivityRowDto>,
    pub next_since: Option<i64>,
}

/// One per-event-type stream toggle, returned by
/// [`get_activity_settings`]. Mirrors `crabcloud_activity::ActivitySetting`
/// — kept as its own DTO so future storage-side fields don't leak onto
/// the server-fn surface without a deliberate change.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActivitySettingDto {
    pub event_type: String,
    pub stream: bool,
}

/// `POST /api/files/activity/list` — return the authed user's activity
/// feed in descending id order, with `id < since` when `since` is
/// present. `limit` defaults to 30, clamped to `[1, 100]` (matches the
/// OCS surface in `routes/ocs/activity.rs`).
#[server(endpoint = "api/files/activity/list", prefix = "")]
pub async fn list_activity(
    since: Option<i64>,
    limit: Option<i64>,
) -> Result<ListActivityResponse, ServerFnError> {
    use crabcloud_activity::render_subject;
    let (state, uid) = super::require_user().await?;
    let limit = limit.unwrap_or(30).clamp(1, 100);
    let rows = state
        .activity
        .list(uid.as_str(), since, limit)
        .await
        .map_err(map_activity_err)?;
    let next_since = rows.last().map(|r| r.id);
    let items = rows
        .into_iter()
        .map(|r| {
            let subject = render_subject(&r.subject_id, &r.subject_params);
            ActivityRowDto {
                id: r.id,
                actor: r.actor,
                event_type: r.event_type,
                subject_id: r.subject_id,
                subject_params: r.subject_params,
                subject,
                object_type: r.object_type,
                object_id: r.object_id,
                occurred_at: r.occurred_at,
                last_seen_at: r.last_seen_at,
                count: r.count,
            }
        })
        .collect();
    Ok(ListActivityResponse { items, next_since })
}

/// `POST /api/files/activity/settings` — return every per-event-type
/// stream toggle the authed user has explicitly set. Missing entries
/// default to `stream: true` on the read side (the UI fills in the
/// default for event types absent from this list).
#[server(endpoint = "api/files/activity/settings", prefix = "")]
pub async fn get_activity_settings() -> Result<Vec<ActivitySettingDto>, ServerFnError> {
    let (state, uid) = super::require_user().await?;
    let rows = state
        .activity_settings
        .get_all_for_user(uid.as_str())
        .await
        .map_err(map_activity_err)?;
    Ok(rows
        .into_iter()
        .map(|s| ActivitySettingDto {
            event_type: s.event_type,
            stream: s.stream,
        })
        .collect())
}

/// `POST /api/files/activity/settings/put` — upsert the
/// `(authed_user, event_type)` stream toggle. The OCS surface uses
/// `PUT /activity/settings` with a JSON body; the server-fn surface
/// has no HTTP verb constraint and so just takes the two fields as
/// positional inputs.
#[server(endpoint = "api/files/activity/settings/put", prefix = "")]
pub async fn set_activity_setting(event_type: String, stream: bool) -> Result<(), ServerFnError> {
    let (state, uid) = super::require_user().await?;
    state
        .activity_settings
        .set(uid.as_str(), &event_type, stream)
        .await
        .map_err(map_activity_err)
}

/// Map the activity service's typed error to the string-bodied
/// `ServerFnError` the dx client surface understands. The activity
/// surface has no user-actionable error variants today (NotFound never
/// escapes the list/settings paths), so every error is logged at
/// `tracing::error!` and surfaces as a generic message — mirrors the
/// policy in the OCS module (`routes/ocs/activity.rs`).
#[cfg(feature = "server")]
fn map_activity_err(err: crabcloud_activity::ActivityError) -> ServerFnError {
    tracing::error!(error = %err, "activity server fn: unhandled ActivityError");
    ServerFnError::new(format!("activity: {err}"))
}
