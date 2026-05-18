//! OCS endpoints for the activity feed.
//!
//! Nextcloud spelling: `/ocs/v2.php/apps/activity/api/v2/...`.
//! All endpoints require the authed user; the row filter is always the
//! authed uid (no `{uid}` segment — third-party OCS clients don't carry
//! one on this surface).
//!
//! * `GET /activity?since=<id>&limit=<N>` — paginated feed (descending id;
//!   strict `id < since` when `since` is present; `limit` defaults 30,
//!   clamped to `[1, 100]`).
//! * `GET /activity/settings`             — per-event-type stream toggles.
//! * `PUT /activity/settings`             — upsert one toggle.
//!
//! The list payload wraps `{ items, next_since }` inside the standard OCS
//! envelope so a single response carries both the rows and the cursor for
//! the next page. Rows include both the raw `subject_id` + `subject_params`
//! AND the pre-rendered English `subject` string (via
//! [`crabcloud_activity::render_subject`]) so OCS clients without the
//! template catalogue can still display a human-readable line.
//!
//! Envelope helpers live in [`super::envelope`] and are shared with
//! `files_versions.rs` / `files_trashbin.rs` so the OCS wire shape stays
//! single-sourced.

use super::envelope::ocs_envelope;
use crate::auth_context::AuthContext;
use crate::extractors::format::OcsFormat;
use axum::extract::{Query, State};
use axum::response::Response;
use axum::routing::get;
use axum::{Extension, Json};
use crabcloud_activity::{render_subject, ActivityError, ActivityRow, ActivitySetting};
use crabcloud_core::AppState;
use crabcloud_ocs::Format;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub fn router() -> axum::Router<AppState> {
    axum::Router::new()
        .route("/activity", get(list_handler))
        .route("/activity/settings", get(get_settings).put(put_setting))
}

// --- error mapping ---------------------------------------------------------

/// Map `ActivityError` → OCS envelope. The activity surface has no
/// user-actionable error variants today (NotFound never escapes the
/// list/settings paths), so every error is logged at `tracing::error!`
/// and surfaces as a 500. This matches the policy in the versions /
/// trash OCS modules for unexpected error variants.
fn from_activity_error(err: ActivityError, fmt: Format) -> Response {
    tracing::error!(error = %err, "activity OCS handler: unhandled ActivityError");
    ocs_envelope(500, &err.to_string(), Value::Null, fmt)
}

// --- wire DTOs -------------------------------------------------------------

/// Per-entry shape returned in the OCS `data.items` array. Mirrors the
/// user-facing fields of `crabcloud_activity::ActivityRow`; the internal
/// `affected_user` column is dropped (the authed uid is implicit on this
/// surface). The pre-rendered English `subject` is included alongside
/// the raw `subject_id` + `subject_params` so clients without the
/// template catalogue can still display a human-readable line.
#[derive(Serialize)]
struct ActivityRowDto {
    id: i64,
    actor: String,
    event_type: String,
    subject_id: String,
    subject_params: Value,
    subject: String,
    object_type: String,
    object_id: Option<i64>,
    occurred_at: i64,
    last_seen_at: i64,
    count: i32,
}

impl From<ActivityRow> for ActivityRowDto {
    fn from(r: ActivityRow) -> Self {
        let subject = render_subject(&r.subject_id, &r.subject_params);
        Self {
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
    }
}

/// Mirror of `ActivitySetting` for the wire — currently identical, but
/// kept as its own DTO so future fields on the storage side don't leak
/// onto the OCS surface without a deliberate change.
#[derive(Serialize)]
struct ActivitySettingDto {
    event_type: String,
    stream: bool,
}

impl From<ActivitySetting> for ActivitySettingDto {
    fn from(s: ActivitySetting) -> Self {
        Self {
            event_type: s.event_type,
            stream: s.stream,
        }
    }
}

// --- handlers --------------------------------------------------------------

#[derive(Deserialize, Default)]
struct ListQuery {
    since: Option<i64>,
    limit: Option<i64>,
}

async fn list_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    fmt: OcsFormat,
    Query(q): Query<ListQuery>,
) -> Response {
    // Defaults per spec §5.1: limit defaults to 30, max 100.
    let limit = q.limit.unwrap_or(30).clamp(1, 100);
    let rows = match state
        .activity
        .list(ctx.user_id.as_str(), q.since, limit)
        .await
    {
        Ok(r) => r,
        Err(e) => return from_activity_error(e, fmt.0),
    };
    // `next_since` is the id of the last (smallest, since the list is
    // descending) row in the page; the client passes it back as `since`
    // for the next page. None when the page is empty — clients should
    // stop polling when they see no items.
    let next_since = rows.last().map(|r| r.id);
    let items: Vec<ActivityRowDto> = rows.into_iter().map(ActivityRowDto::from).collect();
    let data = serde_json::json!({
        "items": items,
        "next_since": next_since,
    });
    ocs_envelope(200, "OK", data, fmt.0)
}

async fn get_settings(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    fmt: OcsFormat,
) -> Response {
    let rows = match state
        .activity_settings
        .get_all_for_user(ctx.user_id.as_str())
        .await
    {
        Ok(r) => r,
        Err(e) => return from_activity_error(e, fmt.0),
    };
    let settings: Vec<ActivitySettingDto> =
        rows.into_iter().map(ActivitySettingDto::from).collect();
    ocs_envelope(
        200,
        "OK",
        serde_json::json!({ "settings": settings }),
        fmt.0,
    )
}

#[derive(Deserialize)]
struct PutSettingBody {
    event_type: String,
    stream: bool,
}

async fn put_setting(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    fmt: OcsFormat,
    Json(body): Json<PutSettingBody>,
) -> Response {
    match state
        .activity_settings
        .set(ctx.user_id.as_str(), &body.event_type, body.stream)
        .await
    {
        Ok(()) => ocs_envelope(200, "OK", Value::Null, fmt.0),
        Err(e) => from_activity_error(e, fmt.0),
    }
}
