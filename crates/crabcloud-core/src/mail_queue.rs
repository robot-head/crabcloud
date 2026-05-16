//! `MailQueue` — persistent queue for outbound mail. Workers claim
//! batches and call `mark_sent` / `mark_failed_*`; the expiration
//! sweeper and the Shares hooks call `enqueue`.
//!
//! The on-disk schema is in migration `0007_mail_queue_and_notification_prefs`.
//! Multidialect dispatch mirrors `crabcloud-sharing::service` —
//! explicit `match self.pool.as_ref()` arms with per-dialect query
//! strings (sqlite + mysql use `?` placeholders; postgres uses `$N`).

use chrono::Utc;
use crabcloud_db::DbPool;
use crabcloud_mail::{EventType, MailEnvelope};
use sqlx::Row as _;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

/// Errors raised by [`MailQueue`].
#[derive(Debug, Error)]
pub enum MailQueueError {
    /// Underlying database error.
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    /// Row had an `event_type` string that doesn't parse into
    /// [`EventType`]. Forward-compatible: a newer server may have
    /// written an event type this binary doesn't know about.
    #[error("unknown event_type in row: {0}")]
    UnknownEventType(String),
}

/// A row claimed for sending by [`MailQueue::claim_batch`]. Only the
/// fields the worker actually needs are surfaced.
#[derive(Debug, Clone)]
pub struct MailQueueRow {
    /// Primary key in `oc_mail_queue`.
    pub id: i64,
    /// Envelope recipient address.
    pub recipient: String,
    /// Rendered subject line.
    pub subject: String,
    /// Rendered HTML body.
    pub html_body: String,
    /// Rendered plain-text body.
    pub text_body: String,
    /// Notification event this row represents.
    pub event_type: EventType,
    /// Attempt count *before* this attempt. 0 on first send.
    pub attempts: i32,
}

/// Backoff schedule (seconds) keyed by current attempt count. After
/// `BACKOFF_SECS.len()` retries the row is marked `Failed`.
const BACKOFF_SECS: [i64; 3] = [60, 300, 1800];
/// Rows whose `state='Sending'` and `claimed_at` is older than this
/// are reclaimed by [`MailQueue::reclaim_stuck`].
const STUCK_SENDING_AFTER_SECS: i64 = 300; // 5 minutes

/// Persistent outbound-mail queue backed by `oc_mail_queue`.
#[derive(Clone)]
pub struct MailQueue {
    pool: Arc<DbPool>,
}

impl MailQueue {
    /// Construct a new queue handle against the given pool. Cloning is
    /// cheap (only an `Arc` is bumped).
    pub fn new(pool: Arc<DbPool>) -> Self {
        Self { pool }
    }

    /// Insert a row in `Pending` state, due immediately. Returns the
    /// generated row id.
    pub async fn enqueue(&self, env: &MailEnvelope) -> Result<i64, MailQueueError> {
        let now = Utc::now().naive_utc();
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
                .bind(now)
                .bind(now)
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
                .bind(now)
                .bind(now)
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
                .bind(now)
                .bind(now)
                .fetch_one(p)
                .await?;
                Ok(row.try_get::<i64, _>("id")?)
            }
        }
    }

    /// Claim up to `limit` rows that are ready to send. Atomically
    /// flips `Pending → Sending` and stamps `claimed_at`. On sqlite the
    /// claim is a select-then-update inside an implicit transaction; on
    /// MySQL/Postgres it uses `FOR UPDATE SKIP LOCKED` so concurrent
    /// workers don't fight over the same rows.
    pub async fn claim_batch(&self, limit: i64) -> Result<Vec<MailQueueRow>, MailQueueError> {
        let now = Utc::now().naive_utc();
        let mut out = Vec::new();
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                // sqlite has no `FOR UPDATE SKIP LOCKED`. Single-node
                // deployments accept the trivial race: we re-check the
                // state in the UPDATE's WHERE clause so a row only
                // transitions to `Sending` once.
                let rows = sqlx::query(
                    "SELECT id, recipient, subject, html_body, text_body, event_type, attempts \
                     FROM oc_mail_queue \
                     WHERE state = 'Pending' AND next_attempt_at <= ? \
                     ORDER BY id LIMIT ?",
                )
                .bind(now)
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
                    .bind(now)
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
            DbPool::MySql(p) => {
                let mut tx = p.begin().await?;
                let rows = sqlx::query(
                    "SELECT id, recipient, subject, html_body, text_body, event_type, attempts \
                     FROM oc_mail_queue \
                     WHERE state = 'Pending' AND next_attempt_at <= ? \
                     ORDER BY id LIMIT ? FOR UPDATE SKIP LOCKED",
                )
                .bind(now)
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
                        "UPDATE oc_mail_queue SET state = 'Sending', claimed_at = ? WHERE id = ?",
                    )
                    .bind(now)
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
                .bind(now)
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
                    .bind(now)
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

    /// Mark a previously-claimed row as successfully sent.
    pub async fn mark_sent(&self, id: i64) -> Result<(), MailQueueError> {
        let now = Utc::now().naive_utc();
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                sqlx::query("UPDATE oc_mail_queue SET state='Sent', sent_at=? WHERE id=?")
                    .bind(now)
                    .bind(id)
                    .execute(p)
                    .await?;
            }
            DbPool::MySql(p) => {
                sqlx::query("UPDATE oc_mail_queue SET state='Sent', sent_at=? WHERE id=?")
                    .bind(now)
                    .bind(id)
                    .execute(p)
                    .await?;
            }
            DbPool::Postgres(p) => {
                sqlx::query("UPDATE oc_mail_queue SET state='Sent', sent_at=$1 WHERE id=$2")
                    .bind(now)
                    .bind(id)
                    .execute(p)
                    .await?;
            }
        }
        Ok(())
    }

    /// Mark a row as failed but retryable. Increments `attempts` and
    /// pushes `next_attempt_at` forward by the next backoff step. The
    /// caller passes the *current* attempt count (i.e. the value of
    /// `MailQueueRow::attempts` before this attempt).
    pub async fn mark_failed_retry(
        &self,
        id: i64,
        err: &str,
        attempts: i32,
    ) -> Result<(), MailQueueError> {
        let idx = (attempts as usize).min(BACKOFF_SECS.len() - 1);
        let backoff = Duration::from_secs(BACKOFF_SECS[idx] as u64);
        let next_attempt = Utc::now()
            + chrono::Duration::from_std(backoff).expect("backoff fits in chrono::Duration");
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                sqlx::query(
                    "UPDATE oc_mail_queue SET state='Pending', attempts=attempts+1, \
                     next_attempt_at=?, last_error=? WHERE id=?",
                )
                .bind(next_attempt.naive_utc())
                .bind(err)
                .bind(id)
                .execute(p)
                .await?;
            }
            DbPool::MySql(p) => {
                sqlx::query(
                    "UPDATE oc_mail_queue SET state='Pending', attempts=attempts+1, \
                     next_attempt_at=?, last_error=? WHERE id=?",
                )
                .bind(next_attempt.naive_utc())
                .bind(err)
                .bind(id)
                .execute(p)
                .await?;
            }
            DbPool::Postgres(p) => {
                sqlx::query(
                    "UPDATE oc_mail_queue SET state='Pending', attempts=attempts+1, \
                     next_attempt_at=$1, last_error=$2 WHERE id=$3",
                )
                .bind(next_attempt.naive_utc())
                .bind(err)
                .bind(id)
                .execute(p)
                .await?;
            }
        }
        Ok(())
    }

    /// Mark a row as permanently failed (no further retries). Sets
    /// `state='Failed'` and records the last error.
    pub async fn mark_failed_permanent(&self, id: i64, err: &str) -> Result<(), MailQueueError> {
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                sqlx::query(
                    "UPDATE oc_mail_queue SET state='Failed', attempts=attempts+1, \
                     last_error=? WHERE id=?",
                )
                .bind(err)
                .bind(id)
                .execute(p)
                .await?;
            }
            DbPool::MySql(p) => {
                sqlx::query(
                    "UPDATE oc_mail_queue SET state='Failed', attempts=attempts+1, \
                     last_error=? WHERE id=?",
                )
                .bind(err)
                .bind(id)
                .execute(p)
                .await?;
            }
            DbPool::Postgres(p) => {
                sqlx::query(
                    "UPDATE oc_mail_queue SET state='Failed', attempts=attempts+1, \
                     last_error=$1 WHERE id=$2",
                )
                .bind(err)
                .bind(id)
                .execute(p)
                .await?;
            }
        }
        Ok(())
    }

    /// Reclaim rows stuck in `Sending` for more than [`STUCK_SENDING_AFTER_SECS`]
    /// seconds — a worker crash between `claim_batch` and `mark_*`
    /// leaves them orphaned otherwise. Returns the number of rows
    /// reset to `Pending`. Run periodically by [`crate::MailWorker`].
    pub async fn reclaim_stuck(&self) -> Result<u64, MailQueueError> {
        let cutoff = (Utc::now() - chrono::Duration::seconds(STUCK_SENDING_AFTER_SECS)).naive_utc();
        let n = match self.pool.as_ref() {
            DbPool::Sqlite(p) => sqlx::query(
                "UPDATE oc_mail_queue SET state='Pending', claimed_at=NULL \
                 WHERE state='Sending' AND claimed_at < ?",
            )
            .bind(cutoff)
            .execute(p)
            .await?
            .rows_affected(),
            DbPool::MySql(p) => sqlx::query(
                "UPDATE oc_mail_queue SET state='Pending', claimed_at=NULL \
                 WHERE state='Sending' AND claimed_at < ?",
            )
            .bind(cutoff)
            .execute(p)
            .await?
            .rows_affected(),
            DbPool::Postgres(p) => sqlx::query(
                "UPDATE oc_mail_queue SET state='Pending', claimed_at=NULL \
                 WHERE state='Sending' AND claimed_at < $1",
            )
            .bind(cutoff)
            .execute(p)
            .await?
            .rows_affected(),
        };
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Multidialect integration tests live in
    // crates/crabcloud-core/tests/mail_queue_e2e.rs. Inline unit tests
    // here cover backoff math only.

    #[test]
    fn backoff_indices_for_attempts() {
        assert_eq!(BACKOFF_SECS[0], 60);
        assert_eq!(BACKOFF_SECS[1], 300);
        assert_eq!(BACKOFF_SECS[2], 1800);
    }
}
