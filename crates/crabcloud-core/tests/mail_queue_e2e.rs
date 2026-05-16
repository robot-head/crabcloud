//! End-to-end tests for [`crabcloud_core::MailQueue`] across sqlite,
//! mysql, and postgres. The sqlite variants run on every `cargo test`
//! invocation; the mysql + postgres variants are `#[ignore]`'d so they
//! only run under `cargo xtask up` (testcontainers/docker).
//!
//! The test scaffolding mirrors `crabcloud-sharing/tests/sharing_e2e.rs`
//! (per-dialect `Fixture` + a `per_dialect!` macro that emits the three
//! sqlite/mysql/postgres test wrappers).

#![allow(unused_crate_dependencies)]

use chrono::{NaiveDateTime, Utc};
use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_config::{DbType, FileConfig};
use crabcloud_core::MailQueue;
use crabcloud_db::{core_set, DbPool, MigrationRunner};
use crabcloud_mail::{EventType, MailEnvelope};
use secrecy::SecretString;
use sqlx::Row as _;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum FixtureKind {
    Sqlite,
    MySql,
    Postgres,
}

struct Fixture {
    pool: Arc<DbPool>,
    queue: MailQueue,
    _tempdir: Option<TempDir>,
}

impl Fixture {
    async fn new(kind: FixtureKind) -> Self {
        let (pool, tempdir) = match kind {
            FixtureKind::Sqlite => {
                let dir = tempfile::tempdir().unwrap();
                let cfg = minimal_sqlite_config(dir.path().join("mailq.db"));
                let pool = DbPool::connect(&cfg).await.unwrap();
                run_migrations(&pool, &cfg).await;
                (pool, Some(dir))
            }
            FixtureKind::MySql => {
                let url = std::env::var("CRABCLOUD_TEST_MYSQL_URL").unwrap_or_else(|_| {
                    "mysql://crabcloud:crabcloud@127.0.0.1:3307/crabcloud".into()
                });
                let cfg = mysql_config_from_url(&url);
                let pool = DbPool::connect(&cfg).await.unwrap();
                reset_external(&pool).await;
                run_migrations(&pool, &cfg).await;
                (pool, None)
            }
            FixtureKind::Postgres => {
                let url = std::env::var("CRABCLOUD_TEST_POSTGRES_URL").unwrap_or_else(|_| {
                    "postgres://crabcloud:crabcloud@127.0.0.1:5433/crabcloud".into()
                });
                let cfg = postgres_config_from_url(&url);
                let pool = DbPool::connect(&cfg).await.unwrap();
                reset_external(&pool).await;
                run_migrations(&pool, &cfg).await;
                (pool, None)
            }
        };
        let pool_arc = Arc::new(pool);
        let queue = MailQueue::new(pool_arc.clone());
        Self {
            pool: pool_arc,
            queue,
            _tempdir: tempdir,
        }
    }
}

async fn run_migrations(pool: &DbPool, cfg: &FileConfig) {
    let mut runner = MigrationRunner::new(pool, &cfg.dbtableprefix);
    runner.register(core_set());
    runner.run().await.unwrap();
}

async fn reset_external(pool: &DbPool) {
    // Same drop set used by the sharing e2e + migration tests, with the
    // mail-related tables at the front of the list (no FKs against
    // them, but drop early to be tidy).
    let tables = [
        "oc_user_notification_prefs",
        "oc_mail_queue",
        "oc_share",
        "oc_properties",
        "oc_filelocks",
        "oc_filecache",
        "oc_mimetypes",
        "oc_storages",
        "oc_authtoken",
        "oc_preferences",
        "oc_group_user",
        "oc_groups",
        "oc_users",
        "oc_appconfig",
        "oc_migrations",
    ];
    match pool {
        DbPool::MySql(p) => {
            for t in tables {
                let _ = sqlx::query(&format!("DROP TABLE IF EXISTS {t}"))
                    .execute(p)
                    .await;
            }
        }
        DbPool::Postgres(p) => {
            for t in tables {
                let _ = sqlx::query(&format!("DROP TABLE IF EXISTS {t}"))
                    .execute(p)
                    .await;
            }
        }
        DbPool::Sqlite(_) => {}
    }
}

fn envelope(recipient: &str, event_type: EventType) -> MailEnvelope {
    MailEnvelope {
        recipient: recipient.to_string(),
        subject: "Subject".into(),
        html_body: "<p>html</p>".into(),
        text_body: "text".into(),
        event_type,
    }
}

async fn fetch_state_and_next_attempt(
    pool: &DbPool,
    id: i64,
) -> (String, NaiveDateTime, i32, Option<String>) {
    match pool {
        DbPool::Sqlite(p) => {
            let row = sqlx::query(
                "SELECT state, next_attempt_at, attempts, last_error FROM oc_mail_queue WHERE id = ?",
            )
            .bind(id)
            .fetch_one(p)
            .await
            .unwrap();
            (
                row.try_get("state").unwrap(),
                row.try_get("next_attempt_at").unwrap(),
                row.try_get::<i64, _>("attempts").unwrap() as i32,
                row.try_get("last_error").unwrap(),
            )
        }
        DbPool::MySql(p) => {
            let row = sqlx::query(
                "SELECT state, next_attempt_at, attempts, last_error FROM oc_mail_queue WHERE id = ?",
            )
            .bind(id)
            .fetch_one(p)
            .await
            .unwrap();
            (
                row.try_get("state").unwrap(),
                row.try_get("next_attempt_at").unwrap(),
                row.try_get::<i32, _>("attempts").unwrap(),
                row.try_get("last_error").unwrap(),
            )
        }
        DbPool::Postgres(p) => {
            let row = sqlx::query(
                "SELECT state, next_attempt_at, attempts, last_error FROM oc_mail_queue WHERE id = $1",
            )
            .bind(id)
            .fetch_one(p)
            .await
            .unwrap();
            (
                row.try_get("state").unwrap(),
                row.try_get("next_attempt_at").unwrap(),
                row.try_get::<i32, _>("attempts").unwrap(),
                row.try_get("last_error").unwrap(),
            )
        }
    }
}

// ---------------- scenarios ----------------

async fn enqueue_then_claim_batch_returns_row(fx: &Fixture) {
    let id = fx
        .queue
        .enqueue(&envelope("bob@example.com", EventType::ShareCreated))
        .await
        .unwrap();
    assert!(id > 0);

    let batch = fx.queue.claim_batch(10).await.unwrap();
    assert_eq!(batch.len(), 1);
    let row = &batch[0];
    assert_eq!(row.id, id);
    assert_eq!(row.recipient, "bob@example.com");
    assert_eq!(row.subject, "Subject");
    assert_eq!(row.event_type, EventType::ShareCreated);
    assert_eq!(row.attempts, 0);

    // After the claim, the row is `Sending`. Another claim should not
    // re-fetch it.
    let again = fx.queue.claim_batch(10).await.unwrap();
    assert_eq!(again.len(), 0);
}

async fn mark_sent_transitions_state(fx: &Fixture) {
    let id = fx
        .queue
        .enqueue(&envelope("bob@example.com", EventType::ShareCreated))
        .await
        .unwrap();
    let _ = fx.queue.claim_batch(1).await.unwrap();
    fx.queue.mark_sent(id).await.unwrap();
    let (state, _next, _attempts, last_error) = fetch_state_and_next_attempt(&fx.pool, id).await;
    assert_eq!(state, "Sent");
    assert!(last_error.is_none());
}

async fn mark_failed_retry_sets_next_attempt_at_with_backoff(fx: &Fixture) {
    let id = fx
        .queue
        .enqueue(&envelope("bob@example.com", EventType::ShareCreated))
        .await
        .unwrap();
    let batch = fx.queue.claim_batch(1).await.unwrap();
    let row = &batch[0];
    let before = Utc::now().naive_utc();
    fx.queue
        .mark_failed_retry(id, "transient: smtp down", row.attempts)
        .await
        .unwrap();
    let (state, next, attempts, last_error) = fetch_state_and_next_attempt(&fx.pool, id).await;
    assert_eq!(state, "Pending");
    assert_eq!(attempts, 1);
    assert_eq!(last_error.as_deref(), Some("transient: smtp down"));
    // First-retry backoff is 60s. Allow a generous +/- window so
    // clock skew between the test and the database doesn't flake.
    let delta_secs = (next - before).num_seconds();
    assert!(
        (50..=70).contains(&delta_secs),
        "expected next_attempt_at ~60s after now; got delta={delta_secs}s"
    );
}

async fn mark_failed_permanent_transitions_to_failed_state(fx: &Fixture) {
    let id = fx
        .queue
        .enqueue(&envelope("bob@example.com", EventType::ShareCreated))
        .await
        .unwrap();
    let _ = fx.queue.claim_batch(1).await.unwrap();
    fx.queue
        .mark_failed_permanent(id, "permanent: bad recipient")
        .await
        .unwrap();
    let (state, _next, attempts, last_error) = fetch_state_and_next_attempt(&fx.pool, id).await;
    assert_eq!(state, "Failed");
    assert_eq!(attempts, 1);
    assert_eq!(last_error.as_deref(), Some("permanent: bad recipient"));
}

async fn reclaim_stuck_returns_old_sending_to_pending(fx: &Fixture) {
    // Insert a row in Sending state with claimed_at 10 minutes ago.
    let ten_minutes_ago = (Utc::now() - chrono::Duration::seconds(600)).naive_utc();
    let now = Utc::now().naive_utc();
    let id: i64 = match fx.pool.as_ref() {
        DbPool::Sqlite(p) => {
            let res = sqlx::query(
                "INSERT INTO oc_mail_queue \
                 (recipient, subject, html_body, text_body, event_type, attempts, \
                  next_attempt_at, state, claimed_at, last_error, created_at, sent_at) \
                 VALUES (?, ?, ?, ?, ?, 0, ?, 'Sending', ?, NULL, ?, NULL)",
            )
            .bind("stuck@example.com")
            .bind("Stuck")
            .bind("<p>stuck</p>")
            .bind("stuck")
            .bind("share_created")
            .bind(now)
            .bind(ten_minutes_ago)
            .bind(now)
            .execute(p)
            .await
            .unwrap();
            res.last_insert_rowid()
        }
        DbPool::MySql(p) => {
            let res = sqlx::query(
                "INSERT INTO oc_mail_queue \
                 (recipient, subject, html_body, text_body, event_type, attempts, \
                  next_attempt_at, state, claimed_at, last_error, created_at, sent_at) \
                 VALUES (?, ?, ?, ?, ?, 0, ?, 'Sending', ?, NULL, ?, NULL)",
            )
            .bind("stuck@example.com")
            .bind("Stuck")
            .bind("<p>stuck</p>")
            .bind("stuck")
            .bind("share_created")
            .bind(now)
            .bind(ten_minutes_ago)
            .bind(now)
            .execute(p)
            .await
            .unwrap();
            res.last_insert_id() as i64
        }
        DbPool::Postgres(p) => {
            let row = sqlx::query(
                "INSERT INTO oc_mail_queue \
                 (recipient, subject, html_body, text_body, event_type, attempts, \
                  next_attempt_at, state, claimed_at, last_error, created_at, sent_at) \
                 VALUES ($1, $2, $3, $4, $5, 0, $6, 'Sending', $7, NULL, $8, NULL) \
                 RETURNING id",
            )
            .bind("stuck@example.com")
            .bind("Stuck")
            .bind("<p>stuck</p>")
            .bind("stuck")
            .bind("share_created")
            .bind(now)
            .bind(ten_minutes_ago)
            .bind(now)
            .fetch_one(p)
            .await
            .unwrap();
            row.try_get::<i64, _>("id").unwrap()
        }
    };

    let reclaimed = fx.queue.reclaim_stuck().await.unwrap();
    assert!(reclaimed >= 1);
    let (state, _next, _attempts, _err) = fetch_state_and_next_attempt(&fx.pool, id).await;
    assert_eq!(state, "Pending");
}

// ---------------- per-dialect test wrappers ----------------

macro_rules! per_dialect {
    ($name:ident) => {
        paste::paste! {
            #[tokio::test]
            async fn [<$name _works_on_sqlite>]() {
                let fx = Fixture::new(FixtureKind::Sqlite).await;
                $name(&fx).await;
            }

            #[tokio::test]
            #[ignore = "needs docker / testcontainers"]
            async fn [<$name _works_on_mysql>]() {
                let fx = Fixture::new(FixtureKind::MySql).await;
                $name(&fx).await;
            }

            #[tokio::test]
            #[ignore = "needs docker / testcontainers"]
            async fn [<$name _works_on_postgres>]() {
                let fx = Fixture::new(FixtureKind::Postgres).await;
                $name(&fx).await;
            }
        }
    };
}

per_dialect!(enqueue_then_claim_batch_returns_row);
per_dialect!(mark_sent_transitions_state);
per_dialect!(mark_failed_retry_sets_next_attempt_at_with_backoff);
per_dialect!(mark_failed_permanent_transitions_to_failed_state);
per_dialect!(reclaim_stuck_returns_old_sending_to_pending);

// --- URL → config helpers, copy of the pattern used in
//     crates/crabcloud-sharing/tests/common/mod.rs.

fn mysql_config_from_url(url: &str) -> FileConfig {
    let parsed = parse_url(url);
    let mut cfg = minimal_sqlite_config(PathBuf::from("ignored.db"));
    cfg.dbtype = DbType::Mysql;
    cfg.dbhost = Some(parsed.host);
    cfg.dbport = Some(parsed.port);
    cfg.dbuser = Some(parsed.user);
    cfg.dbpassword = parsed.password.map(|p| SecretString::new(p.into()));
    cfg.dbname = parsed.database;
    cfg
}

fn postgres_config_from_url(url: &str) -> FileConfig {
    let parsed = parse_url(url);
    let mut cfg = minimal_sqlite_config(PathBuf::from("ignored.db"));
    cfg.dbtype = DbType::Pgsql;
    cfg.dbhost = Some(parsed.host);
    cfg.dbport = Some(parsed.port);
    cfg.dbuser = Some(parsed.user);
    cfg.dbpassword = parsed.password.map(|p| SecretString::new(p.into()));
    cfg.dbname = parsed.database;
    cfg
}

struct ParsedUrl {
    user: String,
    password: Option<String>,
    host: String,
    port: u16,
    database: String,
}

fn parse_url(url: &str) -> ParsedUrl {
    let after_scheme = url.split_once("://").expect("scheme").1;
    let (auth, host_db) = after_scheme.split_once('@').expect("auth");
    let (user, password) = match auth.split_once(':') {
        Some((u, p)) => (u.to_string(), Some(p.to_string())),
        None => (auth.to_string(), None),
    };
    let (host_port, database) = host_db.split_once('/').expect("path");
    let (host, port) = match host_port.split_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap()),
        None => (host_port.to_string(), 0),
    };
    ParsedUrl {
        user,
        password,
        host,
        port,
        database: database.to_string(),
    }
}
