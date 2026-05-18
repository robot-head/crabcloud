# Activity feed — Design (Sub-project 14)

**Status:** spec — design only, no implementation.
**Date:** 2026-05-17
**Sub-project:** 14. Picks up after SP13 file versioning landed (PRs #176–#180). Composes with SP11 mail notifications, SP12 trash, SP13 versioning. Adds a new `crabcloud-activity` crate plus an `oc_activity` + `oc_activity_settings` table pair.

## 1. Goal

Ship Nextcloud-compatible per-user activity feeds. Every file CRUD, share, trash restore, and version restore emits one row per recipient into `oc_activity`. Recipients see their feed via OCS (`/ocs/v2.php/apps/activity/api/v2/activity`), via server fns the Dioxus UI consumes, and via a new "Activity" sidebar entry → activity page with infinite-scroll.

In MVP scope:

- New `crabcloud-activity` crate: `Activity::{emit, list, sweep_expired}`, `ActivitySettings::{get, set, get_all_for_user}`.
- New `ActivityEmitter` trait in `crabcloud-activity` so emitter crates (`crabcloud-fs`, `crabcloud-sharing`, `crabcloud-versions`, `crabcloud-trash`) depend on the trait, not the implementation (mirrors the `MailEnqueuer` precedent from SP11).
- New migration `0011_activity_and_settings` (sqlite + mysql + postgres triplet).
- Emit hooks in `View::{write_file, delete, hard_delete, rename}`, `Trash::restore`, `Shares::{create, delete}`, `Versions::restore`. Each hook calls `Activity::emit` after the underlying operation succeeds.
- Recipient fan-out at emit time: actor + share recipients + group-share members. One row per recipient.
- Event coalescing: successive same-`(affected_user, actor, event_type, object_id)` events within `activity_coalesce_window_secs` (default 600) bump `count + last_seen_at` instead of inserting.
- Per-user-per-event stream opt-out via new `oc_activity_settings` table. Default-true semantics.
- Background `ActivitySweeper` runs daily, deletes rows older than `activity_retention_days` (default 365, matching Nextcloud).
- OCS REST: `GET /ocs/v2.php/apps/activity/api/v2/activity` with cursor pagination + `GET/PUT /ocs/v2.php/apps/activity/api/v2/activity/settings`.
- Server fns: `list_activity(cursor)`, `get_activity_settings()`, `set_activity_setting(event_type, stream)`.
- Dioxus UI: "Activity" sidebar entry → `/activity` route → page with infinite-scroll list + settings sub-route.

Explicitly out of scope (deferred):

- Notification badge / unread counts on the sidebar entry.
- Email digest of activity (the existing mail crate covers per-event mails for shares + expirations; an "X new activity items since yesterday" digest is its own product surface).
- Per-event-type push notifications (no push infra).
- Real-time updates (no SSE / websocket).
- Group-folder activity (no group folders yet).
- Comments / mentions (no comment system).
- Tags / favorites activity (no tag system).
- Activity for filecache scanner discoveries (only user-driven actions emit; scanner-driven sync is operator concern).
- Localization of subject templates beyond English (the structure is i18n-ready — `subject_id` + `subject_params` — but only English templates ship in MVP).
- Aggregation across days ("Alice updated 3 files yesterday"). Coalescing covers within-window same-object; cross-object aggregation is future.

## 2. Load-bearing decisions (brainstormed)

| # | Decision | Rationale |
|---|---|---|
| 1 | **DB-backed event log `oc_activity`** with one row per (recipient, event). Reads filter by `affected_user + occurred_at` window via the `(affected_user, occurred_at DESC)` index. | Matches Nextcloud's `oc_activity` layout byte-for-byte. Reads are O(window) regardless of share fan-out. Trade-off vs single-row + read-time join: more writes for many-recipient events; much simpler reads. |
| 2 | **Fan-out at emit time** — `Activity::emit` resolves recipients (actor + share recipients + group members) and writes one row per recipient. Group membership is point-in-time (resolved at emit; future group changes don't backfill). | Matches Nextcloud. Read perf scales with feed length, not share-graph complexity. New group members joining a group don't see historical activity — feels right per Nextcloud's behavior. |
| 3 | **`ActivityEmitter` trait in `crabcloud-activity`** for emitter crates to depend on instead of the implementation. The trait shape: `async fn emit(&self, event: ActivityEvent) -> Result<(), ActivityEmitError>`. `crabcloud-activity::Activity` impls the trait; emitter crates take `Arc<dyn ActivityEmitter>`. | Mirrors the `MailEnqueuer` precedent from SP11. Lets `crabcloud-sharing` etc. depend on `crabcloud-activity` (for the trait) without a cycle if `crabcloud-activity` ever needs to look at shares — which it does, for recipient resolution. The cycle question: `Activity::emit` itself needs to know share recipients, which means it needs to call into `crabcloud-sharing`. We invert: the emitter site (sharing, fs, etc.) computes the recipient set as part of the event and passes it in. `crabcloud-activity` accepts the recipient list verbatim. No share lookup in the activity crate. |
| 4 | **Recipient resolution happens at the emit site, not in `crabcloud-activity`.** Emitters know the context: `Shares::create` for a group share already iterates group members; `View::write_file` already has the mount → owner_uid + share recipients available via the mount metadata. Each call site composes the `Vec<UserId>` and passes it as part of `ActivityEvent`. | Avoids the `crabcloud-activity` → `crabcloud-sharing` dependency that would otherwise be needed for group expansion. Keeps `crabcloud-activity` focused on persistence + coalesce + read. |
| 5 | **Coalesce window**: same `(affected_user, actor, event_type, object_id)` within `activity_coalesce_window_secs` (default 600) bumps `count + last_seen_at`. Implemented via `idx_activity_coalesce` (composite index on those four fields + `last_seen_at`). | Matches Nextcloud's grouping behavior. Default 10 min reads well on autosave-heavy editors. Worth flagging: the throttle window is intentionally larger than the versions throttle (default 2 s) — versions throttle prevents duplicate snapshots within seconds; activity coalesce groups for human-readable feed display over minutes. |
| 6 | **Coalesce race**: two concurrent emits in the same window may both pass the SELECT and both INSERT. No UNIQUE constraint enforces single-row-per-window. | Accept the rare duplicate. Adding a unique key + tx + UPSERT (and per-dialect upsert plumbing) is not worth it for a low-frequency race that resolves itself with the sweeper. Document in the sweeper docstring so a future pass can decide differently. |
| 7 | **Settings table `oc_activity_settings`** keyed `(user_id, event_type)` with a `stream` BOOLEAN. Defaults to TRUE when no row exists. Lookup happens once per recipient at emit time. | Separate from `oc_user_notification_prefs` (SP11) because Nextcloud cleanly separates the "stream" (activity feed) channel from the "email" / "notification" channels. A user can want the email AND skip the activity row, or vice versa. Reusing the mail prefs would conflate the two. |
| 8 | **Subject templates** stored as `subject_id` (i18n key) + `subject_params` (JSON). MVP ships an English-only `subject_id → template` map in `crabcloud-activity/src/subjects.rs`; templates use `{actor}` / `{file}` etc. placeholders. The OCS surface returns both `subject_id` + `subject_params` + rendered `subject`. | Future translation drops in via `crabcloud-i18n` without breaking the wire shape. Clients that want their own rendering use raw `subject_id` + `subject_params`; clients that want a ready-to-display string read `subject`. |
| 9 | **Two perspectives per event**: same logical action produces different subjects for the actor vs the recipient. Bob updates Alice's shared file → Bob's row uses `file_updated_you`/`"You updated {file}"`, Alice's row uses `file_updated_by`/`"{actor} updated {file}"`. | Reads naturally in the UI without per-row string-rewriting. The emit site decides which subject_id to use based on `recipient == actor`. |
| 10 | **Background sweeper** `ActivitySweeper` daily, deletes rows where `occurred_at < now - retention`. `activity_retention_days = 0` short-circuits. Mirrors the `TrashSweeper`/`VersionsSweeper` shape. | Bounded growth; same `Notify`-shutdown pattern; same `sweep_once()` for sync test drive. |
| 11 | **OCS surface** at `/ocs/v2.php/apps/activity/api/v2/activity` — Nextcloud spelling. `GET` returns a paginated list with `?since=<id>&limit=N` cursor pagination. `GET /settings` returns the user's settings; `PUT /settings` upserts a setting. | Standard apps-API namespacing. Nextcloud third-party clients (mobile apps) work without translation. |
| 12 | **Cursor pagination via descending `id`**, not `occurred_at` — Activity ids are monotonically increasing per dialect (AUTOINCREMENT / BIGSERIAL); using `id` for the cursor sidesteps timestamp ties and gives stable ordering. Response includes `next_since` for follow-on requests. | Standard pagination shape; cheap because `(affected_user, id)` is a natural index extension. |
| 13 | **No mark-read / unread state in MVP.** The feed is a log; reading is the act of viewing. Adding read state means a per-(user, event) bit-set or last-read cursor — out of scope. | Matches Nextcloud's default activity panel. Notification-badge unread counts are a separate feature on top. |
| 14 | **Dioxus UI**: new sidebar entry "Activity" alongside "Deleted files"; clicking routes to `/activity`. Page renders a list with infinite scroll (load-more button on first MVP, IntersectionObserver later). Each row shows: actor avatar/initials, subject string, relative timestamp, count badge when `count > 1`. Settings live at `/activity/settings` (or as a sidebar within the activity page) with one toggle per event type. | Mirrors the trash + versions UI patterns — sidebar entry + dedicated page + settings sub-route. |

## 3. Architecture

```
[emit sites]
 ├─ View::{write_file, delete, hard_delete, rename}     ┐
 ├─ Trash::restore                                       │
 ├─ Shares::{create, delete}                             │ Activity::emit(ActivityEvent)
 ├─ Versions::restore                                    ┘
                                                          │
                                                          ▼
                                                   Activity (impl ActivityEmitter)
                                                   ├─ for each recipient in event.recipients:
                                                   │    ├─ ActivitySettings::stream_enabled(recipient, event_type)?
                                                   │    │     no -> skip
                                                   │    ├─ coalesce_check by (recipient, actor, event_type, object_id)
                                                   │    │     hit  -> UPDATE row count + last_seen_at
                                                   │    │     miss -> INSERT row
                                                   │    └─ done
                                                   └─ return Ok(())

[surfaces]
 ├─ OCS GET /apps/activity/api/v2/activity?since=<id>&limit=N
 │   └─ Activity::list(affected_user, since, limit)
 ├─ OCS GET/PUT /apps/activity/api/v2/activity/settings
 │   └─ ActivitySettings::{get_all_for_user, set}
 ├─ Server fns: list_activity(cursor), get_activity_settings(), set_activity_setting(...)
 └─ Dioxus UI: /activity page + /activity/settings sub-route

[background]
 └─ ActivitySweeper (daily): Activity::sweep_expired(now - retention)

crabcloud-activity  (NEW crate)
 ├─ ActivityEmitter trait (used by emitter crates)
 ├─ Activity struct + impl ActivityEmitter
 ├─ ActivitySettings struct
 ├─ ActivityEvent { actor, event_type, subject_id, subject_params, object_type, object_id, recipients: Vec<UserId> }
 ├─ ActivityRow (read DTO)
 ├─ subjects::render(subject_id, params) -> String  (English MVP)
 ├─ Multidialect SQL via match self.pool.as_ref()
 └─ Depends on: crabcloud-db, crabcloud-users (for UserId)

AppState additions
 ├─ activity: Arc<crabcloud_activity::Activity>
 ├─ activity_settings: Arc<crabcloud_activity::ActivitySettings>
 └─ activity_sweeper_shutdown: Arc<tokio::sync::Notify>
```

## 4. Schema

```sql
CREATE TABLE oc_activity (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,   -- BIGSERIAL pg, BIGINT AUTO_INCREMENT mysql
    affected_user   VARCHAR(64)  NOT NULL,               -- fan-out target uid
    actor           VARCHAR(64)  NOT NULL,               -- "" for system/public-link
    event_type      VARCHAR(64)  NOT NULL,               -- file_created, share_created, ...
    subject_id      VARCHAR(128) NOT NULL,               -- i18n key
    subject_params  TEXT         NOT NULL,               -- JSON object
    object_type     VARCHAR(32)  NOT NULL,               -- file | share | version
    object_id       BIGINT       NULL,                   -- fileid / share id / version id
    occurred_at     BIGINT       NOT NULL,               -- unix seconds, first occurrence
    last_seen_at    BIGINT       NOT NULL,               -- unix seconds, last occurrence
    count           INTEGER      NOT NULL DEFAULT 1
);

CREATE INDEX idx_activity_user_time ON oc_activity (affected_user, occurred_at DESC);
CREATE INDEX idx_activity_coalesce  ON oc_activity (affected_user, actor, event_type, object_id, last_seen_at);

CREATE TABLE oc_activity_settings (
    user_id     VARCHAR(64) NOT NULL,
    event_type  VARCHAR(64) NOT NULL,
    stream      BOOLEAN     NOT NULL DEFAULT TRUE,
    PRIMARY KEY (user_id, event_type)
);
```

## 5. Surface contracts

### 5.1 OCS — `/ocs/v2.php/apps/activity/api/v2/activity`

| Method | Path | Behavior |
|---|---|---|
| GET | `/activity?since=<id>&limit=<N>` | Returns the authed user's activity feed in descending-id order, with id strictly less than `since` (or no filter if `since` absent). `limit` defaults 30, max 100. Response envelope: `{ ocs: { meta, data: [ActivityRowDto] }, next_since?: <id> }`. |
| GET | `/activity/settings` | Returns all per-event toggles for the authed user. Missing entries default to `stream: true`. |
| PUT | `/activity/settings` | Body `{ event_type, stream }`. Upserts the row. |

`ActivityRowDto`: `{ id, actor, event_type, subject_id, subject_params, subject, object_type, object_id, occurred_at, last_seen_at, count }`. `subject` is the rendered English string for now.

### 5.2 Server-fn API (Dioxus)

```rust
#[server]
pub async fn list_activity(since: Option<i64>, limit: Option<i64>)
    -> Result<ListActivityResponse, ServerFnError>;

#[server]
pub async fn get_activity_settings()
    -> Result<Vec<ActivitySettingDto>, ServerFnError>;

#[server]
pub async fn set_activity_setting(event_type: String, stream: bool)
    -> Result<(), ServerFnError>;
```

`ListActivityResponse`: `{ items: Vec<ActivityRowDto>, next_since: Option<i64> }`. All gated by `require_user()`.

## 6. Edge cases

| Case | Behavior |
|---|---|
| **Shared-with-me edit** | Alice updates `/shared/report.docx`. `View::write_file` resolves recipients = [alice] + share recipients (bob, group-foo expanded). Each gets a row. Alice's `subject_id` is `*_you`, Bob's is `*_by`. |
| **Public-link write** | Anonymous edit → actor = "". Recipients = owner only. Subject renders "Someone updated {file} via a shared link". |
| **Public-link delete** | Same shape — actor "", recipients = owner only. |
| **Share-mount delete (Bob deletes from Alice's share)** | `View::delete` already places trash in Bob's bin per SP12. Activity emits with actor=bob, recipients = [bob, alice]. Both see the deletion. |
| **Group share fan-out** | `Shares::create` for a group → recipients = current group members (resolved at emit time, point-in-time). New members joining later don't see historical activity. |
| **Coalesce within window** | Same `(affected_user, actor, event_type, object_id)` within `activity_coalesce_window_secs` → UPDATE the matching row (`count += 1`, `last_seen_at = now`, latest `subject_params`). |
| **Coalesce race** | Two concurrent emits inside the window may both INSERT. Accepted (low-frequency, self-resolves with sweeper). |
| **Stream opt-out** | `oc_activity_settings.stream = false` → skip INSERT for that recipient. The actor's OWN row is exempt from opt-out (you always see your own actions, even if you've muted the event type globally — Nextcloud parity). |
| **Object deleted between emit and read** | The row stays — activity is a historical log. `object_id` may point at a now-nonexistent fileid; the UI tolerates this and renders the historical subject. |
| **Share deleted between emit and read** | Same — activity row persists. |
| **Sweeper retention = 0** | Sweeper short-circuits (returns Ok(0)). Activity grows forever. |
| **Subject_id missing from template map** | Render falls back to `subject_id` verbatim (e.g. `"file_updated_by"`); `tracing::warn!` so an operator can spot it. |
| **Group share with 1000 members** | 1000 INSERTs in a loop. Acceptable for MVP; if it becomes hot, the emit can switch to multi-row INSERT or move to a background task. Document the linear cost in the impl comment. |
| **Emit failure** | `Activity::emit` returns `Err`. Each emit site decides: `View::write_file`, `Shares::create`, etc. **log + continue** — activity is best-effort, the user's write must not fail because the activity log is down. Pattern matches the SP11 mail-enqueue handling. |

## 7. Testing

- **Unit (`crabcloud-activity`)**: emit writes one row per recipient; coalesce within window bumps count + last_seen_at; coalesce outside window inserts new; stream opt-out skips recipient (but never the actor); recipient list with duplicates de-dupes; subject rendering with various params.
- **E2E sqlite**: full emit → list → cursor-paginate round-trip; sweeper expires aged rows; coalesce race produces at most 2 rows (no INSERT failure).
- **`crabcloud-fs::view`**: `View::write_file` emits to actor + share recipients (verified by reading `Activity::list` for each).
- **`crabcloud-sharing`**: `Shares::create` for user share emits to actor + target. For group share, emits to actor + every group member.
- **`crabcloud-trash`**: `Trash::restore` emits `file_restored` to actor.
- **`crabcloud-versions`**: `Versions::restore` emits `version_restored` to owner.
- **OCS e2e**: list with cursor pagination; settings GET/PUT.
- **Server-fn integration**: round-trip list + settings.
- **UI**: SSR snapshot of the activity page (3 events including a coalesced one with count=5).

## 8. Batches (implementation order)

A. **Core + emit hooks + sweeper** — new `crabcloud-activity` crate, migration `0011_activity_and_settings`, `Activity::{emit, list, sweep_expired}`, `ActivitySettings::{get, set, get_all_for_user}`, `ActivityEmitter` trait, subject template map, coalescing logic, recipient-list de-dup, `ActivitySweeper` background task, `activity_retention_days` + `activity_coalesce_window_secs` config knobs, AppState wiring. Emit hooks in `View::{write_file, delete, hard_delete, rename}`, `Trash::restore`, `Shares::{create, delete}`, `Versions::restore`. Cross-crate emitter wiring via `Arc<dyn ActivityEmitter>` (mirrors `MailEnqueuer` precedent). Comprehensive unit + e2e + cross-crate tests.
B. **OCS REST** — `/ocs/v2.php/apps/activity/api/v2/activity` GET with cursor pagination; `/activity/settings` GET/PUT. Reuses shared OCS envelope helpers.
C. **Server fns** — `list_activity`, `get_activity_settings`, `set_activity_setting`. Integration test mirroring `server_fns_trash.rs`.
D. **UI** — "Activity" sidebar entry, `/activity` route with infinite-scroll list, `/activity/settings` route with per-event-type toggles. SSR snapshot tests.

Each batch ships as one PR through subagent-driven-development with the standard two-stage review (spec compliance → code quality).
