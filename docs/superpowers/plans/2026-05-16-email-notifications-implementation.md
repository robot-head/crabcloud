# Email Notifications Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship mailer infrastructure plus three notification flows: share-created (when alice shares with bob), link-emailed (email-share `share_type=4`), and expiration-warning (T-1 day before public-link expires). Per-event-type opt-out via a new prefs table; DB-backed mail queue with retry; daily-ish sweeper for expiration warnings.

**Architecture:** New `crabcloud-mail` crate owns SMTP transport (lettre + rustls), template rendering (tera + rust-embed), and `Transport::{Smtp, Log, Disabled}` dispatch. New `MailQueue` service in `crabcloud-core` persists envelopes in `oc_mail_queue` with retry/backoff; a `MailWorker` background task drains it. `ExpirationWarningSweeper` background task scans `oc_share` hourly for T-1-day expirations. `NotificationPrefs` in `crabcloud-users` gates each per-event opt-out. `Shares::create` hooks enqueue mail after successful row insert; new `ShareType::Email (share_type=4)` is a link variant that also emails the recipient. Settings UI surfaces 3 toggles.

**Tech Stack:** Rust 1.95, `lettre = "0.11"` (features `builder`, `smtp-transport`, `tokio1-rustls-tls`), `tera = "1"`, `rust-embed`, axum 0.8, Dioxus 0.7.

**Spec:** `docs/superpowers/specs/2026-05-16-email-notifications-design.md`

---

## Conventions for every batch

- **Branching:** Each batch is its own PR off `origin/master`:
  ```bash
  cd "C:/Users/Matt Stone/git/crabcloud"
  git fetch origin master
  git switch -c sp11/<batch-letter>-<slug> origin/master
  ```
  Slugs: `a-mail-crate`, `b-queue-and-config`, `c-notification-triggers`, `d-settings-ui`.
- **Commit cadence:** Commit at every "Commit" step.
- **Pre-PR check:**
  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```
- **Merge:** After CI green: `gh pr merge --squash --delete-branch`.
- **Established workaround:** Tests building `AppState` set `cfg.filecache.enabled = false`. Now also set `cfg.mail.transport = "disabled"` unless the test specifically exercises mail behavior (in which case set `"log"`).
- **Pre-existing patterns to mirror:**
  - **Presentation crate shape:** `crates/crabcloud-zip` (SP9), `crates/crabcloud-preview` (SP10) — small focused modules, `lib.rs` is a thin facade, `unused_crate_dependencies` lint quieted via `use foo as _;` anchors in `lib.rs`.
  - **Migration triplet:** `migrations/core/0006_shares/{sqlite,mysql,postgres}.sql`. New migrations get a sequential number.
  - **DB service with multidialect support:** `crates/crabcloud-sharing/src/{service,sql}.rs` — `_QM` (sqlite + mysql) vs `_PG` query constants, `match self.pool.as_ref()` dispatch.
  - **Background task spawn pattern:** None yet — establish in this SP. See Task B6 + C5.

---

## File-by-file map

### New crate: `crabcloud-mail`

```
crates/crabcloud-mail/
├── Cargo.toml
├── src/
│   ├── lib.rs                  — re-exports + crate doc
│   ├── error.rs                — MailError
│   ├── envelope.rs             — MailEnvelope, EventType
│   ├── transport.rs            — Transport enum (Smtp / Log / Disabled), TransportConfig
│   ├── mailer.rs               — Mailer::new, Mailer::send
│   ├── templates.rs            — Tera loader + render_template + TemplateContext
│   └── templates/              — Embedded Tera templates
│       ├── share_created.html
│       ├── share_created.txt
│       ├── link_emailed.html
│       ├── link_emailed.txt
│       ├── expiration_warning.html
│       ├── expiration_warning.txt
│       ├── _partials/header.html
│       └── _partials/footer.html
└── tests/                      — tests inline via #[cfg(test)] mod tests
```

### New migration

```
migrations/core/0007_mail_queue_and_notification_prefs/
├── sqlite.sql
├── mysql.sql
└── postgres.sql
```

### New migration

```
migrations/core/0008_share_last_warned/
├── sqlite.sql
├── mysql.sql
└── postgres.sql
```

(Two separate migrations so the queue/prefs land in Batch B and `last_warned` in Batch C; clean atomicity.)

### Modified

- Workspace `Cargo.toml` — adds `crates/crabcloud-mail` member; adds `lettre`, `tera`, `rust-embed` to `[workspace.dependencies]`.
- `crates/crabcloud-config/src/types.rs` — `MailConfig` + `mail: MailConfig` field.
- `crates/crabcloud-config/src/test_support.rs` — fills `mail` with disabled-transport defaults.
- `crates/crabcloud-core/src/state.rs` — `AppState.mailer`, `AppState.mail_queue`, `AppState.notification_prefs`; spawn `MailWorker` and `ExpirationWarningSweeper`.
- `crates/crabcloud-core/src/mail_queue.rs` (new) — `MailQueue::{enqueue, claim_batch, mark_*, reclaim_stuck}`.
- `crates/crabcloud-core/src/mail_worker.rs` (new) — `MailWorker::run`.
- `crates/crabcloud-core/src/expiration_sweeper.rs` (new) — `ExpirationWarningSweeper::run`.
- `crates/crabcloud-users/src/notification_prefs.rs` (new) — `NotificationPrefs::{get, set}`.
- `crates/crabcloud-users/src/lib.rs` — re-exports.
- `crates/crabcloud-sharing/src/types.rs` — `ShareType::Email` variant + i16 conversion.
- `crates/crabcloud-sharing/src/service.rs` — `Shares::create` hooks for share_type=0/1/4; `Shares::create_link` accepts email recipient.
- `crates/crabcloud-sharing/src/sql.rs` — `last_warned` in SELECT column list; new `UPDATE_LAST_WARNED_*` constants.
- `crates/crabcloud-http/src/routes/ocs/files_sharing.rs` — accept `shareType=4` + email validation + recipient email passthrough.
- `crates/crabcloud-app/src/server_fns/notification_prefs.rs` (new) — get/set server-fns.
- `crates/crabcloud-app/src/pages/settings/notifications.rs` (new) — settings page.
- `crates/crabcloud-app/src/app.rs` — register settings route.

---

# Batch A — `crabcloud-mail` foundation crate

**Branch:** `sp11/a-mail-crate`

**Goal:** Stand up the mailer crate: types, transports (Smtp / Log / Disabled), Tera template loader, three templates. No DB, no scheduling, no notification triggers. Unit-tested in isolation.

### Task A1: Crate skeleton

**Files:**
- Create: `crates/crabcloud-mail/Cargo.toml`
- Create: `crates/crabcloud-mail/src/lib.rs`
- Create: `crates/crabcloud-mail/src/error.rs`
- Modify: workspace `Cargo.toml`

- [ ] **Step 1: Add the crate to the workspace and register deps**

Edit workspace `Cargo.toml`:
1. Add `"crates/crabcloud-mail",` to `members`.
2. Add to `[workspace.dependencies]`:
   ```toml
   lettre = { version = "0.11", default-features = false, features = ["builder", "smtp-transport", "tokio1-rustls-tls"] }
   tera = "1"
   rust-embed = { version = "8", features = ["debug-embed"] }
   ```
3. Add internal workspace dep:
   ```toml
   crabcloud-mail = { path = "crates/crabcloud-mail" }
   ```

- [ ] **Step 2: Write `crates/crabcloud-mail/Cargo.toml`**

```toml
[package]
name = "crabcloud-mail"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
async-trait = { workspace = true }
chrono = { workspace = true }
crabcloud-i18n = { workspace = true }
crabcloud-users = { workspace = true }
lettre = { workspace = true }
rust-embed = { workspace = true }
secrecy = { workspace = true }
serde = { workspace = true }
tera = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true, features = ["sync", "rt", "macros"] }
tracing = { workspace = true }

[dev-dependencies]
tokio = { workspace = true, features = ["macros", "rt-multi-thread", "test-util"] }
```

- [ ] **Step 3: Write `src/lib.rs`**

```rust
//! Mailer infrastructure for Crabcloud.
//!
//! Spec: `docs/superpowers/specs/2026-05-16-email-notifications-design.md`.
//!
//! Public entry points are [`Mailer`] (`send` actual mail) and
//! [`render_template`] (compose a [`MailEnvelope`] from an [`EventType`] +
//! [`TemplateContext`]). The queue and worker layers (Batch B) own the
//! "decide to send" path; this crate just transports.

mod envelope;
mod error;
mod mailer;
mod templates;
mod transport;

pub use envelope::{EventType, MailEnvelope};
pub use error::MailError;
pub use mailer::Mailer;
pub use templates::{render_template, TemplateContext};
pub use transport::{Transport, TransportConfig};
```

- [ ] **Step 4: Write `src/error.rs`**

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MailError {
    #[error("config invalid: {0}")]
    ConfigInvalid(String),
    #[error("template render failed: {0}")]
    Render(String),
    #[error("transport failed: {0}")]
    Transport(String),
    #[error("transient transport failure: {0}")]
    Transient(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl MailError {
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Transient(_))
    }
}
```

- [ ] **Step 5: Build**

```bash
cargo build -p crabcloud-mail
```

Expect: clean, with warnings about unused module re-exports (resolve in later tasks).

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/crabcloud-mail/
git commit -m "mail: crate skeleton with error type and workspace integration"
```

### Task A2: `MailEnvelope` + `EventType`

**Files:**
- Create: `crates/crabcloud-mail/src/envelope.rs`

- [ ] **Step 1: Write impl + tests**

```rust
//! `MailEnvelope` — a fully-rendered mail ready for transport. Carries
//! the recipient address, subject, and both HTML + plaintext bodies for
//! multipart MIME assembly.

use serde::{Deserialize, Serialize};

/// Discriminates which notification event a queued mail represents.
/// Stored as a `&'static str` on the wire (`oc_mail_queue.event_type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventType {
    ShareCreated,
    LinkEmailed,
    ExpirationWarning,
}

impl EventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            EventType::ShareCreated => "share_created",
            EventType::LinkEmailed => "link_emailed",
            EventType::ExpirationWarning => "expiration_warning",
        }
    }

    /// Parse from the on-wire string used in `oc_mail_queue.event_type`.
    /// Returns `None` for unknown strings (forward-compat with rows
    /// written by a newer server).
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "share_created" => Some(Self::ShareCreated),
            "link_emailed" => Some(Self::LinkEmailed),
            "expiration_warning" => Some(Self::ExpirationWarning),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MailEnvelope {
    pub recipient: String,
    pub subject: String,
    pub html_body: String,
    pub text_body: String,
    pub event_type: EventType,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_type_round_trips_via_str() {
        for v in &[EventType::ShareCreated, EventType::LinkEmailed, EventType::ExpirationWarning] {
            let s = v.as_str();
            assert_eq!(EventType::from_str(s), Some(*v));
        }
    }

    #[test]
    fn event_type_parse_rejects_unknown() {
        assert_eq!(EventType::from_str("password_reset"), None);
        assert_eq!(EventType::from_str(""), None);
    }
}
```

- [ ] **Step 2: Run tests**

```bash
cargo test -p crabcloud-mail envelope::tests
```

Expected: 2 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/crabcloud-mail/src/envelope.rs
git commit -m "mail: MailEnvelope + EventType (3 MVP events)"
```

### Task A3: Transport + `TransportConfig`

**Files:**
- Create: `crates/crabcloud-mail/src/transport.rs`

- [ ] **Step 1: Write impl**

```rust
//! Mailer transports: SMTP, Log (tracing-event), Disabled (no-op).

use crate::error::MailError;
use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, Tokio1Executor};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};

/// Operator-tunable transport configuration. Parsed from `FileConfig::mail`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportConfig {
    pub kind: TransportKind,
    pub smtp_host: Option<String>,
    pub smtp_port: Option<u16>,
    pub smtp_username: Option<String>,
    pub smtp_password: Option<secrecy::SecretString>,
    pub smtp_security: SmtpSecurity,
    pub mail_from: Option<String>,
    pub mail_from_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportKind {
    Smtp,
    Log,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SmtpSecurity {
    Tls,
    Starttls,
    None,
}

/// Runtime transport. `Smtp` carries a live lettre client; `Log` writes
/// envelopes to a tracing event; `Disabled` is a no-op that returns Ok.
pub enum Transport {
    Smtp(AsyncSmtpTransport<Tokio1Executor>),
    Log,
    Disabled,
}

impl Transport {
    /// Build a transport from config. Validates required fields when
    /// `kind == Smtp`; logs the active kind at info level.
    pub fn from_config(cfg: &TransportConfig) -> Result<Self, MailError> {
        match cfg.kind {
            TransportKind::Disabled => {
                tracing::info!("mail.transport = disabled — outbound mail will be silently dropped");
                Ok(Self::Disabled)
            }
            TransportKind::Log => {
                tracing::info!("mail.transport = log — outbound mail will be emitted as tracing events");
                Ok(Self::Log)
            }
            TransportKind::Smtp => {
                let host = cfg
                    .smtp_host
                    .as_deref()
                    .ok_or_else(|| MailError::ConfigInvalid("smtp_host required when transport=smtp".into()))?;
                let port = cfg
                    .smtp_port
                    .ok_or_else(|| MailError::ConfigInvalid("smtp_port required when transport=smtp".into()))?;
                let _ = cfg
                    .mail_from
                    .as_deref()
                    .ok_or_else(|| MailError::ConfigInvalid("mail_from required when transport=smtp".into()))?;
                let mut builder = match cfg.smtp_security {
                    SmtpSecurity::Tls => {
                        let tls = TlsParameters::new(host.to_string())
                            .map_err(|e| MailError::ConfigInvalid(format!("tls params: {e}")))?;
                        AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host).tls(Tls::Wrapper(tls))
                    }
                    SmtpSecurity::Starttls => {
                        let tls = TlsParameters::new(host.to_string())
                            .map_err(|e| MailError::ConfigInvalid(format!("tls params: {e}")))?;
                        AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host).tls(Tls::Required(tls))
                    }
                    SmtpSecurity::None => {
                        AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
                    }
                }
                .port(port);
                if let (Some(u), Some(p)) = (cfg.smtp_username.as_deref(), cfg.smtp_password.as_ref()) {
                    builder = builder.credentials(Credentials::new(u.to_string(), p.expose_secret().to_string()))
                        .authentication(vec![Mechanism::Plain, Mechanism::Login]);
                }
                tracing::info!(host, port, "mail.transport = smtp");
                Ok(Self::Smtp(builder.build()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(kind: TransportKind) -> TransportConfig {
        TransportConfig {
            kind,
            smtp_host: None,
            smtp_port: None,
            smtp_username: None,
            smtp_password: None,
            smtp_security: SmtpSecurity::Starttls,
            mail_from: None,
            mail_from_name: None,
        }
    }

    #[test]
    fn disabled_transport_builds() {
        assert!(matches!(
            Transport::from_config(&cfg(TransportKind::Disabled)).unwrap(),
            Transport::Disabled
        ));
    }

    #[test]
    fn log_transport_builds() {
        assert!(matches!(
            Transport::from_config(&cfg(TransportKind::Log)).unwrap(),
            Transport::Log
        ));
    }

    #[test]
    fn smtp_transport_requires_host() {
        let c = cfg(TransportKind::Smtp);
        let r = Transport::from_config(&c);
        match r {
            Err(MailError::ConfigInvalid(msg)) => assert!(msg.contains("smtp_host")),
            other => panic!("expected ConfigInvalid, got {other:?}"),
        }
    }

    #[test]
    fn smtp_transport_requires_mail_from() {
        let mut c = cfg(TransportKind::Smtp);
        c.smtp_host = Some("smtp.example.com".into());
        c.smtp_port = Some(587);
        let r = Transport::from_config(&c);
        match r {
            Err(MailError::ConfigInvalid(msg)) => assert!(msg.contains("mail_from")),
            other => panic!("expected ConfigInvalid, got {other:?}"),
        }
    }

    #[test]
    fn smtp_transport_builds_with_required_fields() {
        let mut c = cfg(TransportKind::Smtp);
        c.smtp_host = Some("smtp.example.com".into());
        c.smtp_port = Some(587);
        c.mail_from = Some("noreply@example.com".into());
        assert!(matches!(
            Transport::from_config(&c).unwrap(),
            Transport::Smtp(_)
        ));
    }
}
```

- [ ] **Step 2: Run tests**

```bash
cargo test -p crabcloud-mail transport::tests
```

Expected: 5 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/crabcloud-mail/src/transport.rs
git commit -m "mail: Transport enum + config validation (Smtp/Log/Disabled)"
```

### Task A4: `Mailer::send`

**Files:**
- Create: `crates/crabcloud-mail/src/mailer.rs`

- [ ] **Step 1: Write impl + tests**

```rust
//! Top-level mailer facade. Holds a configured `Transport` + the from-address
//! pair. `send` actually transmits.

use crate::envelope::MailEnvelope;
use crate::error::MailError;
use crate::transport::{Transport, TransportConfig};
use lettre::message::{header, MultiPart, SinglePart};
use lettre::{AsyncTransport, Message};

pub struct Mailer {
    transport: Transport,
    from_address: String,
    from_name: Option<String>,
}

impl Mailer {
    pub fn from_config(cfg: &TransportConfig) -> Result<Self, MailError> {
        let transport = Transport::from_config(cfg)?;
        let from_address = cfg
            .mail_from
            .clone()
            .unwrap_or_else(|| "no-reply@localhost".to_string());
        Ok(Self {
            transport,
            from_address,
            from_name: cfg.mail_from_name.clone(),
        })
    }

    pub async fn send(&self, env: &MailEnvelope) -> Result<(), MailError> {
        match &self.transport {
            Transport::Disabled => Ok(()),
            Transport::Log => {
                tracing::info!(
                    target: "crabcloud_mail::log_transport",
                    recipient = %env.recipient,
                    subject = %env.subject,
                    event_type = %env.event_type.as_str(),
                    text_body_bytes = env.text_body.len(),
                    "mail.transport=log envelope captured (not sent)"
                );
                Ok(())
            }
            Transport::Smtp(client) => {
                let from = match &self.from_name {
                    Some(name) => format!("{} <{}>", name, self.from_address),
                    None => self.from_address.clone(),
                };
                let msg = Message::builder()
                    .from(from.parse().map_err(|e| {
                        MailError::ConfigInvalid(format!("mail_from parse: {e}"))
                    })?)
                    .to(env.recipient.parse().map_err(|e| {
                        MailError::Transport(format!("recipient parse: {e}"))
                    })?)
                    .subject(&env.subject)
                    .multipart(
                        MultiPart::alternative()
                            .singlepart(
                                SinglePart::builder()
                                    .header(header::ContentType::TEXT_PLAIN)
                                    .body(env.text_body.clone()),
                            )
                            .singlepart(
                                SinglePart::builder()
                                    .header(header::ContentType::TEXT_HTML)
                                    .body(env.html_body.clone()),
                            ),
                    )
                    .map_err(|e| MailError::Transport(format!("message build: {e}")))?;
                client.send(msg).await.map_err(|e| {
                    if e.is_transient() {
                        MailError::Transient(format!("smtp: {e}"))
                    } else {
                        MailError::Transport(format!("smtp: {e}"))
                    }
                })?;
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::EventType;
    use crate::transport::{SmtpSecurity, TransportConfig, TransportKind};

    fn env() -> MailEnvelope {
        MailEnvelope {
            recipient: "bob@example.com".to_string(),
            subject: "Hello".to_string(),
            html_body: "<p>hi</p>".to_string(),
            text_body: "hi".to_string(),
            event_type: EventType::ShareCreated,
        }
    }

    fn disabled() -> TransportConfig {
        TransportConfig {
            kind: TransportKind::Disabled,
            smtp_host: None,
            smtp_port: None,
            smtp_username: None,
            smtp_password: None,
            smtp_security: SmtpSecurity::None,
            mail_from: Some("noreply@example.com".to_string()),
            mail_from_name: None,
        }
    }

    fn log() -> TransportConfig {
        let mut c = disabled();
        c.kind = TransportKind::Log;
        c
    }

    #[tokio::test]
    async fn disabled_send_is_noop() {
        let m = Mailer::from_config(&disabled()).unwrap();
        m.send(&env()).await.unwrap();
    }

    #[tokio::test]
    async fn log_send_emits_tracing_event() {
        // tracing event capture isn't trivial without `tracing-test`; for
        // now just assert the send returns Ok. The e2e tests in Batch B
        // will install a subscriber and assert capture.
        let m = Mailer::from_config(&log()).unwrap();
        m.send(&env()).await.unwrap();
    }
}
```

- [ ] **Step 2: Run tests**

```bash
cargo test -p crabcloud-mail mailer::tests
```

Expected: 2 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/crabcloud-mail/src/mailer.rs
git commit -m "mail: Mailer::send dispatches to Smtp/Log/Disabled with multipart MIME"
```

### Task A5: Tera template loader + `render_template`

**Files:**
- Create: `crates/crabcloud-mail/src/templates.rs`
- Create: `crates/crabcloud-mail/src/templates/*` (template files; see Task A6)

- [ ] **Step 1: Write the loader and render function**

```rust
//! Tera template loader (embedded via rust-embed) + render_template.

use crate::envelope::{EventType, MailEnvelope};
use crate::error::MailError;
use chrono::{DateTime, Utc};
use rust_embed::RustEmbed;
use serde::Serialize;
use std::sync::OnceLock;
use tera::{Context, Tera};

/// Embedded template files. `crates/crabcloud-mail/src/templates/` is
/// hardcoded; operator overrides at `<datadir>/mail-templates/` is a
/// future feature.
#[derive(RustEmbed)]
#[folder = "src/templates"]
struct EmbeddedTemplates;

/// Process-wide Tera instance, loaded once on first use.
fn tera() -> &'static Tera {
    static T: OnceLock<Tera> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = Tera::default();
        for name in EmbeddedTemplates::iter() {
            let bytes = EmbeddedTemplates::get(&name).expect("rust-embed name must resolve");
            let body = std::str::from_utf8(&bytes.data).expect("templates are utf8");
            t.add_raw_template(&name, body).expect("template parse");
        }
        // Plaintext templates need auto-escape OFF — Tera detects via file
        // extension. .html files keep auto-escape on; .txt files don't.
        t.autoescape_on(vec!["html"]);
        t
    })
}

/// Render context. Event-specific fields are passed in via `extra`.
/// `lang` selects the i18n locale; the renderer looks up labels via
/// `crabcloud_i18n::translate(lang, key)` (or whatever the actual i18n
/// API is — verify in lib).
#[derive(Debug, Clone, Serialize)]
pub struct TemplateContext {
    pub lang: String,
    pub instance_url: String,
    pub recipient_display_name: String,
    pub recipient_email: String,
    pub event_specific: serde_json::Value,
}

/// Render a fully-populated MailEnvelope for the given event type.
pub fn render_template(
    event: EventType,
    ctx: TemplateContext,
) -> Result<MailEnvelope, MailError> {
    let html_name = format!("{}.html", event.as_str());
    let text_name = format!("{}.txt", event.as_str());

    let mut tera_ctx = Context::new();
    tera_ctx.insert("instance_url", &ctx.instance_url);
    tera_ctx.insert("recipient_display_name", &ctx.recipient_display_name);
    tera_ctx.insert("recipient_email", &ctx.recipient_email);
    tera_ctx.insert("lang", &ctx.lang);
    if let serde_json::Value::Object(map) = &ctx.event_specific {
        for (k, v) in map.iter() {
            tera_ctx.insert(k.clone(), v);
        }
    }

    let html_body = tera()
        .render(&html_name, &tera_ctx)
        .map_err(|e| MailError::Render(format!("{html_name}: {e}")))?;
    let text_body = tera()
        .render(&text_name, &tera_ctx)
        .map_err(|e| MailError::Render(format!("{text_name}: {e}")))?;

    // Subject lookup: each event has a static subject for now (i18n TBD).
    let subject = match event {
        EventType::ShareCreated => "You've received a new share",
        EventType::LinkEmailed => "A file has been shared with you",
        EventType::ExpirationWarning => "Your share link is expiring soon",
    }
    .to_string();

    Ok(MailEnvelope {
        recipient: ctx.recipient_email,
        subject,
        html_body,
        text_body,
        event_type: event,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx(event_specific: serde_json::Value) -> TemplateContext {
        TemplateContext {
            lang: "en".to_string(),
            instance_url: "https://crabcloud.example".to_string(),
            recipient_display_name: "Bob".to_string(),
            recipient_email: "bob@example.com".to_string(),
            event_specific,
        }
    }

    #[test]
    fn share_created_renders_both_formats() {
        let env = render_template(
            EventType::ShareCreated,
            ctx(json!({
                "owner_display_name": "Alice",
                "path_basename": "Vacation",
            })),
        )
        .unwrap();
        assert_eq!(env.recipient, "bob@example.com");
        assert!(env.subject.contains("share") || env.subject.contains("Share"));
        assert!(env.html_body.contains("Alice"));
        assert!(env.html_body.contains("Vacation"));
        assert!(env.text_body.contains("Alice"));
        assert!(env.text_body.contains("Vacation"));
    }

    #[test]
    fn link_emailed_includes_link_and_omits_password() {
        let env = render_template(
            EventType::LinkEmailed,
            ctx(json!({
                "owner_display_name": "Alice",
                "path_basename": "Photos",
                "link_url": "https://crabcloud.example/s/AbCd123Xyz0789Q",
                "password_protected": true,
            })),
        )
        .unwrap();
        assert!(env.html_body.contains("https://crabcloud.example/s/AbCd123Xyz0789Q"));
        // No password field in the body — security invariant.
        assert!(!env.html_body.contains("password=\""));
        assert!(env.html_body.contains("password") || env.text_body.contains("password"));
    }

    #[test]
    fn expiration_warning_includes_link_and_date() {
        let env = render_template(
            EventType::ExpirationWarning,
            ctx(json!({
                "link_basename": "Photos",
                "link_url": "https://crabcloud.example/s/AbCd123Xyz0789Q",
                "expiration_dt": "2026-06-15",
            })),
        )
        .unwrap();
        assert!(env.html_body.contains("2026-06-15"));
        assert!(env.html_body.contains("Photos"));
    }

    #[test]
    fn html_escapes_user_supplied_path() {
        let env = render_template(
            EventType::ShareCreated,
            ctx(json!({
                "owner_display_name": "Alice",
                "path_basename": "<script>alert(1)</script>",
            })),
        )
        .unwrap();
        // HTML body must escape; plaintext is verbatim.
        assert!(!env.html_body.contains("<script>"));
        assert!(env.html_body.contains("&lt;script&gt;"));
        assert!(env.text_body.contains("<script>"));
    }
}
```

- [ ] **Step 2: Don't run tests yet** — templates don't exist; Task A6 adds them.

- [ ] **Step 3: Commit**

```bash
git add crates/crabcloud-mail/src/templates.rs
git commit -m "mail: Tera template loader + render_template (no templates yet)"
```

### Task A6: Templates (3 events × 2 formats + 2 partials)

**Files:**
- Create: `crates/crabcloud-mail/src/templates/_partials/header.html`
- Create: `crates/crabcloud-mail/src/templates/_partials/footer.html`
- Create: `crates/crabcloud-mail/src/templates/share_created.html`
- Create: `crates/crabcloud-mail/src/templates/share_created.txt`
- Create: `crates/crabcloud-mail/src/templates/link_emailed.html`
- Create: `crates/crabcloud-mail/src/templates/link_emailed.txt`
- Create: `crates/crabcloud-mail/src/templates/expiration_warning.html`
- Create: `crates/crabcloud-mail/src/templates/expiration_warning.txt`

- [ ] **Step 1: Write `_partials/header.html`**

```html
<!DOCTYPE html>
<html lang="{{ lang }}">
<head>
<meta charset="utf-8">
<title>{{ subject }}</title>
</head>
<body style="font-family: sans-serif; max-width: 600px; margin: 0 auto;">
```

- [ ] **Step 2: Write `_partials/footer.html`**

```html
<hr style="border:0;border-top:1px solid #ddd;margin:24px 0;">
<p style="color:#888;font-size:12px;">
  Sent by Crabcloud at <a href="{{ instance_url }}">{{ instance_url }}</a>.
</p>
</body>
</html>
```

- [ ] **Step 3: Write `share_created.html`**

```html
{% include "_partials/header.html" %}
<p>Hi {{ recipient_display_name }},</p>
<p>
  <strong>{{ owner_display_name }}</strong> has shared
  <strong>{{ path_basename }}</strong> with you.
</p>
<p>
  Open <a href="{{ instance_url }}">Crabcloud</a> to view the shared item.
</p>
{% include "_partials/footer.html" %}
```

- [ ] **Step 4: Write `share_created.txt`**

```
Hi {{ recipient_display_name }},

{{ owner_display_name }} has shared {{ path_basename }} with you.

Open Crabcloud to view it: {{ instance_url }}

--
Sent by Crabcloud.
```

- [ ] **Step 5: Write `link_emailed.html`**

```html
{% include "_partials/header.html" %}
<p>Hi,</p>
<p>
  <strong>{{ owner_display_name }}</strong> has shared
  <strong>{{ path_basename }}</strong> with you via a public link.
</p>
<p>
  <a href="{{ link_url }}" style="display:inline-block;padding:10px 16px;background:#0d6efd;color:#fff;text-decoration:none;border-radius:4px;">
    Open share
  </a>
</p>
<p style="font-size:13px;color:#666;">
  Link URL: <a href="{{ link_url }}">{{ link_url }}</a>
</p>
{% if password_protected %}
<p>
  This link is password-protected. The sender will share the password with you separately.
</p>
{% endif %}
{% include "_partials/footer.html" %}
```

- [ ] **Step 6: Write `link_emailed.txt`**

```
Hi,

{{ owner_display_name }} has shared {{ path_basename }} with you via a public link.

Open the share at: {{ link_url }}

{% if password_protected %}
This link is password-protected. The sender will share the password with you separately.
{% endif %}
--
Sent by Crabcloud at {{ instance_url }}.
```

- [ ] **Step 7: Write `expiration_warning.html`**

```html
{% include "_partials/header.html" %}
<p>Hi,</p>
<p>
  Your public-link share for <strong>{{ link_basename }}</strong> expires
  on <strong>{{ expiration_dt }}</strong>.
</p>
<p>
  Open <a href="{{ instance_url }}">Crabcloud</a> to extend or revoke the share.
</p>
<p style="font-size:13px;color:#666;">
  Link URL: <a href="{{ link_url }}">{{ link_url }}</a>
</p>
{% include "_partials/footer.html" %}
```

- [ ] **Step 8: Write `expiration_warning.txt`**

```
Hi,

Your public-link share for {{ link_basename }} expires on {{ expiration_dt }}.

Open Crabcloud to extend or revoke the share: {{ instance_url }}

Link URL: {{ link_url }}

--
Sent by Crabcloud.
```

- [ ] **Step 9: Run tests**

```bash
cargo test -p crabcloud-mail templates::tests
```

Expected: 4 tests pass. If the `html_escapes_user_supplied_path` test fails because Tera's auto-escape is off by default on string templates, swap `t.autoescape_on(vec!["html"])` for a more specific extension match — `add_raw_template` may not pattern-match on filename for auto-escape. Workaround: explicitly call `tera_ctx.insert("path_basename", &askama_escape::escape(s, askama_escape::Html))` in the renderer, OR rename HTML templates to end in `.tera.html` and adjust the autoescape pattern.

- [ ] **Step 10: Commit**

```bash
git add crates/crabcloud-mail/src/templates/
git commit -m "mail: HTML + plaintext templates for 3 MVP events"
```

### Task A7: Pre-PR sweep + PR

- [ ] **Step 1: Sweep**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Fix any drift. Common: `unused_crate_dependencies` on `chrono` / `serde` if not used in a code path — anchor via `use foo as _;` in `lib.rs`.

- [ ] **Step 2: Push + open PR**

```bash
git push -u origin sp11/a-mail-crate
gh pr create --title "sp11(a): crabcloud-mail foundation (lettre + tera, 3 transports, 3 templates)" --body "$(cat <<'EOF'
## Summary
- New `crabcloud-mail` crate.
- `Mailer::send` over `Transport::{Smtp, Log, Disabled}` with multipart MIME.
- Tera template loader (embedded via rust-embed) + `render_template` for 3 events.
- HTML + plaintext templates for `share_created`, `link_emailed`, `expiration_warning`.
- Unit-tested in isolation; no DB, no scheduling, no notification triggers yet.

## Test plan
- [ ] CI green (fmt-and-clippy, build-wasm, test-sqlite, test-multidialect, e2e)
- [ ] `cargo test -p crabcloud-mail` passes locally

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 3: Merge after green.**

---

# Batch B — Queue + worker + config + AppState wiring

**Branch:** `sp11/b-queue-and-config`

**Goal:** Add `MailConfig` to `FileConfig`. Land the `oc_mail_queue` + `oc_user_notification_prefs` migration. Build `MailQueue`, `NotificationPrefs`, and the `MailWorker` background task. Wire everything into `AppState`. E2E test that an enqueued mail under `transport=log` produces a tracing event.

### Task B1: `MailConfig` in `FileConfig`

**Files:**
- Modify: `crates/crabcloud-config/src/types.rs`
- Modify: `crates/crabcloud-config/src/test_support.rs`

- [ ] **Step 1: Add `MailConfig` struct**

In `crates/crabcloud-config/src/types.rs`, after the existing nested config sections (cache, etc.), add:

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MailConfig {
    /// Transport mode: "smtp" (production), "log" (dev — emit envelopes
    /// as tracing events), "disabled" (worker not spawned).
    #[serde(default = "default_mail_transport")]
    pub transport: String,
    /// SMTP server hostname. Required when transport=smtp.
    #[serde(default)]
    pub smtp_host: Option<String>,
    /// SMTP server port. Required when transport=smtp.
    #[serde(default)]
    pub smtp_port: Option<u16>,
    /// SMTP authentication username. Optional.
    #[serde(default)]
    pub smtp_username: Option<String>,
    /// SMTP authentication password. Optional. SecretString redacts in logs.
    #[serde(default)]
    pub smtp_password: Option<SecretString>,
    /// Connection security: "tls" (implicit), "starttls", or "none".
    #[serde(default = "default_smtp_security")]
    pub smtp_security: String,
    /// From address (envelope + From header). Required when transport=smtp.
    #[serde(default)]
    pub mail_from: Option<String>,
    /// Optional display name for From header.
    #[serde(default)]
    pub mail_from_name: Option<String>,
}

impl Default for MailConfig {
    fn default() -> Self {
        Self {
            transport: default_mail_transport(),
            smtp_host: None,
            smtp_port: None,
            smtp_username: None,
            smtp_password: None,
            smtp_security: default_smtp_security(),
            mail_from: None,
            mail_from_name: None,
        }
    }
}

fn default_mail_transport() -> String {
    "disabled".to_string()
}

fn default_smtp_security() -> String {
    "starttls".to_string()
}
```

Then add to `FileConfig`:

```rust
    /// Mail transport configuration.
    #[serde(default)]
    pub mail: MailConfig,
```

- [ ] **Step 2: Update `test_support.rs`**

In `crates/crabcloud-config/src/test_support.rs::minimal_sqlite_config`, fill `mail` with the default (already `transport=disabled`):

```rust
        mail: MailConfig::default(),
```

- [ ] **Step 3: Build + test**

```bash
cargo build -p crabcloud-config
cargo test -p crabcloud-config
```

- [ ] **Step 4: Commit**

```bash
git add crates/crabcloud-config/
git commit -m "config: MailConfig nested section (transport/smtp_*/mail_from)"
```

### Task B2: Migration `0007_mail_queue_and_notification_prefs`

**Files:**
- Create: `migrations/core/0007_mail_queue_and_notification_prefs/sqlite.sql`
- Create: `migrations/core/0007_mail_queue_and_notification_prefs/mysql.sql`
- Create: `migrations/core/0007_mail_queue_and_notification_prefs/postgres.sql`

- [ ] **Step 1: Write `sqlite.sql`**

```sql
CREATE TABLE oc_mail_queue (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    recipient        VARCHAR(255) NOT NULL,
    subject          VARCHAR(512) NOT NULL,
    html_body        TEXT         NOT NULL,
    text_body        TEXT         NOT NULL,
    event_type       VARCHAR(64)  NOT NULL,
    attempts         INTEGER      NOT NULL DEFAULT 0,
    next_attempt_at  TIMESTAMP    NOT NULL,
    state            VARCHAR(16)  NOT NULL DEFAULT 'Pending',
    claimed_at       TIMESTAMP    NULL,
    last_error       TEXT         NULL,
    created_at       TIMESTAMP    NOT NULL,
    sent_at          TIMESTAMP    NULL
);

CREATE INDEX idx_mail_queue_state_next_attempt ON oc_mail_queue (state, next_attempt_at);

CREATE TABLE oc_user_notification_prefs (
    user_id      VARCHAR(64)  NOT NULL,
    event_type   VARCHAR(64)  NOT NULL,
    enabled      SMALLINT     NOT NULL DEFAULT 1,
    PRIMARY KEY (user_id, event_type)
);
```

- [ ] **Step 2: Write `mysql.sql`**

```sql
CREATE TABLE oc_mail_queue (
    id               BIGINT       NOT NULL AUTO_INCREMENT,
    recipient        VARCHAR(255) NOT NULL,
    subject          VARCHAR(512) NOT NULL,
    html_body        TEXT         NOT NULL,
    text_body        TEXT         NOT NULL,
    event_type       VARCHAR(64)  NOT NULL,
    attempts         INT          NOT NULL DEFAULT 0,
    next_attempt_at  TIMESTAMP    NOT NULL,
    state            VARCHAR(16)  NOT NULL DEFAULT 'Pending',
    claimed_at       TIMESTAMP    NULL,
    last_error       TEXT         NULL,
    created_at       TIMESTAMP    NOT NULL,
    sent_at          TIMESTAMP    NULL,
    PRIMARY KEY (id)
) ENGINE=InnoDB COLLATE=utf8mb4_bin;

CREATE INDEX idx_mail_queue_state_next_attempt ON oc_mail_queue (state, next_attempt_at);

CREATE TABLE oc_user_notification_prefs (
    user_id      VARCHAR(64)  NOT NULL,
    event_type   VARCHAR(64)  NOT NULL,
    enabled      SMALLINT     NOT NULL DEFAULT 1,
    PRIMARY KEY (user_id, event_type)
) ENGINE=InnoDB COLLATE=utf8mb4_bin;
```

- [ ] **Step 3: Write `postgres.sql`**

```sql
CREATE TABLE oc_mail_queue (
    id               BIGSERIAL    PRIMARY KEY,
    recipient        VARCHAR(255) NOT NULL,
    subject          VARCHAR(512) NOT NULL,
    html_body        TEXT         NOT NULL,
    text_body        TEXT         NOT NULL,
    event_type       VARCHAR(64)  NOT NULL,
    attempts         INTEGER      NOT NULL DEFAULT 0,
    next_attempt_at  TIMESTAMP    NOT NULL,
    state            VARCHAR(16)  NOT NULL DEFAULT 'Pending',
    claimed_at       TIMESTAMP    NULL,
    last_error       TEXT         NULL,
    created_at       TIMESTAMP    NOT NULL,
    sent_at          TIMESTAMP    NULL
);

CREATE INDEX idx_mail_queue_state_next_attempt ON oc_mail_queue (state, next_attempt_at);

CREATE TABLE oc_user_notification_prefs (
    user_id      VARCHAR(64)  NOT NULL,
    event_type   VARCHAR(64)  NOT NULL,
    enabled      SMALLINT     NOT NULL DEFAULT 1,
    PRIMARY KEY (user_id, event_type)
);
```

- [ ] **Step 4: Verify migration discovered + runs**

```bash
cargo test -p crabcloud-db migrate_end_to_end -- --nocapture
```

Expected: the existing multidialect migration test picks up the new directory and applies the SQL. If the test runner doesn't auto-discover, look at how prior migrations were registered (`migrations/core/0006_shares` should be a pattern — same shape).

- [ ] **Step 5: Commit**

```bash
git add migrations/core/0007_mail_queue_and_notification_prefs/
git commit -m "db: 0007_mail_queue_and_notification_prefs migration (3 dialects)"
```

### Task B3: `MailQueue` service

**Files:**
- Create: `crates/crabcloud-core/src/mail_queue.rs`
- Modify: `crates/crabcloud-core/src/lib.rs` — re-exports

- [ ] **Step 1: Write the impl**

```rust
//! `MailQueue` — persistent queue for outbound mail. Workers claim
//! batches and call `mark_sent`/`mark_failed_*`; the expiration sweeper
//! and the Shares hooks call `enqueue`.

use chrono::{DateTime, Utc};
use crabcloud_db::DbPool;
use crabcloud_mail::{EventType, MailEnvelope};
use sqlx::Row as _;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MailQueueError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error("unknown event_type in row: {0}")]
    UnknownEventType(String),
}

#[derive(Debug, Clone)]
pub struct MailQueueRow {
    pub id: i64,
    pub recipient: String,
    pub subject: String,
    pub html_body: String,
    pub text_body: String,
    pub event_type: EventType,
    pub attempts: i32,
}

const BACKOFF_SECS: [i64; 3] = [60, 300, 1800];
const STUCK_SENDING_AFTER_SECS: i64 = 300; // 5 minutes

#[derive(Clone)]
pub struct MailQueue {
    pool: Arc<DbPool>,
}

impl MailQueue {
    pub fn new(pool: Arc<DbPool>) -> Self {
        Self { pool }
    }

    pub async fn enqueue(&self, env: &MailEnvelope) -> Result<i64, MailQueueError> {
        let now = Utc::now();
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let res = sqlx::query(
                    "INSERT INTO oc_mail_queue \
                     (recipient, subject, html_body, text_body, event_type, attempts, \
                      next_attempt_at, state, claimed_at, last_error, created_at, sent_at) \
                     VALUES (?, ?, ?, ?, ?, 0, ?, 'Pending', NULL, NULL, ?, NULL)",
                )
                .bind(&env.recipient)
                .bind(&env.subject)
                .bind(&env.html_body)
                .bind(&env.text_body)
                .bind(env.event_type.as_str())
                .bind(now.naive_utc())
                .bind(now.naive_utc())
                .execute(p)
                .await?;
                Ok(res.last_insert_rowid())
            }
            DbPool::MySql(p) => {
                let res = sqlx::query(
                    "INSERT INTO oc_mail_queue \
                     (recipient, subject, html_body, text_body, event_type, attempts, \
                      next_attempt_at, state, claimed_at, last_error, created_at, sent_at) \
                     VALUES (?, ?, ?, ?, ?, 0, ?, 'Pending', NULL, NULL, ?, NULL)",
                )
                .bind(&env.recipient)
                .bind(&env.subject)
                .bind(&env.html_body)
                .bind(&env.text_body)
                .bind(env.event_type.as_str())
                .bind(now.naive_utc())
                .bind(now.naive_utc())
                .execute(p)
                .await?;
                Ok(res.last_insert_id() as i64)
            }
            DbPool::Postgres(p) => {
                let row = sqlx::query(
                    "INSERT INTO oc_mail_queue \
                     (recipient, subject, html_body, text_body, event_type, attempts, \
                      next_attempt_at, state, claimed_at, last_error, created_at, sent_at) \
                     VALUES ($1, $2, $3, $4, $5, 0, $6, 'Pending', NULL, NULL, $7, NULL) \
                     RETURNING id",
                )
                .bind(&env.recipient)
                .bind(&env.subject)
                .bind(&env.html_body)
                .bind(&env.text_body)
                .bind(env.event_type.as_str())
                .bind(now.naive_utc())
                .bind(now.naive_utc())
                .fetch_one(p)
                .await?;
                Ok(row.try_get::<i64, _>("id")?)
            }
        }
    }

    /// Claim up to `limit` rows that are ready to send. Atomically flips
    /// state Pending → Sending and stamps claimed_at.
    pub async fn claim_batch(&self, limit: i64) -> Result<Vec<MailQueueRow>, MailQueueError> {
        let now = Utc::now();
        let mut out = Vec::new();
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                // sqlite has no FOR UPDATE SKIP LOCKED; we accept the
                // race in single-node deployments by selecting then
                // updating in a small transaction.
                let rows = sqlx::query(
                    "SELECT id, recipient, subject, html_body, text_body, event_type, attempts \
                     FROM oc_mail_queue \
                     WHERE state = 'Pending' AND next_attempt_at <= ? \
                     ORDER BY id LIMIT ?",
                )
                .bind(now.naive_utc())
                .bind(limit)
                .fetch_all(p)
                .await?;
                for row in rows {
                    let id: i64 = row.try_get("id")?;
                    let recipient: String = row.try_get("recipient")?;
                    let subject: String = row.try_get("subject")?;
                    let html_body: String = row.try_get("html_body")?;
                    let text_body: String = row.try_get("text_body")?;
                    let event_type_str: String = row.try_get("event_type")?;
                    let attempts: i64 = row.try_get("attempts")?;
                    let event_type = EventType::from_str(&event_type_str)
                        .ok_or(MailQueueError::UnknownEventType(event_type_str))?;
                    let upd = sqlx::query(
                        "UPDATE oc_mail_queue SET state = 'Sending', claimed_at = ? \
                         WHERE id = ? AND state = 'Pending'",
                    )
                    .bind(now.naive_utc())
                    .bind(id)
                    .execute(p)
                    .await?;
                    if upd.rows_affected() == 1 {
                        out.push(MailQueueRow {
                            id,
                            recipient,
                            subject,
                            html_body,
                            text_body,
                            event_type,
                            attempts: attempts as i32,
                        });
                    }
                }
            }
            DbPool::MySql(p) | DbPool::Postgres(p) if matches!(self.pool.as_ref(), DbPool::Postgres(_)) => {
                let _ = p; // typecheck guard; the actual block is below
                unreachable!()
            }
            DbPool::MySql(p) => {
                // MySQL 8 supports FOR UPDATE SKIP LOCKED. Use it inside
                // a transaction.
                let mut tx = p.begin().await?;
                let rows = sqlx::query(
                    "SELECT id, recipient, subject, html_body, text_body, event_type, attempts \
                     FROM oc_mail_queue \
                     WHERE state = 'Pending' AND next_attempt_at <= ? \
                     ORDER BY id LIMIT ? FOR UPDATE SKIP LOCKED",
                )
                .bind(now.naive_utc())
                .bind(limit)
                .fetch_all(&mut *tx)
                .await?;
                for row in rows {
                    let id: i64 = row.try_get("id")?;
                    let recipient: String = row.try_get_unchecked("recipient")?;
                    let subject: String = row.try_get_unchecked("subject")?;
                    let html_body: String = row.try_get_unchecked("html_body")?;
                    let text_body: String = row.try_get_unchecked("text_body")?;
                    let event_type_str: String = row.try_get_unchecked("event_type")?;
                    let attempts: i32 = row.try_get("attempts")?;
                    let event_type = EventType::from_str(&event_type_str)
                        .ok_or(MailQueueError::UnknownEventType(event_type_str))?;
                    sqlx::query(
                        "UPDATE oc_mail_queue SET state = 'Sending', claimed_at = ? WHERE id = ?",
                    )
                    .bind(now.naive_utc())
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                    out.push(MailQueueRow {
                        id,
                        recipient,
                        subject,
                        html_body,
                        text_body,
                        event_type,
                        attempts,
                    });
                }
                tx.commit().await?;
            }
            DbPool::Postgres(p) => {
                let mut tx = p.begin().await?;
                let rows = sqlx::query(
                    "SELECT id, recipient, subject, html_body, text_body, event_type, attempts \
                     FROM oc_mail_queue \
                     WHERE state = 'Pending' AND next_attempt_at <= $1 \
                     ORDER BY id LIMIT $2 FOR UPDATE SKIP LOCKED",
                )
                .bind(now.naive_utc())
                .bind(limit)
                .fetch_all(&mut *tx)
                .await?;
                for row in rows {
                    let id: i64 = row.try_get("id")?;
                    let recipient: String = row.try_get("recipient")?;
                    let subject: String = row.try_get("subject")?;
                    let html_body: String = row.try_get("html_body")?;
                    let text_body: String = row.try_get("text_body")?;
                    let event_type_str: String = row.try_get("event_type")?;
                    let attempts: i32 = row.try_get("attempts")?;
                    let event_type = EventType::from_str(&event_type_str)
                        .ok_or(MailQueueError::UnknownEventType(event_type_str))?;
                    sqlx::query(
                        "UPDATE oc_mail_queue SET state = 'Sending', claimed_at = $1 WHERE id = $2",
                    )
                    .bind(now.naive_utc())
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                    out.push(MailQueueRow {
                        id,
                        recipient,
                        subject,
                        html_body,
                        text_body,
                        event_type,
                        attempts,
                    });
                }
                tx.commit().await?;
            }
        }
        Ok(out)
    }

    pub async fn mark_sent(&self, id: i64) -> Result<(), MailQueueError> {
        let now = Utc::now();
        match self.pool.as_ref() {
            DbPool::Sqlite(p) | DbPool::MySql(p) if matches!(self.pool.as_ref(), DbPool::MySql(_)) => {
                let _ = p; unreachable!()
            }
            DbPool::Sqlite(p) => {
                sqlx::query("UPDATE oc_mail_queue SET state='Sent', sent_at=? WHERE id=?")
                    .bind(now.naive_utc()).bind(id).execute(p).await?;
            }
            DbPool::MySql(p) => {
                sqlx::query("UPDATE oc_mail_queue SET state='Sent', sent_at=? WHERE id=?")
                    .bind(now.naive_utc()).bind(id).execute(p).await?;
            }
            DbPool::Postgres(p) => {
                sqlx::query("UPDATE oc_mail_queue SET state='Sent', sent_at=$1 WHERE id=$2")
                    .bind(now.naive_utc()).bind(id).execute(p).await?;
            }
        }
        Ok(())
    }

    pub async fn mark_failed_retry(&self, id: i64, err: &str, attempts: i32) -> Result<(), MailQueueError> {
        let idx = attempts.min(2) as usize;
        let backoff = Duration::from_secs(BACKOFF_SECS[idx] as u64);
        let next_attempt = Utc::now() + chrono::Duration::from_std(backoff).unwrap();
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                sqlx::query(
                    "UPDATE oc_mail_queue SET state='Pending', attempts=attempts+1, \
                     next_attempt_at=?, last_error=? WHERE id=?",
                ).bind(next_attempt.naive_utc()).bind(err).bind(id).execute(p).await?;
            }
            DbPool::MySql(p) => {
                sqlx::query(
                    "UPDATE oc_mail_queue SET state='Pending', attempts=attempts+1, \
                     next_attempt_at=?, last_error=? WHERE id=?",
                ).bind(next_attempt.naive_utc()).bind(err).bind(id).execute(p).await?;
            }
            DbPool::Postgres(p) => {
                sqlx::query(
                    "UPDATE oc_mail_queue SET state='Pending', attempts=attempts+1, \
                     next_attempt_at=$1, last_error=$2 WHERE id=$3",
                ).bind(next_attempt.naive_utc()).bind(err).bind(id).execute(p).await?;
            }
        }
        Ok(())
    }

    pub async fn mark_failed_permanent(&self, id: i64, err: &str) -> Result<(), MailQueueError> {
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                sqlx::query("UPDATE oc_mail_queue SET state='Failed', attempts=attempts+1, last_error=? WHERE id=?")
                    .bind(err).bind(id).execute(p).await?;
            }
            DbPool::MySql(p) => {
                sqlx::query("UPDATE oc_mail_queue SET state='Failed', attempts=attempts+1, last_error=? WHERE id=?")
                    .bind(err).bind(id).execute(p).await?;
            }
            DbPool::Postgres(p) => {
                sqlx::query("UPDATE oc_mail_queue SET state='Failed', attempts=attempts+1, last_error=$1 WHERE id=$2")
                    .bind(err).bind(id).execute(p).await?;
            }
        }
        Ok(())
    }

    /// Reclaim rows stuck in `Sending` for > 5 minutes. Run periodically
    /// by the worker.
    pub async fn reclaim_stuck(&self) -> Result<u64, MailQueueError> {
        let cutoff = Utc::now() - chrono::Duration::seconds(STUCK_SENDING_AFTER_SECS);
        let n = match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                sqlx::query("UPDATE oc_mail_queue SET state='Pending', claimed_at=NULL WHERE state='Sending' AND claimed_at < ?")
                    .bind(cutoff.naive_utc()).execute(p).await?.rows_affected()
            }
            DbPool::MySql(p) => {
                sqlx::query("UPDATE oc_mail_queue SET state='Pending', claimed_at=NULL WHERE state='Sending' AND claimed_at < ?")
                    .bind(cutoff.naive_utc()).execute(p).await?.rows_affected()
            }
            DbPool::Postgres(p) => {
                sqlx::query("UPDATE oc_mail_queue SET state='Pending', claimed_at=NULL WHERE state='Sending' AND claimed_at < $1")
                    .bind(cutoff.naive_utc()).execute(p).await?.rows_affected()
            }
        };
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    // Multidialect integration tests live in
    // crates/crabcloud-core/tests/mail_queue_e2e.rs. Inline unit tests
    // here cover backoff math only.

    #[test]
    fn backoff_indices_for_attempts() {
        use super::BACKOFF_SECS;
        assert_eq!(BACKOFF_SECS[0], 60);
        assert_eq!(BACKOFF_SECS[1], 300);
        assert_eq!(BACKOFF_SECS[2], 1800);
    }
}
```

(The dispatch shape uses repeated `match` arms because some pool variants compile under both Sqlite and MySql query strings — the duplication is intentional and matches the established `crabcloud-sharing/src/service.rs` shape.)

- [ ] **Step 2: Re-export from lib.rs**

In `crates/crabcloud-core/src/lib.rs`:

```rust
pub mod mail_queue;
pub use mail_queue::{MailQueue, MailQueueError, MailQueueRow};
```

- [ ] **Step 3: Add deps**

Add to `crates/crabcloud-core/Cargo.toml`:

```toml
crabcloud-mail = { workspace = true }
```

- [ ] **Step 4: Build**

```bash
cargo build -p crabcloud-core
```

Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/crabcloud-core/
git commit -m "core: MailQueue (enqueue/claim_batch/mark_*/reclaim_stuck)"
```

### Task B4: Multidialect integration tests for MailQueue

**Files:**
- Create: `crates/crabcloud-core/tests/mail_queue_e2e.rs`

- [ ] **Step 1: Write the tests**

Mirror the test scaffolding in `crates/crabcloud-sharing/tests/sharing_e2e.rs` (which has a `per_dialect!` macro). The new tests:

- `enqueue_then_claim_batch_returns_row` — insert one row, claim with limit 1, get it back.
- `mark_sent_transitions_state` — claim, mark_sent, re-query to confirm state.
- `mark_failed_retry_sets_next_attempt_at_with_backoff` — claim, mark_failed_retry with attempts=0, confirm next_attempt_at ≈ now+60s.
- `mark_failed_permanent_transitions_to_failed_state` — claim, mark_failed_permanent, re-query.
- `reclaim_stuck_returns_old_sending_to_pending` — manually insert a row in Sending with claimed_at 10 minutes ago, call reclaim_stuck, re-query to confirm state='Pending'.

Each test runs in the same harness as the sharing e2e tests; mysql + postgres variants are `#[ignore]`'d for non-docker dev runs.

- [ ] **Step 2: Run tests**

```bash
cargo test -p crabcloud-core --test mail_queue_e2e
```

Expected: 5 sqlite tests pass; mysql + postgres ignored.

- [ ] **Step 3: Commit**

```bash
git add crates/crabcloud-core/tests/mail_queue_e2e.rs
git commit -m "core(tests): MailQueue multidialect integration (5 cases)"
```

### Task B5: `NotificationPrefs` service

**Files:**
- Create: `crates/crabcloud-users/src/notification_prefs.rs`
- Modify: `crates/crabcloud-users/src/lib.rs`

- [ ] **Step 1: Write the service**

```rust
//! Per-user, per-event-type opt-out for email notifications.
//!
//! Default = enabled (true). Stored in `oc_user_notification_prefs`.

use crabcloud_db::DbPool;
use sqlx::Row as _;
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum NotificationPrefsError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

#[derive(Clone)]
pub struct NotificationPrefs {
    pool: Arc<DbPool>,
}

impl NotificationPrefs {
    pub fn new(pool: Arc<DbPool>) -> Self {
        Self { pool }
    }

    /// Returns the enabled state for `(user_id, event_type)`. Defaults to
    /// `true` when no row exists.
    pub async fn get(&self, user_id: &str, event_type: &str) -> Result<bool, NotificationPrefsError> {
        let row: Option<i16> = match self.pool.as_ref() {
            DbPool::Sqlite(p) => sqlx::query("SELECT enabled FROM oc_user_notification_prefs WHERE user_id = ? AND event_type = ?")
                .bind(user_id).bind(event_type).fetch_optional(p).await?
                .map(|r| r.try_get("enabled")).transpose()?,
            DbPool::MySql(p) => sqlx::query("SELECT enabled FROM oc_user_notification_prefs WHERE user_id = ? AND event_type = ?")
                .bind(user_id).bind(event_type).fetch_optional(p).await?
                .map(|r| r.try_get("enabled")).transpose()?,
            DbPool::Postgres(p) => sqlx::query("SELECT enabled FROM oc_user_notification_prefs WHERE user_id = $1 AND event_type = $2")
                .bind(user_id).bind(event_type).fetch_optional(p).await?
                .map(|r| r.try_get("enabled")).transpose()?,
        };
        Ok(row.map(|v| v != 0).unwrap_or(true))
    }

    pub async fn set(&self, user_id: &str, event_type: &str, enabled: bool) -> Result<(), NotificationPrefsError> {
        let v: i16 = if enabled { 1 } else { 0 };
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                sqlx::query(
                    "INSERT INTO oc_user_notification_prefs (user_id, event_type, enabled) VALUES (?, ?, ?) \
                     ON CONFLICT (user_id, event_type) DO UPDATE SET enabled = excluded.enabled",
                ).bind(user_id).bind(event_type).bind(v).execute(p).await?;
            }
            DbPool::MySql(p) => {
                sqlx::query(
                    "INSERT INTO oc_user_notification_prefs (user_id, event_type, enabled) VALUES (?, ?, ?) \
                     ON DUPLICATE KEY UPDATE enabled = VALUES(enabled)",
                ).bind(user_id).bind(event_type).bind(v).execute(p).await?;
            }
            DbPool::Postgres(p) => {
                sqlx::query(
                    "INSERT INTO oc_user_notification_prefs (user_id, event_type, enabled) VALUES ($1, $2, $3) \
                     ON CONFLICT (user_id, event_type) DO UPDATE SET enabled = EXCLUDED.enabled",
                ).bind(user_id).bind(event_type).bind(v).execute(p).await?;
            }
        }
        Ok(())
    }
}
```

- [ ] **Step 2: Re-export from `lib.rs`**

```rust
pub mod notification_prefs;
pub use notification_prefs::{NotificationPrefs, NotificationPrefsError};
```

- [ ] **Step 3: Add an integration test**

In `crates/crabcloud-users/tests/notification_prefs_e2e.rs`:

- `get_returns_true_by_default` — no row → true.
- `set_then_get_round_trips_false` — set false, get returns false.
- `set_then_set_true_round_trips` — toggle off then on → true.
- `set_is_per_event_type` — set share_created=false; link_emailed still defaults to true.

- [ ] **Step 4: Run + commit**

```bash
cargo test -p crabcloud-users --test notification_prefs_e2e
```

Expected: 4 tests pass.

```bash
git add crates/crabcloud-users/
git commit -m "users: NotificationPrefs get/set with default-true semantics"
```

### Task B6: `MailWorker` background task

**Files:**
- Create: `crates/crabcloud-core/src/mail_worker.rs`
- Modify: `crates/crabcloud-core/src/lib.rs`

- [ ] **Step 1: Write the worker**

```rust
//! Background task that drains `oc_mail_queue` and sends via `Mailer`.

use crate::mail_queue::MailQueue;
use crabcloud_mail::{EventType, MailEnvelope, MailError, Mailer};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

pub struct MailWorker {
    queue: MailQueue,
    mailer: Arc<Mailer>,
    shutdown: Arc<Notify>,
}

impl MailWorker {
    pub fn new(queue: MailQueue, mailer: Arc<Mailer>) -> (Self, Arc<Notify>) {
        let shutdown = Arc::new(Notify::new());
        (Self { queue, mailer, shutdown: shutdown.clone() }, shutdown)
    }

    pub async fn run(self) {
        let mut cycles = 0u64;
        loop {
            cycles += 1;
            if cycles.is_multiple_of(5) {
                if let Err(e) = self.queue.reclaim_stuck().await {
                    tracing::warn!(error = %e, "mail_worker.reclaim_stuck failed");
                }
            }
            let batch = match self.queue.claim_batch(8).await {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(error = %e, "mail_worker.claim_batch failed");
                    if self.sleep_or_shutdown(Duration::from_secs(5)).await {
                        return;
                    }
                    continue;
                }
            };
            if batch.is_empty() {
                if self.sleep_or_shutdown(Duration::from_secs(5)).await {
                    return;
                }
                continue;
            }
            for row in batch {
                let env = MailEnvelope {
                    recipient: row.recipient,
                    subject: row.subject,
                    html_body: row.html_body,
                    text_body: row.text_body,
                    event_type: row.event_type,
                };
                let send_result = self.mailer.send(&env).await;
                match send_result {
                    Ok(()) => {
                        let _ = self.queue.mark_sent(row.id).await;
                    }
                    Err(e) if e.is_transient() && row.attempts < 3 => {
                        let _ = self.queue.mark_failed_retry(row.id, &e.to_string(), row.attempts).await;
                    }
                    Err(e) => {
                        let _ = self.queue.mark_failed_permanent(row.id, &e.to_string()).await;
                    }
                }
            }
        }
    }

    /// Sleep for `dur` or return `true` if shutdown was signaled.
    async fn sleep_or_shutdown(&self, dur: Duration) -> bool {
        tokio::select! {
            _ = tokio::time::sleep(dur) => false,
            _ = self.shutdown.notified() => true,
        }
    }
}
```

Add to lib.rs:

```rust
pub mod mail_worker;
pub use mail_worker::MailWorker;
```

- [ ] **Step 2: Build**

```bash
cargo build -p crabcloud-core
```

- [ ] **Step 3: Commit**

```bash
git add crates/crabcloud-core/src/mail_worker.rs crates/crabcloud-core/src/lib.rs
git commit -m "core: MailWorker background task (drain, retry, reclaim)"
```

### Task B7: `AppState` wiring

**Files:**
- Modify: `crates/crabcloud-core/src/state.rs`

- [ ] **Step 1: Add fields + builder wiring**

In `crates/crabcloud-core/src/state.rs`, add to `AppState`:

```rust
    /// Mailer (transport-only). The MailWorker is the consumer; user code
    /// generally enqueues via `mail_queue` rather than calling this
    /// directly.
    pub mailer: Arc<crabcloud_mail::Mailer>,
    /// Persistent outbound-mail queue.
    pub mail_queue: crate::mail_queue::MailQueue,
    /// Per-user notification opt-out service.
    pub notification_prefs: crabcloud_users::NotificationPrefs,
    /// Mailer worker shutdown handle (for graceful shutdown in tests).
    pub mail_worker_shutdown: Arc<tokio::sync::Notify>,
```

In `AppStateBuilder::build()`, after the existing `publiclinks_auth`/`preview` setup, build:

```rust
let mail_transport_cfg = crabcloud_mail::TransportConfig {
    kind: match self.config.mail.transport.as_str() {
        "smtp" => crabcloud_mail::TransportKind::Smtp,
        "log" => crabcloud_mail::TransportKind::Log,
        _ => crabcloud_mail::TransportKind::Disabled,
    },
    smtp_host: self.config.mail.smtp_host.clone(),
    smtp_port: self.config.mail.smtp_port,
    smtp_username: self.config.mail.smtp_username.clone(),
    smtp_password: self.config.mail.smtp_password.clone(),
    smtp_security: match self.config.mail.smtp_security.as_str() {
        "tls" => crabcloud_mail::SmtpSecurity::Tls,
        "none" => crabcloud_mail::SmtpSecurity::None,
        _ => crabcloud_mail::SmtpSecurity::Starttls,
    },
    mail_from: self.config.mail.mail_from.clone(),
    mail_from_name: self.config.mail.mail_from_name.clone(),
};
let mailer = Arc::new(crabcloud_mail::Mailer::from_config(&mail_transport_cfg).map_err(Error::Mail)?);
let mail_queue = crate::mail_queue::MailQueue::new(pool.clone());
let notification_prefs = crabcloud_users::NotificationPrefs::new(pool.clone());

// Spawn the worker (unless transport=disabled).
let (mail_worker, mail_worker_shutdown) = crate::mail_worker::MailWorker::new(mail_queue.clone(), mailer.clone());
if !matches!(mail_transport_cfg.kind, crabcloud_mail::TransportKind::Disabled) {
    tokio::spawn(async move { mail_worker.run().await });
}
```

Re-exports for `TransportKind` / `SmtpSecurity` need to come from `crabcloud_mail::transport` — add `pub use transport::{SmtpSecurity, TransportKind};` to `crabcloud-mail/src/lib.rs`. If `crabcloud_mail::TransportConfig` exposes those types directly, no change needed.

Also add to `Error` enum in `crates/crabcloud-core/src/error.rs`:

```rust
    #[error("mail config invalid: {0}")]
    Mail(crabcloud_mail::MailError),
```

- [ ] **Step 2: Build + run existing tests**

```bash
cargo test --workspace
```

Fix any breakage. The wiring may break some existing tests that don't construct `mail_queue`; reviewing the new fields in `AppState::new` / builder builds.

- [ ] **Step 3: Commit**

```bash
git add crates/crabcloud-core/
git commit -m "core: AppState wires Mailer + MailQueue + NotificationPrefs + spawns MailWorker"
```

### Task B8: E2E test with log transport

**Files:**
- Create: `crates/crabcloud-core/tests/mail_log_transport_e2e.rs`

- [ ] **Step 1: Write the test**

```rust
//! End-to-end: enqueue a mail with the log transport active; assert the
//! worker drains it within a couple of polling cycles and emits the
//! `crabcloud_mail::log_transport` tracing event.

use crabcloud_mail::{EventType, MailEnvelope};
use tracing::subscriber;
use tracing_test::traced_test;

#[traced_test]
#[tokio::test]
async fn log_transport_drain_emits_tracing_event() {
    // Build an AppState with cfg.mail.transport = "log".
    // (Mirror the existing make_state helper from crabcloud-http/tests/support.)
    let (state, _tmp) = test_make_state_with_log_transport().await;
    let env = MailEnvelope {
        recipient: "bob@example.com".to_string(),
        subject: "Test".to_string(),
        html_body: "<p>hi</p>".to_string(),
        text_body: "hi".to_string(),
        event_type: EventType::ShareCreated,
    };
    state.mail_queue.enqueue(&env).await.unwrap();
    // Worker polls every 5s; wait up to 10s.
    for _ in 0..20 {
        if logs_contain("mail.transport=log envelope captured") {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    panic!("expected log transport to emit envelope tracing event within 10s");
}

async fn test_make_state_with_log_transport() -> (crabcloud_core::AppState, tempfile::TempDir) {
    // Construct minimal AppState with mail.transport="log". Adapt the
    // existing make_state harness from crabcloud-http/tests/support/mod.rs.
    todo!("adapt make_state — pull in crabcloud-config + crabcloud-core test support")
}
```

The `tracing-test` crate provides `logs_contain` for in-process log assertion. Add it to dev-deps if not already present.

The harness is tricky — there's no existing `test_make_state` in `crabcloud-core` itself. Either: (a) duplicate the helper inline; (b) bring the helper from `crates/crabcloud-http/tests/support/mod.rs` if you can. Whichever is cleaner.

- [ ] **Step 2: Run + commit**

```bash
cargo test -p crabcloud-core --test mail_log_transport_e2e
```

```bash
git add crates/crabcloud-core/tests/
git commit -m "core(tests): e2e log-transport drain emits tracing event"
```

### Task B9: Pre-PR sweep + PR

Standard. PR title: `sp11(b): MailConfig + DB queue + worker + NotificationPrefs + AppState wiring`.

---

# Batch C — Notification triggers + ShareType::Email + expiration sweeper

**Branch:** `sp11/c-notification-triggers`

**Goal:** Add `ShareType::Email` (share_type=4). Hook `Shares::create` for share_type=0/1 to enqueue share_created mails. Hook share_type=4 to enqueue link_emailed mails. Add the `last_warned` column and the expiration sweeper. OCS handler accepts shareType=4 + email validation. E2E tests across all four trigger paths.

### Task C1: Migration `0008_share_last_warned`

**Files:**
- Create: `migrations/core/0008_share_last_warned/{sqlite,mysql,postgres}.sql`

- [ ] **Step 1: Write the migrations** (3 dialects, one column add each)

`sqlite.sql`:
```sql
ALTER TABLE oc_share ADD COLUMN last_warned TIMESTAMP NULL;
```

`mysql.sql`:
```sql
ALTER TABLE oc_share ADD COLUMN last_warned TIMESTAMP NULL;
```

`postgres.sql`:
```sql
ALTER TABLE oc_share ADD COLUMN last_warned TIMESTAMP NULL;
```

- [ ] **Step 2: Update SQL constants in `crabcloud-sharing/src/sql.rs`**

Find every `SELECT id, share_type, ... FROM oc_share` and add `last_warned` to the column list. Same for INSERT bind lists.

- [ ] **Step 3: Update row decoders in `crabcloud-sharing/src/service.rs`**

Add `last_warned: Option<NaiveDateTime>` to `RowParts`, populate from the SELECT, assemble into `ShareRow`. Also add `last_warned: Option<DateTime<Utc>>` to `ShareRow`.

INSERT statements: add `NULL` for `last_warned`. The bind site for INSERT increases by 1.

- [ ] **Step 4: Run sharing tests**

```bash
cargo test -p crabcloud-sharing
```

Expected: pre-existing tests still pass with the column added.

- [ ] **Step 5: Commit**

```bash
git add migrations/core/0008_share_last_warned/ crates/crabcloud-sharing/src/sql.rs crates/crabcloud-sharing/src/service.rs
git commit -m "db+sharing: 0008_share_last_warned column + ShareRow extension"
```

### Task C2: `ShareType::Email`

**Files:**
- Modify: `crates/crabcloud-sharing/src/types.rs`

- [ ] **Step 1: Add the variant**

In `ShareType` enum:

```rust
pub enum ShareType {
    User = 0,
    Group = 1,
    Link = 3,
    Email = 4,
}
```

And the i16 conversion:

```rust
impl TryFrom<i16> for ShareType {
    type Error = &'static str;
    fn try_from(v: i16) -> Result<Self, Self::Error> {
        match v {
            0 => Ok(ShareType::User),
            1 => Ok(ShareType::Group),
            3 => Ok(ShareType::Link),
            4 => Ok(ShareType::Email),
            _ => Err("unsupported share_type"),
        }
    }
}
```

- [ ] **Step 2: Update `Shares::create` dispatch**

In `crates/crabcloud-sharing/src/service.rs`:

```rust
        if matches!(req.share_type, ShareType::Link | ShareType::Email) {
            return self.create_link(req).await;
        }
```

`create_link` itself doesn't need to discriminate yet — it generates a token + persists the row. The actual mail-enqueue happens in Task C4.

- [ ] **Step 3: Update existing test fixtures**

`rg "ShareType::" crates --type rust` — review every match arm to ensure `Email` is handled or explicitly skipped.

- [ ] **Step 4: Build + test**

```bash
cargo test -p crabcloud-sharing
```

- [ ] **Step 5: Commit**

```bash
git add crates/crabcloud-sharing/src/types.rs crates/crabcloud-sharing/src/service.rs
git commit -m "sharing: ShareType::Email (share_type=4) routes through create_link"
```

### Task C3: `share_created` hook in `Shares::create`

**Files:**
- Modify: `crates/crabcloud-sharing/src/service.rs`
- Modify: `crates/crabcloud-sharing/Cargo.toml` — add deps for `crabcloud-mail`, `crabcloud-users::NotificationPrefs`, and the queue.

- [ ] **Step 1: Make `Shares` carry a `MailQueue` + `NotificationPrefs`**

Since `crabcloud-sharing` already takes `Arc<UsersService>`, it can reach `notification_prefs` via that. The `MailQueue` is in `crabcloud-core` — which would be a cyclic dep. Workaround: define a trait `MailEnqueuer` in `crabcloud-sharing` (or a small bridge crate) and have `crabcloud-core::MailQueue` implement it. `Shares::new` takes `Arc<dyn MailEnqueuer>`.

Add to `crates/crabcloud-sharing/src/service.rs`:

```rust
#[async_trait::async_trait]
pub trait MailEnqueuer: Send + Sync {
    async fn enqueue(&self, env: &crabcloud_mail::MailEnvelope) -> Result<(), MailEnqueueError>;
}

#[derive(Debug, thiserror::Error)]
#[error("mail enqueue failed: {0}")]
pub struct MailEnqueueError(pub String);
```

And the `Shares` struct gets a new field `mail: Arc<dyn MailEnqueuer>`. Its constructor adds the parameter.

In `crates/crabcloud-core/src/mail_queue.rs`, impl the trait:

```rust
#[async_trait::async_trait]
impl crabcloud_sharing::MailEnqueuer for MailQueue {
    async fn enqueue(&self, env: &crabcloud_mail::MailEnvelope) -> Result<(), crabcloud_sharing::MailEnqueueError> {
        self.enqueue(env).await
            .map(|_| ())
            .map_err(|e| crabcloud_sharing::MailEnqueueError(e.to_string()))
    }
}
```

- [ ] **Step 2: Add the post-insert hook in `Shares::create` for user/group shares**

```rust
        // After successful row insert for share_type=User/Group, enqueue
        // share_created mail if the recipient has an email + opted in.
        if matches!(req.share_type, ShareType::User) {
            let recipient_uid = UserId::new(req.share_with.clone()).ok();
            if let Some(uid) = recipient_uid {
                self.try_enqueue_share_created_mail(&uid, &req.requester, &storage_path).await;
            }
        }
```

And define:

```rust
    async fn try_enqueue_share_created_mail(
        &self,
        recipient_uid: &UserId,
        owner_uid: &str,
        storage_path: &StoragePath,
    ) {
        // 1. Look up recipient email + display name.
        let user = match self.users.user_store().lookup(recipient_uid).await {
            Ok(Some(u)) => u,
            _ => return, // no user, no mail
        };
        let email = match &user.email {
            Some(e) => e.as_str().to_string(),
            None => return,
        };
        // 2. Check opt-out.
        let prefs = self.users.notification_prefs(); // accessor we add to UsersService
        match prefs.get(recipient_uid.as_str(), "share_created").await {
            Ok(false) => return,
            Err(e) => {
                tracing::warn!(error = %e, "notification_prefs.get failed; skipping mail");
                return;
            }
            Ok(true) => {}
        };
        // 3. Look up owner display name.
        let owner_display = match UserId::new(owner_uid.to_string()).ok()
            .and_then(|uid| { /* sync lookup not available; reuse user_store */ None }) {
            Some(u) => u,
            None => owner_uid.to_string(),
        };
        // 4. Render + enqueue.
        let ctx = crabcloud_mail::TemplateContext {
            lang: "en".to_string(),
            instance_url: "https://crabcloud.example".to_string(), // TODO: pull from config
            recipient_display_name: user.display_name.clone(),
            recipient_email: email,
            event_specific: serde_json::json!({
                "owner_display_name": owner_display,
                "path_basename": storage_path.basename(),
            }),
        };
        let env = match crabcloud_mail::render_template(crabcloud_mail::EventType::ShareCreated, ctx) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(error = %e, "render_template failed; skipping mail");
                return;
            }
        };
        if let Err(e) = self.mail.enqueue(&env).await {
            tracing::warn!(error = %e, "mail.enqueue failed; share still succeeded");
        }
    }
```

The `instance_url` and owner-display-name lookup are TBD-ish here — the implementer will need to plumb `FileConfig::overwrite_cli_url` (or similar) through `Shares::new` as a string, OR add a method `UsersService::display_name_of(uid)`. Take the cleanest path.

- [ ] **Step 3: Build + test**

```bash
cargo test -p crabcloud-sharing
```

- [ ] **Step 4: Commit**

```bash
git add crates/crabcloud-sharing/ crates/crabcloud-core/src/mail_queue.rs
git commit -m "sharing: enqueue share_created mail on user-share create (opt-in via prefs)"
```

### Task C4: `link_emailed` hook + OCS shareType=4 wiring

**Files:**
- Modify: `crates/crabcloud-sharing/src/service.rs`
- Modify: `crates/crabcloud-http/src/routes/ocs/files_sharing.rs`

- [ ] **Step 1: In `Shares::create_link`, after the row is inserted, if `share_type == Email`, enqueue.**

Mirror the share_created path. Recipient email is `req.share_with` directly (not a uid). Template context includes `link_url`, `password_protected`, `expiration` (already in `req.expire_date`).

The `link_url` needs the host prefix. Pull from `FileConfig::overwrite_cli_url` if available, fallback to `"/s/<token>"` (relative).

- [ ] **Step 2: OCS handler validates the email**

In `crates/crabcloud-http/src/routes/ocs/files_sharing.rs::create_handler`, when `shareType=4`, validate `form.share_with` as an email via `crabcloud_users::Email::parse`. Reject with `400 InvalidEmail` on failure.

- [ ] **Step 3: E2E tests**

Add to `crates/crabcloud-http/tests/files_sharing_e2e.rs`:

- `ocs_create_email_share_enqueues_mail_in_log_transport`: POST with shareType=4, valid email; assert a tracing event with subject "A file has been shared with you" was emitted.
- `ocs_create_email_share_with_invalid_email_returns_400`.

- [ ] **Step 4: Run + commit**

```bash
cargo test -p crabcloud-http --test files_sharing_e2e
```

```bash
git add crates/crabcloud-sharing/ crates/crabcloud-http/
git commit -m "sharing+ocs: shareType=4 (email-link) enqueues link_emailed mail"
```

### Task C5: `ExpirationWarningSweeper` background task

**Files:**
- Create: `crates/crabcloud-core/src/expiration_sweeper.rs`
- Modify: `crates/crabcloud-core/src/state.rs` — spawn the task

- [ ] **Step 1: Write the sweeper**

```rust
//! Background task: hourly sweep oc_share for links expiring in <24h,
//! enqueue expiration_warning mails, stamp last_warned.

use crate::mail_queue::MailQueue;
use crabcloud_db::DbPool;
use crabcloud_mail::{EventType, TemplateContext, render_template};
use crabcloud_users::{NotificationPrefs, UsersService};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

pub struct ExpirationWarningSweeper {
    pool: Arc<DbPool>,
    queue: MailQueue,
    users: UsersService,
    prefs: NotificationPrefs,
    shutdown: Arc<Notify>,
}

impl ExpirationWarningSweeper {
    pub fn new(
        pool: Arc<DbPool>,
        queue: MailQueue,
        users: UsersService,
        prefs: NotificationPrefs,
    ) -> (Self, Arc<Notify>) {
        let shutdown = Arc::new(Notify::new());
        (
            Self { pool, queue, users, prefs, shutdown: shutdown.clone() },
            shutdown,
        )
    }

    pub async fn run(self) {
        loop {
            if let Err(e) = self.sweep().await {
                tracing::warn!(error = %e, "expiration_sweeper.sweep failed");
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(3600)) => {}
                _ = self.shutdown.notified() => return,
            }
        }
    }

    async fn sweep(&self) -> Result<(), sqlx::Error> {
        let rows = self.select_expiring_links().await?;
        for row in rows {
            // Look up owner email + prefs.
            // (Implementation: similar to share_created hook in Task C3.)
            // Enqueue if opted in.
            // Stamp last_warned regardless.
            let _ = row; // omitted body details — see plan §3.3
        }
        Ok(())
    }

    async fn select_expiring_links(&self) -> Result<Vec<ExpiringLink>, sqlx::Error> {
        // SELECT id, uid_owner, file_target, token, expiration FROM oc_share
        // WHERE share_type IN (3,4) AND expiration IS NOT NULL
        //   AND expiration > now() AND expiration <= now()+24h
        //   AND last_warned IS NULL
        // ORDER BY id LIMIT 200
        // Per-dialect query string + bind shape.
        // Returns Vec<ExpiringLink>.
        todo!("dialect dispatch")
    }
}

struct ExpiringLink {
    id: i64,
    uid_owner: String,
    file_target: String,
    token: String,
    expiration: chrono::DateTime<chrono::Utc>,
}
```

This task is the most substantial in Batch C; budget ~30 min. Use the existing per-dialect dispatch shape from `MailQueue`.

- [ ] **Step 2: Spawn the sweeper from `AppStateBuilder::build`**

```rust
let (sweeper, sweeper_shutdown) = crate::expiration_sweeper::ExpirationWarningSweeper::new(
    pool.clone(),
    mail_queue.clone(),
    users.clone(),
    notification_prefs.clone(),
);
if !matches!(mail_transport_cfg.kind, crabcloud_mail::TransportKind::Disabled) {
    tokio::spawn(async move { sweeper.run().await });
}
```

- [ ] **Step 3: Multidialect integration test**

In `crates/crabcloud-core/tests/expiration_sweeper_e2e.rs`:

- `sweep_finds_links_in_24h_window` — seed link with expiration=now+12h; run sweep manually (don't rely on the timer); assert one queue row enqueued + last_warned stamped.
- `sweep_skips_already_warned` — same setup, run twice, only one queue row.
- `sweep_skips_outside_window` — links expiring in 48h or already expired → no enqueue.
- `sweep_respects_owner_opt_out` — owner with `expiration_warning=false` → no enqueue but last_warned still stamped.

- [ ] **Step 4: Run + commit**

```bash
cargo test -p crabcloud-core --test expiration_sweeper_e2e
```

```bash
git add crates/crabcloud-core/
git commit -m "core: ExpirationWarningSweeper hourly background task (T-1 day warnings)"
```

### Task C6: Pre-PR sweep + PR

Standard. PR title: `sp11(c): notification triggers (ShareType::Email + share_created hook + expiration sweeper)`.

---

# Batch D — Settings UI

**Branch:** `sp11/d-settings-ui`

**Goal:** Add server-fns + dx page for the user-facing notification-prefs toggle panel.

### Task D1: server-fns for notification_prefs

**Files:**
- Create: `crates/crabcloud-app/src/server_fns/notification_prefs.rs`
- Modify: `crates/crabcloud-app/src/server_fns/mod.rs` — register module

- [ ] **Step 1: Write the server-fns**

```rust
//! `/api/notification_prefs/get` and `/api/notification_prefs/set`.

use crabcloud_core::AppState;
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationPrefsDto {
    pub share_created: bool,
    pub link_emailed: bool,
    pub expiration_warning: bool,
}

#[server(endpoint = "/api/notification_prefs/get")]
pub async fn notification_prefs_get() -> Result<NotificationPrefsDto, ServerFnError> {
    // 1. Resolve AppState + user from FullstackContext.
    // 2. Query notification_prefs.get for the 3 event types.
    // 3. Return DTO with each.
    todo!("mirror pattern from existing server-fns")
}

#[server(endpoint = "/api/notification_prefs/set")]
pub async fn notification_prefs_set(
    event_type: String,
    enabled: bool,
) -> Result<(), ServerFnError> {
    // 1. Resolve AppState + user.
    // 2. Validate event_type is one of the 3 known strings.
    // 3. notification_prefs.set(uid, event_type, enabled).
    todo!()
}
```

- [ ] **Step 2: Commit**

```bash
git add crates/crabcloud-app/src/server_fns/
git commit -m "app(server_fns): notification_prefs get/set"
```

### Task D2: Settings page

**Files:**
- Create: `crates/crabcloud-app/src/pages/settings/notifications.rs`
- Modify: `crates/crabcloud-app/src/app.rs` — register route

- [ ] **Step 1: Write the page**

```rust
use crate::server_fns::notification_prefs::{
    notification_prefs_get, notification_prefs_set, NotificationPrefsDto,
};
use dioxus::prelude::*;

#[component]
pub fn NotificationSettings() -> Element {
    let prefs = use_resource(|| async move { notification_prefs_get().await.ok() });

    let toggle = move |event_type: &'static str, new_value: bool| {
        spawn(async move {
            let _ = notification_prefs_set(event_type.to_string(), new_value).await;
            prefs.restart();
        });
    };

    rsx! {
        section { class: "notifications-settings",
            h1 { "Email notifications" }
            p { "Choose which events trigger an email to you." }
            if let Some(Some(prefs_val)) = prefs.read().as_ref() {
                // Three toggle rows. See dx-idiom for checkboxes elsewhere
                // in pages/settings.
                ToggleRow {
                    label: "Notify me when others share with me",
                    enabled: prefs_val.share_created,
                    on_toggle: move |v| toggle("share_created", v),
                }
                ToggleRow {
                    label: "Send a copy of email-share confirmations",
                    enabled: prefs_val.link_emailed,
                    on_toggle: move |v| toggle("link_emailed", v),
                }
                ToggleRow {
                    label: "Warn me before my public links expire",
                    enabled: prefs_val.expiration_warning,
                    on_toggle: move |v| toggle("expiration_warning", v),
                }
            }
        }
    }
}

#[component]
fn ToggleRow(label: &'static str, enabled: bool, on_toggle: EventHandler<bool>) -> Element {
    rsx! {
        label { class: "notification-toggle",
            input {
                r#type: "checkbox",
                checked: enabled,
                onchange: move |evt| {
                    on_toggle.call(evt.value() == "true");
                },
            }
            "{label}"
        }
    }
}
```

- [ ] **Step 2: Register route**

In `crates/crabcloud-app/src/app.rs`'s Routable enum, add a variant for `/settings/notifications` pointing at `NotificationSettings`.

- [ ] **Step 3: Build the WASM bundle**

```bash
cargo build -p crabcloud-app --target wasm32-unknown-unknown --no-default-features --features web
```

- [ ] **Step 4: Commit**

```bash
git add crates/crabcloud-app/
git commit -m "app(settings): notification-prefs panel with 3 toggles"
```

### Task D3: SSR snapshot

**Files:**
- Modify: `crates/crabcloud-app/tests/server_fns_files.rs` or new test file

- [ ] **Step 1: Snapshot the page**

Assert the rendered HTML contains the three labels: `"Notify me when others share with me"`, `"Send a copy of email-share confirmations"`, `"Warn me before my public links expire"`.

- [ ] **Step 2: Commit**

```bash
git add crates/crabcloud-app/tests/
git commit -m "app(tests): SSR snapshot for notification-prefs page"
```

### Task D4: Pre-PR sweep + PR

Standard. PR title: `sp11(d): settings UI for notification preferences`.

---

## Acceptance criteria (spec → coverage map)

| Spec section | Test / artifact |
|---|---|
| §2 Decision 1 (new crate) | Batch A Task A1. |
| §2 Decision 2 (lettre + rustls) | Task A3 + transport tests. |
| §2 Decision 3 (tera + rust-embed) | Task A5 + A6 templates + tests. |
| §2 Decision 4 (DB queue) | Batch B Task B2 migration + B3 service + B4 integration tests. |
| §2 Decision 5 (worker) | Batch B Task B6 + B8 e2e. |
| §2 Decision 6 (3 transports) | Task A3 tests cover Smtp/Log/Disabled. |
| §2 Decision 7 (FileConfig.mail) | Task B1. |
| §2 Decision 8 (per-event opt-out) | Task B5 + Batch D UI. |
| §2 Decision 9 (ShareType::Email) | Batch C Task C2 + C4. |
| §2 Decision 10 (expiration sweeper) | Batch C Task C5 + integration tests. |
| §2 Decision 11 (multipart MIME) | Task A4 + e2e. |
| §2 Decision 12 (settings UI) | Batch D Task D2 + D3 snapshot. |
| §3.1-3.5 data flows | E2E tests across batches. |
| §4 testing strategy | Mapped above. |
| §5 risks | Mitigations baked into implementation. |
