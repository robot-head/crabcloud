//! Scanner: continuous consumer of `ChannelEventSink` events + on-demand
//! full-scan + drift recovery via `RecvError::Lagged`.

pub mod apply;
pub mod full_scan;

use crabcloud_storage::{ChannelEventSink, Storage};
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::error::FileCacheResult;
use crate::FileCache;

pub struct Scanner {
    cache: Arc<FileCache>,
    storages: DashMap<String, Arc<dyn Storage>>,
    sink: Arc<ChannelEventSink>,
}

impl Scanner {
    pub fn new(cache: Arc<FileCache>, sink: Arc<ChannelEventSink>) -> Self {
        Self {
            cache,
            storages: DashMap::new(),
            sink,
        }
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
                        if let Err(e) = self.cache.apply(&event).await {
                            warn!(?event, error = %e, "filecache apply failed; scheduling re-scan");
                            if let Some(storage) = self.storage_for(event.storage_id()) {
                                let _ = full_scan::full_scan(&self.cache, &storage).await;
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
