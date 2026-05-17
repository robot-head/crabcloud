//! Background task: daily walk of the preview cache root, deleting
//! rendered JPEGs whose `mtime` is older than `preview_retention_days`.
//!
//! The cache layout is `<root>/<storage_id>/<fileid>/<size>-<etag>.jpg`
//! (see `crabcloud-preview`). After deleting stale files we
//! opportunistically `remove_dir` on now-empty `<fileid>` and
//! `<storage_id>` directories — non-empty directories just return an
//! error, which we swallow. Depth is bounded (three levels), so we
//! recurse manually rather than depending on `walkdir`.
//!
//! Like [`crate::ExpirationWarningSweeper`], the long-running [`run`]
//! loop sleeps on a `tokio::sync::Notify` so test teardown (and
//! eventual graceful-shutdown wiring) can cancel cooperatively, and
//! [`cleanup_once`] is exposed `pub` so tests can drive a pass
//! synchronously without waiting for the daily timer.
//!
//! [`run`]: PreviewCacheCleanup::run
//! [`cleanup_once`]: PreviewCacheCleanup::cleanup_once

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::Notify;

/// How long to sleep between sweeps in `run()`.
const CLEANUP_INTERVAL: Duration = Duration::from_secs(24 * 3600);

/// Periodic deleter for stale entries under the preview cache root.
#[derive(Clone)]
pub struct PreviewCacheCleanup {
    root: PathBuf,
    retention: chrono::Duration,
    shutdown: Arc<Notify>,
}

impl PreviewCacheCleanup {
    /// Construct a cleanup task + paired shutdown handle. `notify_one()`
    /// on the returned `Arc<Notify>` cancels the `run()` loop after the
    /// current pass completes.
    pub fn new(root: PathBuf, retention_days: u32) -> (Self, Arc<Notify>) {
        let shutdown = Arc::new(Notify::new());
        (
            Self {
                root,
                retention: chrono::Duration::days(retention_days as i64),
                shutdown: shutdown.clone(),
            },
            shutdown,
        )
    }

    /// Long-running loop: cleanup, sleep, repeat. Cancels cooperatively
    /// when the paired shutdown `Notify` is notified.
    pub async fn run(self) {
        loop {
            if let Err(e) = self.cleanup_once().await {
                tracing::warn!(error = %e, "preview_cache_cleanup.cleanup_once failed");
            }
            tokio::select! {
                _ = tokio::time::sleep(CLEANUP_INTERVAL) => {}
                _ = self.shutdown.notified() => return,
            }
        }
    }

    /// Drive a single cleanup pass. Returns the number of files
    /// deleted. Exposed `pub` so tests can invoke it directly without
    /// waiting for the daily timer.
    ///
    /// Walks `<root>/<storage_id>/<fileid>/*.jpg`. After deleting stale
    /// files in a `<fileid>` directory, attempts `remove_dir` on the
    /// `<fileid>` directory (and similarly on `<storage_id>` after
    /// processing all its children). Errors from those opportunistic
    /// removes are ignored — non-empty dirs simply stay.
    pub async fn cleanup_once(&self) -> std::io::Result<u64> {
        if !self.root.exists() {
            return Ok(0);
        }
        let cutoff =
            SystemTime::now() - Duration::from_secs(self.retention.num_seconds().max(0) as u64);
        let mut deleted = 0u64;

        let mut storages = match tokio::fs::read_dir(&self.root).await {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e),
        };
        while let Some(storage_entry) = storages.next_entry().await? {
            let storage_path = storage_entry.path();
            let storage_meta = match tokio::fs::metadata(&storage_path).await {
                Ok(m) => m,
                Err(_) => continue,
            };
            if !storage_meta.is_dir() {
                continue;
            }
            let mut fileids = match tokio::fs::read_dir(&storage_path).await {
                Ok(r) => r,
                Err(_) => continue,
            };
            while let Some(fileid_entry) = fileids.next_entry().await? {
                let fileid_path = fileid_entry.path();
                let fileid_meta = match tokio::fs::metadata(&fileid_path).await {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                if !fileid_meta.is_dir() {
                    continue;
                }
                deleted += sweep_fileid_dir(&fileid_path, cutoff).await?;
                // Opportunistic — fails silently if non-empty.
                let _ = tokio::fs::remove_dir(&fileid_path).await;
            }
            // Opportunistic — fails silently if non-empty.
            let _ = tokio::fs::remove_dir(&storage_path).await;
        }
        Ok(deleted)
    }
}

/// Delete files in `<fileid>` whose mtime is older than `cutoff`.
/// Returns the count deleted.
async fn sweep_fileid_dir(dir: &std::path::Path, cutoff: SystemTime) -> std::io::Result<u64> {
    let mut deleted = 0u64;
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        let meta = match tokio::fs::metadata(&path).await {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !meta.is_file() {
            continue;
        }
        let mtime = match meta.modified() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if mtime < cutoff {
            if let Err(e) = tokio::fs::remove_file(&path).await {
                tracing::warn!(
                    error = %e,
                    path = %path.display(),
                    "preview_cache_cleanup: remove_file failed"
                );
                continue;
            }
            deleted += 1;
        }
    }
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::time::{Duration as StdDuration, SystemTime};
    use tempfile::tempdir;

    #[tokio::test]
    async fn cleanup_once_deletes_files_older_than_retention() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        // Layout: <root>/<storage_id>/<fileid>/<size>-<etag>.jpg
        let storage = root.join("42");
        let fileid_old = storage.join("100");
        let fileid_fresh = storage.join("101");
        std::fs::create_dir_all(&fileid_old).unwrap();
        std::fs::create_dir_all(&fileid_fresh).unwrap();
        let old_file = fileid_old.join("256-abcd.jpg");
        let fresh_file = fileid_fresh.join("256-efgh.jpg");
        std::fs::write(&old_file, b"old").unwrap();
        std::fs::write(&fresh_file, b"fresh").unwrap();

        // Backdate the old file by ~90 days.
        let ninety_days_ago = SystemTime::now() - StdDuration::from_secs(90 * 24 * 3600);
        let f = File::options().write(true).open(&old_file).unwrap();
        f.set_modified(ninety_days_ago).unwrap();
        drop(f);

        let (cleanup, _shutdown) = PreviewCacheCleanup::new(root.clone(), 60);
        let deleted = cleanup.cleanup_once().await.unwrap();
        assert_eq!(deleted, 1, "exactly one stale file should be removed");
        assert!(!old_file.exists(), "old file should be gone");
        assert!(fresh_file.exists(), "fresh file should remain");
        // Empty fileid dir for the old file should have been pruned;
        // the fresh one and the storage dir must remain (non-empty).
        assert!(!fileid_old.exists(), "empty <fileid> dir should be pruned");
        assert!(fileid_fresh.exists(), "non-empty <fileid> dir stays");
        assert!(storage.exists(), "non-empty <storage_id> dir stays");
    }

    #[tokio::test]
    async fn cleanup_once_on_missing_root_returns_zero() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("nonexistent");
        let (cleanup, _shutdown) = PreviewCacheCleanup::new(root, 60);
        let deleted = cleanup.cleanup_once().await.unwrap();
        assert_eq!(deleted, 0);
    }
}
