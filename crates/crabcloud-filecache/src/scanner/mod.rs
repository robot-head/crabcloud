//! Scanner: continuous consumer of `ChannelEventSink` events + on-demand
//! full-scan + drift recovery via `RecvError::Lagged`.
//!
//! Publishes a downstream "applied" broadcast (`subscribe_applied`) that
//! re-emits each `StorageEvent` only AFTER the corresponding
//! `cache.apply` succeeds. Consumers that need the filecache row for
//! the just-emitted event (the search indexer is the in-tree case)
//! should subscribe to that signal instead of the raw storage sink —
//! the apply happens-before relationship eliminates the prior
//! poll-with-backoff race.

pub mod apply;
pub mod full_scan;

use crabcloud_storage::{ChannelEventSink, Storage, StorageEvent};
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::error::FileCacheResult;
use crate::FileCache;

/// Default capacity for the post-apply broadcast channel. Sized
/// generously so a slow downstream (the search indexer) tolerates a
/// short burst before `RecvError::Lagged`; the small extra memory is
/// cheap relative to losing events.
const APPLIED_BROADCAST_CAPACITY: usize = 1024;

pub struct Scanner {
    cache: Arc<FileCache>,
    storages: DashMap<String, Arc<dyn Storage>>,
    sink: Arc<ChannelEventSink>,
    applied_tx: broadcast::Sender<StorageEvent>,
}

impl Scanner {
    pub fn new(cache: Arc<FileCache>, sink: Arc<ChannelEventSink>) -> Self {
        let (applied_tx, _) = broadcast::channel(APPLIED_BROADCAST_CAPACITY);
        Self {
            cache,
            storages: DashMap::new(),
            sink,
            applied_tx,
        }
    }

    /// Subscribe to the downstream "scanner-applied" signal: receives
    /// each `StorageEvent` only AFTER `FileCache::apply` for that event
    /// has succeeded. Downstreams that need the filecache row for the
    /// just-emitted event should use this instead of the raw storage
    /// sink to avoid the scanner-vs-consumer race. Events whose apply
    /// failed (logged + scheduled for re-scan) are NOT re-emitted; the
    /// consumer's view of those is the same as if the event never
    /// happened, and the re-scan will repopulate the cache out-of-band.
    pub fn subscribe_applied(&self) -> broadcast::Receiver<StorageEvent> {
        self.applied_tx.subscribe()
    }

    /// Register a storage so the scanner's drift-recovery + on-demand full
    /// scans can find it by `storage_id`.
    pub fn register_storage(&self, storage: Arc<dyn Storage>) {
        self.storages.insert(storage.id().to_string(), storage);
    }

    /// Look up a registered storage by `id`.
    pub fn storage_for(&self, id: &str) -> Option<Arc<dyn Storage>> {
        self.storages.get(id).map(|s| s.clone())
    }

    /// BFS-walk the given storage, populating every reachable cache row.
    /// Returns the number of paths visited (including root).
    pub async fn full_scan(&self, storage: &Arc<dyn Storage>) -> FileCacheResult<u64> {
        full_scan::full_scan(&self.cache, storage).await
    }

    /// Snapshot the currently-registered storages. Used by the consumer
    /// loop on `Lagged` recovery so we don't hold a `DashMap` iter guard
    /// across an `.await`.
    fn snapshot_storages(&self) -> Vec<Arc<dyn Storage>> {
        self.storages.iter().map(|e| e.value().clone()).collect()
    }

    /// Spawn the continuous consumer loop. Subscribes to the sink, applies
    /// each event to the cache, recovers from `RecvError::Lagged` by full-
    /// scanning every registered storage, and exits cleanly on `Closed`.
    pub fn spawn(self: Arc<Self>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut rx = self.sink.subscribe();
            info!("scanner consumer started");
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        // Retry on SQLITE_BUSY (deadlock-avoidance —
                        // sqlite3_busy_timeout intentionally does NOT
                        // wait when two connections are both promoting
                        // a deferred tx to write). Short exponential
                        // backoff, then fall back to a full_scan if
                        // apply keeps failing. The retry envelope is
                        // bounded so a genuinely broken apply still
                        // surfaces in a reasonable time.
                        let apply_result = apply_with_busy_retry(
                            &self.cache,
                            &event,
                        )
                        .await;
                        match apply_result {
                            Ok(()) => {
                                let _ = self.applied_tx.send(event);
                            }
                            Err(e) => {
                                warn!(?event, error = %e, "filecache apply failed; scheduling re-scan");
                                if let Some(storage) = self.storage_for(event.storage_id()) {
                                    let _ = full_scan::full_scan(&self.cache, &storage).await;
                                }
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(skipped = n, "scanner lagged; full-scanning all storages");
                        for storage in self.snapshot_storages() {
                            let _ = full_scan::full_scan(&self.cache, &storage).await;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        info!("scanner channel closed; consumer exiting");
                        return;
                    }
                }
            }
        })
    }
}

/// Apply an event with bounded retry on `SQLITE_BUSY`. sqlite's
/// `sqlite3_busy_timeout` does NOT wait when two connections both want
/// to promote a deferred transaction to a write (deadlock-avoidance —
/// returns SQLITE_BUSY immediately to one of them). With multiple
/// background writers (search indexer upserts, activity emits, trash
/// inserts) overlapping the scanner's apply, this case shows up
/// occasionally; a short retry loop converges on the next pool slice
/// without surfacing a transient error to the apply caller.
async fn apply_with_busy_retry(
    cache: &FileCache,
    event: &StorageEvent,
) -> FileCacheResult<()> {
    // ~1.5s upper bound — 50% margin over sqlx-sqlite's 5s
    // `busy_timeout` default would be excessive for what is almost
    // always a sub-50ms wait under real contention. Bound on attempts,
    // not wall time, keeps the budget predictable.
    let backoffs = [10u64, 20, 40, 80, 160, 320, 640];
    let mut last_err = None;
    for ms in backoffs {
        match cache.apply(event).await {
            Ok(()) => return Ok(()),
            Err(e) if is_sqlite_busy(&e) => {
                last_err = Some(e);
                tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_err.expect("at least one retry attempted before exhausting backoffs"))
}

/// True for transient sqlite write-contention errors that retrying
/// will likely resolve. Code 5 is `SQLITE_BUSY`; code 517 is the
/// WAL-mode `SQLITE_BUSY_SNAPSHOT` variant (kept here so the same
/// retry helper covers operators who flip on WAL).
fn is_sqlite_busy(e: &crate::error::FileCacheError) -> bool {
    if let crate::error::FileCacheError::Db(sqlx::Error::Database(dbe)) = e {
        if let Some(code) = dbe.code() {
            return matches!(code.as_ref(), "5" | "517");
        }
    }
    false
}
