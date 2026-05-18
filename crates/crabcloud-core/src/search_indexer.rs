//! Background task: subscribes to the SCANNER's post-apply broadcast
//! and maintains the `oc_search` per-user index.
//!
//! Spec: `docs/superpowers/specs/2026-05-17-search-design.md`.
//!
//! Per-event panic-survival via `tokio::spawn` + ignore-JoinError. A
//! corrupt event can't take down the indexer.
//!
//! Recipients are resolved at the moment of the event via
//! `Shares::share_fanout_contexts_for_fileid` (point-in-time) — the
//! owner is always upserted, and per-share recipient sets are layered
//! on top with the share-mount-translated path so the recipient's
//! search hit reads as e.g. `/from-alice/q1/r.docx` rather than the
//! owner's `/docs/q1/r.docx`.
//!
//! Limitations called out in the SP15 spec §2 / §6:
//!   - `Deleted` events don't carry a fileid; the indexer queries
//!     `oc_search` directly by `(storage_id, path)` to discover it.
//!   - Trash bytes are not pushed through `storage_sink` (`Trash`
//!     bypasses the event sink), so the per-file Deleted handler
//!     correctly removes the user-visible row when a file is
//!     soft-deleted to trash. A defensive `is_trash_path` heuristic
//!     also short-circuits any future event that happens to live
//!     under `files_trashbin/`.
//!
//! Event ordering: this indexer uses fire-and-forget `tokio::spawn`
//! per event, so a `Written` and `Deleted` on the same path in quick
//! succession may be processed out of order under contention. The race
//! window is short (sub-second under normal load) and the eventual
//! state converges via subsequent events. If strict ordering becomes
//! necessary, switch to a per-`(storage_id, path)` serialization
//! queue.

use crabcloud_filecache::{FileCache, Scanner};
use crabcloud_search::{translate_path, Search};
use crabcloud_sharing::Shares;
use crabcloud_storage::{FileKind, StorageEvent, StoragePath};
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
    scanner: Arc<Scanner>,
    rx: tokio::sync::broadcast::Receiver<StorageEvent>,
    shutdown: Arc<Notify>,
}

impl SearchIndexer {
    /// Construct an indexer + return its shutdown notify handle.
    /// Subscribes to the SCANNER's post-apply broadcast eagerly so the
    /// receiver is registered before the consumer task starts, avoiding
    /// a window where early events would be silently dropped. The
    /// scanner's post-apply ordering means `filecache.lookup` for the
    /// just-emitted event sees a populated row — no
    /// poll-with-backoff race remains.
    pub fn new(
        search: Arc<Search>,
        shares: Arc<Shares>,
        filecache: Arc<FileCache>,
        scanner: Arc<Scanner>,
    ) -> (Self, Arc<Notify>) {
        let shutdown = Arc::new(Notify::new());
        let rx = scanner.subscribe_applied();
        (
            Self {
                search,
                shares,
                filecache,
                scanner,
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
                        let scanner = self.scanner.clone();
                        // Per-event panic survival: spawn the handler
                        // as a fire-and-forget task. A panic in the
                        // spawned task is logged by the tokio runtime
                        // and does NOT propagate back to this loop. We
                        // intentionally don't `await` the handle so a
                        // slow event handler doesn't backpressure the
                        // broadcast channel into `Lagged`.
                        std::mem::drop(tokio::spawn(async move {
                            handle_event(&search, &shares, &filecache, &scanner, event).await;
                        }));
                    }
                    Err(RecvError::Lagged(n)) => {
                        warn!(dropped = n, "search indexer: lagged behind scanner; events dropped");
                    }
                    Err(RecvError::Closed) => {
                        info!("search indexer: scanner channel closed; exiting");
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
    scanner: &Scanner,
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
            if let Err(e) =
                upsert_for_event(search, shares, filecache, scanner, &storage_id, &path).await
            {
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
                handle_move(search, shares, filecache, scanner, &storage_id, &from, &to).await
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
            if let Err(e) =
                upsert_for_event(search, shares, filecache, scanner, &storage_id, &to).await
            {
                warn!(error = %e, storage_id, to = %to.as_str(), "search indexer: copy-upsert failed");
            }
        }
    }
}

/// Resolve the filecache row for `(storage_id, path)` and UPSERT one
/// search row per current viewer: the owner (path = owner's view) plus
/// each per-share recipient (path = share-mount-translated view).
async fn upsert_for_event(
    search: &Search,
    shares: &Shares,
    filecache: &FileCache,
    scanner: &Scanner,
    storage_id: &str,
    path: &StoragePath,
) -> Result<(), crabcloud_search::SearchError> {
    // The scanner publishes post-apply, so the filecache row for this
    // event is already written by the time we run. A single lookup is
    // sufficient — no poll-with-backoff needed.
    let row = filecache.lookup(storage_id, path).await?;
    let Some(row) = row else {
        // Apply failed (scanner logs + schedules a re-scan) or the row
        // was deleted concurrently. Either way, skip; the re-scan path
        // will repopulate and the next touch on this file will re-index.
        debug!(
            storage_id,
            path = %path.as_str(),
            "search indexer: filecache miss after scanner-applied; skipping"
        );
        return Ok(());
    };
    if matches!(row.kind, FileKind::Directory) {
        return Ok(());
    }

    let owner_uid = scanner
        .storage_for(storage_id)
        .and_then(|s| s.owner_uid().map(str::to_string));

    let contexts = match shares.share_fanout_contexts_for_fileid(row.fileid).await {
        Ok(c) => c,
        Err(e) => {
            warn!(
                error = %e,
                fileid = row.fileid,
                "search indexer: share_fanout_contexts_for_fileid failed; indexing owner only"
            );
            Vec::new()
        }
    };

    let owner_path = web_path(path);
    let owner_basename = std::path::Path::new(&owner_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&row.name)
        .to_string();

    // Always upsert the owner if we know who they are.
    if let Some(uid) = owner_uid.as_deref() {
        search
            .upsert_for_file(
                uid,
                row.fileid,
                &row.storage_id,
                &owner_basename,
                &owner_path,
                row.mimetype.as_str(),
                row.mtime as i64,
                row.size as i64,
            )
            .await?;
    } else if contexts.is_empty() {
        // No owner attribution AND no shares cover this file: skip
        // rather than fabricate a phantom row.
        debug!(
            storage_id,
            "search indexer: no owner attribution and no shares; skipping upsert"
        );
        return Ok(());
    }

    // Per-share recipient upserts with translated paths. If multiple
    // shares cover the same recipient, later contexts overwrite
    // earlier ones (documented in `share_fanout_contexts_for_fileid`).
    for ctx in &contexts {
        let viewer_path = translate_path(&ctx.owner_subroot, &ctx.recipient_prefix, &owner_path);
        let basename = std::path::Path::new(&viewer_path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&row.name)
            .to_string();
        for r in &ctx.recipients {
            // Skip the owner — already upserted above with their own
            // path (and the owner shouldn't appear in any recipient set
            // for their own share, but defend against group-membership
            // edge cases anyway).
            if Some(r.as_str()) == owner_uid.as_deref() {
                continue;
            }
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
/// filecache, which the scanner has already updated by the time we
/// observe the post-apply signal).
///
/// The DELETE-ALL-then-re-UPSERT shape implicitly handles the
/// out-of-share rename case: viewers who lost access (their share row
/// no longer covers the new path) get cleaned up by
/// `delete_for_file(to_row.fileid)` and are simply absent from the
/// fresh recipient set we re-upsert against.
async fn handle_move(
    search: &Search,
    shares: &Shares,
    filecache: &FileCache,
    scanner: &Scanner,
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
    // path + mtime + recipient set for the upsert. No backoff: scanner
    // post-apply ordering guarantees the row is present.
    let to_row = filecache.lookup(storage_id, to).await?;
    let Some(to_row) = to_row else {
        // Apply must have failed on the scanner side; just remove any
        // stale row by fileid so search doesn't keep serving the
        // pre-move path.
        if let Some(fid) = pre_move_fileid {
            debug!(
                storage_id,
                "search indexer: post-move filecache miss; removing stale row by fileid"
            );
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

    let owner_uid = scanner
        .storage_for(storage_id)
        .and_then(|s| s.owner_uid().map(str::to_string));

    let contexts = match shares.share_fanout_contexts_for_fileid(to_row.fileid).await {
        Ok(c) => c,
        Err(e) => {
            warn!(
                error = %e,
                fileid = to_row.fileid,
                "search indexer: share_fanout_contexts_for_fileid (post-move) failed; indexing owner only"
            );
            Vec::new()
        }
    };

    // Rebuild: delete EVERY viewer row for this fileid (cleans up
    // viewers who can no longer see it, including out-of-share
    // renames), then upsert one row per current recipient with the
    // post-move translated path.
    search.delete_for_file(to_row.fileid).await?;

    let owner_path = web_path(to);
    let owner_basename = std::path::Path::new(&owner_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&to_row.name)
        .to_string();
    if let Some(uid) = owner_uid.as_deref() {
        search
            .upsert_for_file(
                uid,
                to_row.fileid,
                &to_row.storage_id,
                &owner_basename,
                &owner_path,
                to_row.mimetype.as_str(),
                to_row.mtime as i64,
                to_row.size as i64,
            )
            .await?;
    }

    for ctx in &contexts {
        let viewer_path = translate_path(&ctx.owner_subroot, &ctx.recipient_prefix, &owner_path);
        let basename = std::path::Path::new(&viewer_path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&to_row.name)
            .to_string();
        for r in &ctx.recipients {
            if Some(r.as_str()) == owner_uid.as_deref() {
                continue;
            }
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
    }
    Ok(())
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
