//! Background task: subscribes to `storage_sink` events and maintains
//! the `oc_search` per-user index.
//!
//! Spec: `docs/superpowers/specs/2026-05-17-search-design.md`.
//!
//! Per-event panic-survival via `tokio::spawn` + ignore-JoinError. A
//! corrupt event can't take down the indexer.
//!
//! Recipients are resolved at the moment of the event via
//! `Shares::recipients_for_fileid` (point-in-time) — the owner is
//! always upserted, and the share-graph recipients are layered on top.
//!
//! Limitations called out in the SP15 spec §2 / §6:
//!   - Path stored for recipient rows is the OWNER's path, not the
//!     viewer's share-mount-translated path. The bulk
//!     `Search::fan_out_for_share` path translates correctly. This
//!     indexer's per-write path is the documented MVP compromise; UI
//!     handles either.
//!   - `Deleted` events don't carry a fileid; the indexer queries
//!     `oc_search` directly by `(storage_id, path)` to discover it.
//!   - Trash bytes are not pushed through `storage_sink` (`Trash`
//!     bypasses the event sink), so the per-file Deleted handler
//!     correctly removes the user-visible row when a file is
//!     soft-deleted to trash. A defensive `is_trash_path` heuristic
//!     also short-circuits any future event that happens to live
//!     under `files_trashbin/`.

use crabcloud_filecache::FileCache;
use crabcloud_search::Search;
use crabcloud_sharing::Shares;
use crabcloud_storage::{ChannelEventSink, FileKind, StorageEvent, StoragePath};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::Notify;
use tracing::{debug, info, warn};

/// Long-running subscriber that funnels `StorageEvent`s into the
/// `oc_search` index.
pub struct SearchIndexer {
    search: Arc<Search>,
    shares: Arc<Shares>,
    filecache: Arc<FileCache>,
    rx: tokio::sync::broadcast::Receiver<StorageEvent>,
    shutdown: Arc<Notify>,
}

impl SearchIndexer {
    /// Construct an indexer + return its shutdown notify handle.
    /// `storage_sink.subscribe()` is called eagerly so the indexer's
    /// receiver is registered before the consumer task starts, avoiding
    /// a window where early events would be silently dropped.
    pub fn new(
        search: Arc<Search>,
        shares: Arc<Shares>,
        filecache: Arc<FileCache>,
        storage_sink: &ChannelEventSink,
    ) -> (Self, Arc<Notify>) {
        let shutdown = Arc::new(Notify::new());
        let rx = storage_sink.subscribe();
        (
            Self {
                search,
                shares,
                filecache,
                rx,
                shutdown: shutdown.clone(),
            },
            shutdown,
        )
    }

    /// Consumer loop. Exits on `shutdown.notify_one()` or when the
    /// underlying broadcast channel closes.
    pub async fn run(mut self) {
        info!("search indexer started");
        loop {
            tokio::select! {
                _ = self.shutdown.notified() => {
                    info!("search indexer received shutdown");
                    return;
                }
                ev = self.rx.recv() => match ev {
                    Ok(event) => {
                        let search = self.search.clone();
                        let shares = self.shares.clone();
                        let filecache = self.filecache.clone();
                        let event_for_log = format!("{event:?}");
                        // Per-event panic survival: drive the handler
                        // through a `spawn`. A panic surfaces as a
                        // `JoinError` we log + continue past.
                        let handle = tokio::spawn(async move {
                            handle_event(&search, &shares, &filecache, event).await;
                        });
                        if let Err(e) = handle.await {
                            warn!(error = %e, event = %event_for_log, "search indexer: handler panicked");
                        }
                    }
                    Err(RecvError::Lagged(n)) => {
                        warn!(dropped = n, "search indexer: lagged behind storage_sink; events dropped");
                    }
                    Err(RecvError::Closed) => {
                        info!("search indexer: storage_sink closed; exiting");
                        return;
                    }
                },
            }
        }
    }
}

async fn handle_event(
    search: &Search,
    shares: &Shares,
    filecache: &FileCache,
    event: StorageEvent,
) {
    if is_trash_event(&event) {
        return;
    }
    match event {
        StorageEvent::Written {
            storage_id,
            path,
            metadata: _,
        } => {
            if let Err(e) = upsert_for_event(search, shares, filecache, &storage_id, &path).await {
                warn!(error = %e, storage_id, path = %path.as_str(), "search indexer: upsert failed");
            }
        }
        StorageEvent::DirCreated { .. } => {
            // Directories are not searchable in MVP (spec §1).
        }
        StorageEvent::Deleted { storage_id, path } => {
            if let Err(e) = delete_for_event(search, &storage_id, &path).await {
                warn!(error = %e, storage_id, path = %path.as_str(), "search indexer: delete failed");
            }
        }
        StorageEvent::Moved {
            storage_id,
            from,
            to,
        } => {
            if let Err(e) =
                handle_move(search, shares, filecache, &storage_id, &from, &to).await
            {
                warn!(error = %e, storage_id, from = %from.as_str(), to = %to.as_str(), "search indexer: move failed");
            }
        }
        StorageEvent::Copied {
            storage_id,
            from: _,
            to,
        } => {
            // A copy creates a fresh fileid at `to`. Upsert that one.
            if let Err(e) = upsert_for_event(search, shares, filecache, &storage_id, &to).await {
                warn!(error = %e, storage_id, to = %to.as_str(), "search indexer: copy-upsert failed");
            }
        }
    }
}

/// Resolve the filecache row for `(storage_id, path)` and UPSERT one
/// search row per current recipient (owner + share recipients).
async fn upsert_for_event(
    search: &Search,
    shares: &Shares,
    filecache: &FileCache,
    storage_id: &str,
    path: &StoragePath,
) -> Result<(), crabcloud_search::SearchError> {
    // Wait briefly for the filecache scanner to have applied this
    // event. Both consumers race on the same broadcast channel; in
    // practice the scanner wins quickly, but we retry on miss to be
    // robust against scheduling jitter.
    let row = loop_lookup_with_backoff(filecache, storage_id, path).await?;
    let Some(row) = row else {
        debug!(storage_id, path = %path.as_str(), "search indexer: filecache miss after retry; skipping");
        return Ok(());
    };
    if matches!(row.kind, FileKind::Directory) {
        return Ok(());
    }

    let owner_uid = owner_uid_from_storage_id(storage_id);
    let mut recipients = match shares.recipients_for_fileid(row.fileid).await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, fileid = row.fileid, "search indexer: recipients_for_fileid failed; indexing owner only");
            Vec::new()
        }
    };
    // Make sure the owner is always included.
    if let Some(uid) = owner_uid.as_deref() {
        if !recipients.iter().any(|r| r.as_str() == uid) {
            if let Ok(uid) = crabcloud_users::UserId::new(uid) {
                recipients.push(uid);
            }
        }
    }

    if recipients.is_empty() {
        // We don't know who owns this file (storage-id heuristic failed
        // AND no shares exist). Skip rather than write a phantom row.
        debug!(storage_id, "search indexer: no recipients resolved; skipping upsert");
        return Ok(());
    }

    let viewer_path = web_path(path);
    let basename = std::path::Path::new(&viewer_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&row.name)
        .to_string();
    for r in recipients {
        search
            .upsert_for_file(
                r.as_str(),
                row.fileid,
                &row.storage_id,
                &basename,
                &viewer_path,
                row.mimetype.as_str(),
                row.mtime as i64,
                row.size as i64,
            )
            .await?;
    }
    Ok(())
}

/// Discover the fileid for a deleted `(storage_id, path)` via the
/// search index itself, then DELETE every viewer row for that fileid.
async fn delete_for_event(
    search: &Search,
    storage_id: &str,
    path: &StoragePath,
) -> Result<(), crabcloud_search::SearchError> {
    let viewer_path = web_path(path);
    let Some(fileid) = search
        .fileid_for_storage_path(storage_id, &viewer_path)
        .await?
    else {
        // Nothing indexed for that path — nothing to remove. Common when
        // the file was never written through the indexer (e.g. created
        // before SP15 shipped) or a directory was deleted.
        return Ok(());
    };
    search.delete_for_file(fileid).await
}

/// Handle a Move (rename / move-within-storage). The fileid is stable
/// across a move, so we can resolve it from EITHER the old path (via
/// the index, if it was indexed pre-move) or the new path (via
/// filecache, which the scanner updates).
async fn handle_move(
    search: &Search,
    shares: &Shares,
    filecache: &FileCache,
    storage_id: &str,
    from: &StoragePath,
    to: &StoragePath,
) -> Result<(), crabcloud_search::SearchError> {
    let from_viewer = web_path(from);
    let pre_move_fileid = search
        .fileid_for_storage_path(storage_id, &from_viewer)
        .await
        .ok()
        .flatten();
    // Look up via filecache at the destination — this gives us the new
    // path + mtime + recipient set for the upsert. Same scanner-race
    // retry as `upsert_for_event`.
    let to_row = loop_lookup_with_backoff(filecache, storage_id, to).await?;
    let Some(to_row) = to_row else {
        // Race with scanner; just remove any stale row by fileid.
        if let Some(fid) = pre_move_fileid {
            debug!(storage_id, "search indexer: post-move filecache miss; removing stale row by fileid");
            search.delete_for_file(fid).await?;
        }
        return Ok(());
    };
    if matches!(to_row.kind, FileKind::Directory) {
        // Directories themselves aren't indexed; their children will
        // get separate Moved events from the storage layer (or get
        // re-indexed when next touched). MVP scope.
        return Ok(());
    }

    // Recompute the recipient set against the NEW path (it may have
    // crossed a shared-subroot boundary).
    let owner_uid = owner_uid_from_storage_id(storage_id);
    let mut new_recipients = match shares.recipients_for_fileid(to_row.fileid).await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, fileid = to_row.fileid, "search indexer: recipients_for_fileid (post-move) failed; indexing owner only");
            Vec::new()
        }
    };
    if let Some(uid) = owner_uid.as_deref() {
        if !new_recipients.iter().any(|r| r.as_str() == uid) {
            if let Ok(uid) = crabcloud_users::UserId::new(uid) {
                new_recipients.push(uid);
            }
        }
    }
    let new_recipient_set: HashSet<String> = new_recipients
        .iter()
        .map(|u| u.as_str().to_string())
        .collect();

    // Rebuild: delete EVERY viewer row for this fileid (cleans up
    // viewers who can no longer see it), then upsert one row per current
    // recipient.
    search.delete_for_file(to_row.fileid).await?;
    if new_recipients.is_empty() {
        return Ok(());
    }
    let viewer_path = web_path(to);
    let basename = std::path::Path::new(&viewer_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&to_row.name)
        .to_string();
    for r in &new_recipients {
        search
            .upsert_for_file(
                r.as_str(),
                to_row.fileid,
                &to_row.storage_id,
                &basename,
                &viewer_path,
                to_row.mimetype.as_str(),
                to_row.mtime as i64,
                to_row.size as i64,
            )
            .await?;
    }
    // Touch the set just to suppress the unused warning — we may use
    // it for finer-grained delta-application in a future iteration.
    let _ = new_recipient_set;
    Ok(())
}

/// Retry `filecache.lookup` up to ~500ms total to absorb the
/// indexer-vs-scanner race on a single broadcast event. Both consumers
/// receive the same event independently; without retry the indexer
/// often loses the race when the scanner is busy on a different write.
async fn loop_lookup_with_backoff(
    filecache: &FileCache,
    storage_id: &str,
    path: &StoragePath,
) -> Result<Option<crabcloud_filecache::FilecacheRow>, crabcloud_search::SearchError> {
    let backoffs = [10u64, 20, 40, 80, 150, 200];
    for (i, ms) in backoffs.iter().enumerate() {
        match filecache.lookup(storage_id, path).await {
            Ok(Some(r)) => return Ok(Some(r)),
            Ok(None) => {
                if i + 1 == backoffs.len() {
                    return Ok(None);
                }
                tokio::time::sleep(std::time::Duration::from_millis(*ms)).await;
            }
            Err(e) => return Err(crabcloud_search::SearchError::from(e)),
        }
    }
    Ok(None)
}

/// Return the leading-`/` form of a StoragePath, suitable for the
/// `oc_search.path` column convention (matches what `Shares::create`
/// produces via `format!("/{file_target}")`).
fn web_path(p: &StoragePath) -> String {
    let s = p.as_str();
    if s.is_empty() {
        "/".to_string()
    } else if s.starts_with('/') {
        s.to_string()
    } else {
        format!("/{s}")
    }
}

/// Heuristic: parse the owner uid out of a `LocalStorage`-shaped
/// `storage_id`. Format is `local::<canonical_path>` where the path
/// ends with `<uid>/files`. We strip the `local::` scheme, normalize
/// path separators, and return the segment before the final `files`.
///
/// Returns `None` for any storage_id we don't recognize — the caller
/// falls back to skipping owner-injection rather than fabricating a
/// uid. A non-local storage backend (S3, etc.) would surface as
/// `None` here and would need its own owner-resolution path.
fn owner_uid_from_storage_id(storage_id: &str) -> Option<String> {
    let stripped = storage_id.strip_prefix("local::")?;
    // Normalize both unix and windows-style separators.
    let normalized = stripped.replace('\\', "/");
    let mut segments: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
    let last = segments.pop()?;
    if last != "files" {
        return None;
    }
    let uid = segments.pop()?;
    Some(uid.to_string())
}

/// Defensive check: any future event whose path lives under
/// `files_trashbin/` should be ignored. `Trash::soft_delete` doesn't
/// use the event sink, so today this is unreachable — but it costs
/// nothing to keep the spec §6 invariant explicit.
fn is_trash_event(event: &StorageEvent) -> bool {
    let path = match event {
        StorageEvent::Written { path, .. }
        | StorageEvent::DirCreated { path, .. }
        | StorageEvent::Deleted { path, .. } => path.as_str(),
        StorageEvent::Moved { to, .. } | StorageEvent::Copied { to, .. } => to.as_str(),
    };
    path.starts_with("files_trashbin/") || path == "files_trashbin"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_uid_extracts_from_local_storage_id_unix() {
        assert_eq!(
            owner_uid_from_storage_id("local::/var/lib/cc/alice/files"),
            Some("alice".to_string())
        );
    }

    #[test]
    fn owner_uid_extracts_from_local_storage_id_windows() {
        assert_eq!(
            owner_uid_from_storage_id(r"local::C:\Users\test\data\alice\files"),
            Some("alice".to_string())
        );
    }

    #[test]
    fn owner_uid_returns_none_for_non_local() {
        assert_eq!(owner_uid_from_storage_id("s3::bucket/prefix"), None);
        assert_eq!(owner_uid_from_storage_id("home::alice"), None);
    }

    #[test]
    fn owner_uid_returns_none_for_unexpected_suffix() {
        assert_eq!(owner_uid_from_storage_id("local::/var/lib/cc/alice"), None);
    }

    #[test]
    fn web_path_adds_leading_slash() {
        let p = StoragePath::new("docs/r.txt").unwrap();
        assert_eq!(web_path(&p), "/docs/r.txt");
    }

    #[test]
    fn web_path_handles_root() {
        assert_eq!(web_path(&StoragePath::root()), "/");
    }

    #[test]
    fn is_trash_event_detects_trash_path() {
        let p = StoragePath::new("files_trashbin/files/x.txt").unwrap();
        let ev = StorageEvent::Deleted {
            storage_id: "x".into(),
            path: p,
        };
        assert!(is_trash_event(&ev));
    }

    #[test]
    fn is_trash_event_ignores_normal_path() {
        let p = StoragePath::new("docs/x.txt").unwrap();
        let ev = StorageEvent::Deleted {
            storage_id: "x".into(),
            path: p,
        };
        assert!(!is_trash_event(&ev));
    }
}
