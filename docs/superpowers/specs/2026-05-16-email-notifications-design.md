# Email notifications — Design (Sub-project 11)

**Status:** spec — design only, no implementation.
**Date:** 2026-05-16
**Sub-project:** 11 of ~13. Final SP of the SP9–SP11 trio originally split off after SP8. Builds on SP8's `oc_share.mail_send` column (which has been wired-but-unread until now) and the existing `crabcloud-users` / `crabcloud-i18n` infrastructure.

## 1. Goal

Ship the mailer infrastructure and three notification flows:

1. **`share_created`** — when alice shares with bob, bob gets an "alice shared X with you" email.
2. **`link_emailed`** — when alice creates an email-share (`share_type=4`), the recipient gets the link URL via email.
3. **`expiration_warning`** — public-link owners get a T-1 day warning before a link expires.

A new `crabcloud-mail` crate owns SMTP transport (lettre + rustls), template rendering (tera), and a DB-backed mail queue with retry. A background worker drains the queue. A daily-ish scheduler (hourly sweep) runs the expiration warning pass. Per-event-type opt-out via a new `oc_user_notification_prefs` table.

**In scope:**

- New `crabcloud-mail` crate: SMTP transport, template engine, `MailEnvelope` types, `Mailer::send_template`.
- New `oc_mail_queue` table + migration: persistent queue with `attempts` / `next_attempt_at` / `state` columns. Worker drains it.
- New `oc_user_notification_prefs` table: per-event-type opt-out (3 event types in MVP).
- New `oc_share.last_warned` column: prevents re-warning on the expiration sweep.
- Extended `ShareType::Email` (`share_type=4`): creates a public link AND enqueues a recipient email.
- Notification hooks in `Shares::create` (user/group → share_created; email-link → link_emailed) and a daily scheduler (expiration_warning).
- Templates for the 3 events, HTML + plaintext multipart, i18n via existing `crabcloud-i18n`.
- Account-settings UI to toggle each event type.
- E2E + unit tests on each layer.

**Explicitly out of scope (deferred):**

- Password-reset / welcome / email-verified flows (no current password-reset infrastructure).
- Group-share notifications (fan-out concern; ship 1:1 first, group later).
- File-drop "someone uploaded to your drop zone" notification.
- Storage quota warnings.
- Digest mode / batching ("daily summary of 12 share notifications").
- Federated email-share between Crabcloud instances.
- Operator-overridable templates at `<datadir>/mail-templates/` — covered in §3 architecture comments but not in the MVP test surface; lazy-load is straightforward to add post-MVP.

## 2. Load-bearing decisions (brainstormed)

| # | Decision | Rationale |
|---|---|---|
| 1 | **New crate `crabcloud-mail`**: lettre (`builder`, `smtp-transport`, `tokio1-rustls-tls`) + tera + thiserror + serde. Same shape as `crabcloud-zip` and `crabcloud-preview` — focused presentation/transport crate, no DB coupling. The queue layer (which depends on DB) lives separately in `crabcloud-core` so `crabcloud-mail` stays DB-free. | Mirrors the established crate-sizing pattern from SP9 / SP10. |
| 2 | **lettre with rustls.** Implicit TLS (`smtps://`), STARTTLS, and plain transport all supported. Plain + LOGIN + XOAUTH2 auth. | Consistent with the rest of the workspace's rustls stack; covers every real-world provider (Gmail, SES, Postfix, in-network MTA). |
| 3 | **Tera for templates**, embedded via `rust-embed` so single-binary deploys still work. Operator can override at `<datadir>/mail-templates/` (loaded lazily at startup if directory exists). Templates live under `crates/crabcloud-mail/templates/{event}.{html,txt}`. | Mature, Jinja-like, supports inheritance + macros for header/footer partials. Embedding keeps the binary self-contained. |
| 4 | **DB-backed queue**: new `oc_mail_queue` table. Columns: `id BIGSERIAL`, `recipient TEXT NOT NULL`, `subject TEXT NOT NULL`, `html_body TEXT NOT NULL`, `text_body TEXT NOT NULL`, `event_type TEXT NOT NULL`, `attempts INT NOT NULL DEFAULT 0`, `next_attempt_at TIMESTAMP NOT NULL`, `state TEXT NOT NULL DEFAULT 'Pending'`, `claimed_at TIMESTAMP NULL`, `last_error TEXT NULL`, `created_at TIMESTAMP NOT NULL`, `sent_at TIMESTAMP NULL`. | Survives restarts; backpressure is implicit (rows pile up if SMTP is slow). Backoff schedule `[60, 300, 1800]` (1m / 5m / 30m) per `attempts`, max 3 retries. |
| 5 | **`MailWorker::run()`** background task: claims a batch of 8 rows per cycle via `SELECT ... FOR UPDATE SKIP LOCKED LIMIT 8` (Postgres / MySQL) or single-row select (sqlite, single-node). Updates state to `Sending` on claim, `Sent` on success, `Pending` with `next_attempt_at` bumped on transient failure, `Failed` after 3 attempts. Sleeps 5s between empty drains. Spawned in `AppStateBuilder::build()` only when `mail.transport != "disabled"`. | One queue, one worker. No fan-out parallelism in MVP. Easy to reason about. |
| 6 | **`mail.transport` config**: `"smtp"` (production), `"log"` (dev — write envelopes to a tracing event instead of sending), `"disabled"` (worker doesn't spawn; calls to enqueue silently no-op). | Tests pin to `"log"` and assert via the tracing subscriber. Operators can disable mail entirely in air-gapped deployments. |
| 7 | **`FileConfig::mail`** nested section: `transport`, `smtp_host`, `smtp_port`, `smtp_username` (Optional), `smtp_password` (Optional, SecretString), `smtp_security` (`tls` / `starttls` / `none`), `mail_from` (required if transport=smtp), `mail_from_name`. Required fields validated at startup. | Standard config shape; `SecretString` matches the existing `secret` and `dbpassword` patterns. |
| 8 | **Per-event-type opt-out**: new `oc_user_notification_prefs` table `(user_id TEXT, event_type TEXT, enabled BOOLEAN, PRIMARY KEY (user_id, event_type))`. Default = `true` when no row exists. Mailer queries it before enqueueing. Three MVP event types: `share_created`, `link_emailed`, `expiration_warning`. | New event types in future SPs are additive (no migration). Default-true keeps existing users opted in. |
| 9 | **`ShareType::Email` (`share_type=4`)**: alias of `Link` for storage; on create, ALSO enqueues a "you've been shared X" email to `share_with` (treated as the recipient's email address, not a uid). Includes the link URL + a hint that the password (if any) was sent separately. | Matches Nextcloud's OCS shape exactly. The link's `password` and `expiration` fields work identically. Sending the password in the same email is a security anti-pattern; we omit it from the body and tell the recipient to ask the sender. |
| 10 | **Expiration warning scheduler**: a `tokio::spawn`'d task started by `AppStateBuilder::build()`. Wakes every hour, queries `oc_share` for `share_type IN (3,4) AND expiration BETWEEN now() AND now()+24h AND last_warned IS NULL`, enqueues one mail per row, stamps `last_warned = now()` regardless of opt-out result. Idempotent across restarts (the `last_warned IS NULL` filter prevents re-warning). | Matches Nextcloud's daily `ExpireSharesJob` semantics (T-1 day). Hourly granularity (instead of daily) catches links created less than 24h before expiration. |
| 11 | **Templates emit multipart MIME** (`alternative` part with `text/html` + `text/plain`). i18n: a Tera context variable `t` is the i18n translator function, looked up at render time. Default language picked from the user's preferences (if available) or `FileConfig::default_language`. Tera's auto-escape covers HTML XSS; plaintext templates skip escape. | Universal client support (HTML + plain). i18n keeps templates language-agnostic. |
| 12 | **Settings UI**: new section in the user settings page (`pages/settings/notifications.rs`) with 3 toggles. Each toggle hits a new server-fn that upserts a row in `oc_user_notification_prefs`. | Minimal UI: three labeled checkboxes + Save. New event types add new rows here. |

## 3. Architecture

```
Files UI / OCS handlers
 ├─ POST /ocs/.../shares  (share_type=0|1, user/group)
 │    └─ Shares::create → enqueue share_created mail (if recipient opted in)
 ├─ POST /ocs/.../shares  (share_type=4, email-link)
 │    └─ Shares::create → enqueue link_emailed mail to share_with
 └─ Settings page → server_fn upserts oc_user_notification_prefs row

Server
 ├─ crabcloud-mail  (NEW crate)
 │   ├─ MailEnvelope { to: Email, subject: String, html_body: String, text_body: String }
 │   ├─ Templates: Tera + rust-embed of templates/{share_created,link_emailed,expiration_warning}.{html,txt}
 │   ├─ Mailer::new(transport, from, from_name) — entry point used by Worker only
 │   ├─ Transport: Smtp(AsyncSmtpTransport<Tokio1Executor>) | Log | Disabled
 │   ├─ Mailer::send(env) — actually sends via lettre or logs via tracing
 │   ├─ render_template(event: EventType, ctx: TemplateContext) -> RenderedMail
 │   ├─ EventType: ShareCreated | LinkEmailed | ExpirationWarning
 │   └─ TemplateContext: i18n translator + event-specific fields
 │
 ├─ MailQueue  (crates/crabcloud-core/src/mail_queue.rs)
 │   ├─ enqueue(envelope, event_type) -> Result<i64>  (returns queue row id)
 │   ├─ claim_batch(limit) -> Vec<MailQueueRow>  (per-dialect FOR UPDATE SKIP LOCKED;
 │   │                                            sqlite degrades to a single locked row)
 │   ├─ mark_sent(id)
 │   ├─ mark_failed_retry(id, err, next_attempt_at) — `attempts < 3`
 │   ├─ mark_failed_permanent(id, err) — `attempts == 3`, state → Failed
 │   ├─ reclaim_stuck() — rows in Sending with claimed_at < now()-5m flip back to Pending
 │   └─ Queries against oc_mail_queue (3 dialects)
 │
 ├─ MailWorker  (background task spawned by AppStateBuilder::build)
 │   ├─ loop:
 │   │   1. queue.reclaim_stuck() (every 5th cycle)
 │   │   2. queue.claim_batch(8) → if empty, sleep 5s
 │   │   3. for each row: mailer.send → mark_sent OR mark_failed(retry-after)
 │   └─ Skipped when config.mail.transport == "disabled"
 │
 ├─ ExpirationWarningSweeper  (background task spawned alongside MailWorker)
 │   └─ Every 1h: SELECT shares WHERE share_type IN (3,4) AND expiration
 │      BETWEEN now() AND now()+24h AND last_warned IS NULL.
 │      For each: look up owner email + prefs; if opted in, render template + enqueue.
 │      UPDATE last_warned regardless of opt-out result.
 │
 ├─ NotificationPrefs  (new module in crabcloud-users)
 │   ├─ get(uid, event_type) -> bool  (defaults true)
 │   ├─ set(uid, event_type, enabled)
 │   └─ Queries against oc_user_notification_prefs (3 dialects)
 │
 └─ Shares service hooks (extension in crabcloud-sharing)
     ├─ create() for share_type=0|1: after row insert, if recipient's
     │   prefs allow share_created AND recipient has an email, enqueue mail.
     ├─ create() for share_type=4: after row insert, enqueue mail to
     │   share_with (treated as email address, not uid).
     └─ All mail-related side effects are best-effort — failure to enqueue
        is logged + dropped (the share itself succeeds).
```

### 3.1 Data flow — alice shares a folder with bob (`share_type=0`)

1. alice's browser POSTs to `/ocs/v2.php/apps/files_sharing/api/v1/shares` with `shareType=0&shareWith=bob&path=/Vacation`.
2. Existing handler creates the row via `Shares::create`.
3. Inside `Shares::create`, after a successful insert: look up bob's email + notification prefs.
4. If bob has an email AND `prefs.share_created == true`: render `share_created.{html,txt}` with context `{ owner_display_name: "Alice", path_basename: "Vacation", instance_url: "https://crabcloud.example", recipient_display_name: "Bob", t: <i18n> }`, then `MailQueue::enqueue(envelope, ShareCreated)`.
5. The OCS create handler returns 200 to alice immediately.
6. `MailWorker` (background) picks up the row on its next 5-second cycle, sends via lettre, marks `Sent`.

### 3.2 Data flow — email-link share (`share_type=4`)

1. alice posts to OCS with `shareType=4&shareWith=charlie@example.com&path=/Photos`.
2. `Shares::create` dispatches to the link-create path (Batch B in SP8), generating a token + persisting the row with `share_with = "charlie@example.com"` (NOT a uid) and `share_type = 4`.
3. After insert: render `link_emailed.{html,txt}` with `{ owner_display_name: "Alice", link_url: "https://host/s/<token>", path_basename: "Photos", expiration: "2026-06-15", password_protected: <bool>, t: <i18n> }`. Enqueue.
4. No opt-out check — `charlie@example.com` is not a registered user (or they may be, but the OCS contract is "send a link to this address regardless of internal prefs").

### 3.3 Data flow — expiration warning

1. Hourly: the sweeper runs `SELECT id, uid_owner, file_target, token, expiration FROM oc_share WHERE share_type IN (3,4) AND expiration IS NOT NULL AND expiration > now() AND expiration <= now()+24h AND last_warned IS NULL`.
2. For each row: look up the owner's email + prefs. If opted in: enqueue `expiration_warning` mail (`{ link_basename, link_url, expiration_dt }`).
3. `UPDATE oc_share SET last_warned = now() WHERE id = ?` regardless of opt-out result (so we don't re-check the prefs every hour).
4. Worker picks up the queued envelopes on its normal cycle.

### 3.4 Data flow — mail worker retry on transient SMTP failure

1. Worker claims a batch of 8 rows, sets state = `Sending`.
2. For one row, `mailer.send(env)` returns `Err(lettre::transport::smtp::Error::Permanent(_))`. Worker marks `Failed` with `last_error`.
3. For another row, returns `Err(lettre::transport::smtp::Error::Transient(_))`. Worker checks `attempts`:
   - `attempts < 3`: `mark_failed_retry(retry_after_secs)` with backoff = `[60, 300, 1800]` indexed by `attempts`. Row stays `Pending`; `next_attempt_at` = now + backoff.
   - `attempts >= 3`: `mark_failed_permanent(last_error)`; state → `Failed`.
4. Successful sends: `state = Sent`, `sent_at = now()`.

### 3.5 Settings UI flow

1. User navigates to `/settings/security` (existing page) and clicks the new "Email notifications" tab → loads `/settings/notifications`.
2. dx SSR fetches current prefs via `notification_prefs_get` server-fn (returns 3 booleans).
3. User toggles a checkbox; client calls `notification_prefs_set(event_type, enabled)` server-fn, which upserts the row in `oc_user_notification_prefs`.
4. The mailer reads prefs at enqueue time (data flow §3.1), so changes take effect immediately for subsequent shares.

### 3.6 Configuration

`FileConfig` gains a nested `mail` section:

```toml
[mail]
transport = "smtp"          # "smtp" | "log" | "disabled"
smtp_host = "smtp.example.com"
smtp_port = 587
smtp_security = "starttls"  # "tls" | "starttls" | "none"
smtp_username = "noreply@example.com"
smtp_password = "..."        # SecretString
mail_from = "noreply@example.com"
mail_from_name = "Crabcloud"
```

Required fields when `transport=smtp`: `smtp_host`, `smtp_port`, `mail_from`. Others optional. Validated at startup.

`instance_url` (used in mail bodies for clickable links) is sourced from the existing `FileConfig::overwrite_cli_url` (set on installation), with fallback to `https://<bind_address>` if unset.

## 4. Testing strategy

The riskiest seams: (a) DB queue state machine under concurrent worker + scheduler, (b) Tera template rendering with i18n, (c) opt-out check ordering (we MUST check prefs before enqueue, otherwise opt-outs are oracled by queue-row existence), (d) retry backoff math, (e) email address validation at the OCS boundary.

### 4.1 `crabcloud-mail` unit tests

- `render_share_created_renders_html_and_text`: render both formats, parse the HTML, assert key fields appear.
- `render_link_emailed_includes_link_url_and_password_warning`: link URL present; if `password_protected: true`, body contains a hint to ask the sender for the password (NOT the password itself).
- `render_expiration_warning_formats_date_in_user_locale`: i18n locale switches affect the date format.
- `transport_log_emits_tracing_event`: `Transport::Log` writes a structured tracing event with envelope fields.
- `transport_disabled_send_is_no_op`.
- `smtp_transport_construction_fails_on_invalid_host`: invalid host config returns `MailError::ConfigInvalid`.
- `template_loader_falls_back_to_embedded_on_override_missing`: when the operator override directory doesn't exist, the embedded copies are used.
- `tera_auto_escape_blocks_html_injection_in_share_path`: a share path containing `<script>` is HTML-escaped in the rendered body.

### 4.2 `MailQueue` integration tests (multidialect)

- `enqueue_then_claim_batch_returns_row`: insert one, claim with limit 1, get it back.
- `claim_batch_skips_locked_rows`: two workers, two rows; each claims one, never both same. (Postgres / MySQL only; sqlite skipped.)
- `mark_failed_with_retry_sets_next_attempt_at`: backoff math correct for attempts 0, 1, 2.
- `mark_failed_at_attempt_3_transitions_to_failed_state`: persistent failure path.
- `reclaim_stuck_pending_rows`: a row stuck in `Sending` for > 5 minutes gets reclaimed by the next `reclaim_stuck` call.

### 4.3 `ExpirationWarningSweeper` integration tests

- `sweep_finds_links_in_24h_window`: seed a link with `expiration = now()+12h`, run sweep, assert one row enqueued + `last_warned` stamped.
- `sweep_skips_already_warned`: same link, run sweep twice, only one mail enqueued.
- `sweep_skips_outside_window`: links expiring in 48h or already expired → no enqueue.
- `sweep_respects_owner_opt_out`: owner has `expiration_warning = false` → no enqueue but `last_warned` still stamped.
- `sweep_handles_owner_missing_email`: owner exists but has no email → no enqueue, `last_warned` still stamped.

### 4.4 `Shares::create` integration tests (additions to existing)

- `share_type_0_create_enqueues_share_created_mail`: bob is a user with email; alice shares with bob; queue has one row.
- `share_type_0_create_skips_when_recipient_has_no_email`: enqueue is no-op.
- `share_type_0_create_skips_when_recipient_opted_out`: prefs row with `share_created=false`; enqueue is no-op.
- `share_type_4_create_enqueues_link_emailed_mail`: email + share row both created.
- `share_type_4_rejects_invalid_email`: `share_with = "not-an-email"` → `ShareError::InvalidEmail`.

### 4.5 `crabcloud-http` e2e tests

- `notification_prefs_get_returns_defaults_when_empty`: GET on the settings server-fn for a user with no prefs row returns `{ share_created: true, link_emailed: true, expiration_warning: true }`.
- `notification_prefs_set_persists`: POST the toggle; GET reflects the change.
- `ocs_share_type_4_creates_link_and_enqueues_email_in_log_transport`: assert the tracing log captures the envelope (via a `tracing-test` subscriber or similar).
- `ocs_share_type_4_invalid_email_returns_400`: malformed email → 400.

### 4.6 Settings UI SSR snapshot

- `settings_notifications_renders_three_toggles`: SSR snapshot includes 3 labeled checkboxes.

## 5. Risks & mitigations

| Risk | Mitigation |
|---|---|
| SMTP credentials in `FileConfig` leak via debug output / logs. | `smtp_password: SecretString` (same pattern as `secret` and `dbpassword`). `Debug` impl redacts; never logged verbatim. |
| Mail queue grows unbounded if SMTP is permanently broken. | Failed rows transition to `Failed` state after 3 attempts. Operator can `SELECT count(*) FROM oc_mail_queue WHERE state='Failed'` to monitor. A future SP can add a cleanup task that deletes `Sent` and `Failed` rows older than 30 days. |
| Worker crashes mid-send leave a row stuck in `Sending`. | Rows in `Sending` state with `claimed_at < now()-5m` are reclaimed by `MailQueue::reclaim_stuck` on the worker's next idle cycle. Standard recovery pattern. |
| Multiple replicas (multi-node deploy) both run a worker + sweeper, sending the same email twice. | `FOR UPDATE SKIP LOCKED` on Postgres/MySQL prevents queue race. For sqlite (single-node only), this is moot — multi-node sqlite is unsupported anyway. For the sweeper: stamping `last_warned` under the same transaction as the SELECT prevents duplicate enqueue across replicas. |
| Tera template panics on missing context variable. | `render_template` returns `MailError::Render` and the enqueue is skipped (with a tracing warn). Templates are unit-tested for the canonical context shape. Operator-overridden templates are loaded with `Tera::add_template_file` which uses `try_get` access semantics — same behavior. |
| Recipient address typo in OCS `share_type=4` create silently drops the mail. | Validate `share_with` via `Email::parse` at the OCS handler boundary; reject with `400 InvalidEmail` before any row is written. |
| User reads their own data via the queue table (e.g. the SQL leaks share targets via the body). | `oc_mail_queue` rows are operator-visible only via DB access. No user-facing endpoint exposes them. Mail bodies do include data ("alice shared X with you") which is expected — the recipient is going to see this anyway. |
| Tera template injection from user-supplied data (e.g. a share path with `{% raw %}`) escapes to template logic. | Tera auto-escapes string interpolation in HTML mode. Plaintext templates pass strings through verbatim — fine because they're not parsed as HTML. Templates only consume already-validated `StoragePath` / `Email` / `UserId` types. |
| Expiration sweep runs at startup and floods the queue if hundreds of links are about to expire. | Sweep is rate-limited implicitly by the worker drain rate (8 rows / 5s). Fan-out is bounded. Log a tracing event when a single sweep enqueues > 100 rows. |
| Log transport accidentally enabled in production → silent mail drop. | Startup validation warns if `mail.transport = "log"` while `bind_address` isn't local-only (`127.0.0.1` / `0.0.0.0:0`). Tracing event at startup announces the active transport. |
| User opts out of `share_created` but still receives `expiration_warning` for their own links. | Intentional: the three event types are independent. Documented in the settings UI (labels: "Notify me when others share with me", "Send a copy of email-share confirmations", "Warn me before my links expire"). |
| Password included in `link_emailed` mail body leaks via mail archives. | We deliberately DO NOT include the password in the mail body. The body says "the sender will share the password separately." Operator-overridden templates that include the password are operator's responsibility. |

## 6. Future work / SP-later hooks

- Group-share notifications (fan-out: enqueue one row per group member, deduplicated by uid).
- File-drop "someone uploaded X to your drop zone" notification.
- Storage quota warnings (80% / 95% thresholds).
- Digest mode: batch all notifications for a user into a daily summary.
- Operator-overridable templates at `<datadir>/mail-templates/{event}.{html,txt}`.
- Cleanup task for `oc_mail_queue` rows older than 30 days.
- Per-event-type CC / BCC headers (e.g. share notifications also CC the share owner).
- DKIM signing (lettre supports it via a feature flag).
- Federated email-share between Crabcloud instances.
