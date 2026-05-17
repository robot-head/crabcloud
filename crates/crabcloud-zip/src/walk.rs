//! Pre-flight walk that builds a [`ZipPlan`] and rejects oversize folders.
//!
//! Walks depth-first via [`crabcloud_fs::View::list`] / `stat`. Aborts on
//! first overflow with [`WalkError::TooLarge`].

use crate::error::WalkError;
use crate::types::{PlanKind, PlannedEntry, ZipCaps, ZipPlan};
use crabcloud_fs::path::UserPath;
use crabcloud_fs::View;
use crabcloud_storage::FileKind;

/// Walk `root` (which must be a directory) and return a [`ZipPlan`] of
/// every entry to include, or `WalkError::TooLarge` if either cap is hit.
///
/// `root` is the user-facing path (e.g. `/Photos`). The returned
/// `PlannedEntry.zip_name` starts at the basename of `root` so the
/// archive's internal structure begins `<basename(root)>/...`.
pub async fn walk_for_caps(
    view: &View,
    root: &UserPath,
    caps: &ZipCaps,
) -> Result<ZipPlan, WalkError> {
    let basename = root_basename(root);
    let mut entries: Vec<PlannedEntry> = Vec::new();
    let mut total_bytes: u64 = 0;

    // DFS via an explicit stack of (user_path, zip_prefix). Recording the
    // current directory itself before descending preserves empty folders.
    let mut stack: Vec<(UserPath, String)> = Vec::new();
    stack.push((root.clone(), basename));

    while let Some((current_user_path, zip_prefix)) = stack.pop() {
        // Record the directory itself so empty folders survive the zip.
        if !zip_prefix.is_empty() {
            let dir_meta = view.stat(&current_user_path).await?;
            push_entry(
                &mut entries,
                &mut total_bytes,
                caps,
                PlannedEntry {
                    storage_path: storage_path_of(view, &current_user_path)?,
                    zip_name: zip_prefix.clone(),
                    kind: PlanKind::Dir,
                    size: 0,
                    mtime: dir_meta.mtime,
                    mime: String::new(),
                },
            )?;
        }
        let dir_entries = view.list(&current_user_path).await?;
        for de in dir_entries {
            let child_user_path = join_user_path(&current_user_path, &de.name)?;
            let child_prefix = if zip_prefix.is_empty() {
                de.name.clone()
            } else {
                format!("{zip_prefix}/{}", de.name)
            };
            match de.metadata.kind {
                FileKind::Directory => {
                    stack.push((child_user_path, child_prefix));
                }
                FileKind::File => {
                    push_entry(
                        &mut entries,
                        &mut total_bytes,
                        caps,
                        PlannedEntry {
                            storage_path: storage_path_of(view, &child_user_path)?,
                            zip_name: child_prefix,
                            kind: PlanKind::File,
                            size: de.metadata.size,
                            mtime: de.metadata.mtime,
                            mime: de.metadata.mimetype.as_str().to_string(),
                        },
                    )?;
                }
            }
        }
    }

    Ok(ZipPlan {
        entries,
        total_bytes,
    })
}

/// Basename of a user-facing path, stripped of leading/trailing slashes.
///
/// Returns the empty string for the home root (`/`). Public so HTTP
/// handlers can derive `Content-Disposition` filenames without duplicating
/// the trimming logic.
pub fn root_basename(root: &UserPath) -> String {
    let stripped = root.as_str().trim_start_matches('/').trim_end_matches('/');
    if stripped.is_empty() {
        return String::new();
    }
    match stripped.rsplit_once('/') {
        Some((_, last)) => last.to_string(),
        None => stripped.to_string(),
    }
}

fn join_user_path(parent: &UserPath, child: &str) -> Result<UserPath, WalkError> {
    let p = parent.as_str().trim_end_matches('/');
    let candidate = if p == "/" || p.is_empty() {
        format!("/{child}")
    } else {
        format!("{p}/{child}")
    };
    UserPath::new(candidate).map_err(WalkError::View)
}

fn storage_path_of(
    view: &View,
    user_path: &UserPath,
) -> Result<crabcloud_storage::StoragePath, WalkError> {
    let (_, sp) = view.cache_key_for(user_path).map_err(WalkError::View)?;
    Ok(sp)
}

fn push_entry(
    entries: &mut Vec<PlannedEntry>,
    total_bytes: &mut u64,
    caps: &ZipCaps,
    entry: PlannedEntry,
) -> Result<(), WalkError> {
    *total_bytes = total_bytes.saturating_add(entry.size);
    let new_count = entries.len() as u64 + 1;
    if new_count > caps.max_entries || *total_bytes > caps.max_bytes {
        return Err(WalkError::TooLarge {
            count: new_count,
            bytes: *total_bytes,
        });
    }
    entries.push(entry);
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crabcloud_config::test_support::minimal_sqlite_config;
    use crabcloud_db::{core_set, DbPool, MigrationRunner};
    use crabcloud_filecache::FileCache;
    use crabcloud_fs::{Mount, View};
    use crabcloud_storage::{
        memory::MemoryStorage, ChannelEventSink, NoopEventSink, Storage, StoragePath,
    };
    use crabcloud_users::UserId;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Builds a View with a single home mount at `/`, an in-memory storage,
    /// and a FileCache backed by a fresh in-memory SQLite. Seeded with a
    /// fixed Photos/ tree.
    ///
    /// Returns `(view, tempdir)` — the tempdir owns the sqlite file and
    /// must outlive the view's filecache pool. Tests should hold the
    /// tempdir for the duration of the test body.
    pub(crate) async fn seed_view() -> (View, TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let cfg = minimal_sqlite_config(dir.path().join("zip-walk.db"));
        let pool = DbPool::connect(&cfg).await.unwrap();
        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();
        let filecache = Arc::new(FileCache::new(pool.clone()));
        let sink = Arc::new(ChannelEventSink::new(64));
        let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new("alice"));
        let pool_arc = Arc::new(pool);
        let versions = Arc::new(crabcloud_versions::Versions::new(
            pool_arc.clone(),
            dir.path().to_path_buf(),
            std::sync::Arc::new(crabcloud_activity::NoopEmitter),
        ));
        let trash = Arc::new(crabcloud_trash::Trash::new(
            pool_arc,
            dir.path().to_path_buf(),
            versions.clone(),
            std::sync::Arc::new(crabcloud_activity::NoopEmitter),
        ));

        // Seed:
        //   /Photos/
        //     cat.jpg     (8 bytes)
        //     dog.jpg     (8 bytes)
        //     vacation/
        //       beach.jpg (16 bytes)
        //       empty/
        let s = &NoopEventSink;
        storage
            .mkdir(&StoragePath::new("Photos").unwrap(), s)
            .await
            .unwrap();
        storage
            .mkdir(&StoragePath::new("Photos/vacation").unwrap(), s)
            .await
            .unwrap();
        storage
            .mkdir(&StoragePath::new("Photos/vacation/empty").unwrap(), s)
            .await
            .unwrap();
        storage
            .put_file(
                &StoragePath::new("Photos/cat.jpg").unwrap(),
                Box::pin(std::io::Cursor::new(b"cat-data".to_vec())),
                s,
            )
            .await
            .unwrap();
        storage
            .put_file(
                &StoragePath::new("Photos/dog.jpg").unwrap(),
                Box::pin(std::io::Cursor::new(b"dog-data".to_vec())),
                s,
            )
            .await
            .unwrap();
        storage
            .put_file(
                &StoragePath::new("Photos/vacation/beach.jpg").unwrap(),
                Box::pin(std::io::Cursor::new(b"beach-data-16byt".to_vec())),
                s,
            )
            .await
            .unwrap();

        let view = View::new(
            UserId::new("alice").unwrap(),
            vec![Mount {
                path_prefix: StoragePath::root(),
                storage,
                metadata: None,
            }],
            filecache,
            sink,
            trash,
            crabcloud_fs::VersionsHooks::permissive(versions),
        );
        (view, dir)
    }

    #[tokio::test]
    async fn walk_counts_entries_recursively() {
        let (view, _dir) = seed_view().await;
        let plan = walk_for_caps(
            &view,
            &UserPath::new("/Photos").unwrap(),
            &ZipCaps {
                max_entries: 100,
                max_bytes: 1024,
            },
        )
        .await
        .unwrap();
        // Photos + Photos/vacation + Photos/vacation/empty = 3 dirs.
        // Photos/cat.jpg + Photos/dog.jpg + Photos/vacation/beach.jpg = 3 files.
        assert_eq!(plan.entries.len(), 6);
        assert_eq!(plan.total_bytes, 8 + 8 + 16);
    }

    #[tokio::test]
    async fn walk_rejects_on_entries_overflow() {
        let (view, _dir) = seed_view().await;
        let r = walk_for_caps(
            &view,
            &UserPath::new("/Photos").unwrap(),
            &ZipCaps {
                max_entries: 2,
                max_bytes: 1024,
            },
        )
        .await;
        match r {
            Err(WalkError::TooLarge { count, .. }) => assert!(count >= 3),
            other => panic!("expected TooLarge, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn walk_rejects_on_bytes_overflow() {
        let (view, _dir) = seed_view().await;
        let r = walk_for_caps(
            &view,
            &UserPath::new("/Photos").unwrap(),
            &ZipCaps {
                max_entries: 100,
                max_bytes: 10,
            },
        )
        .await;
        match r {
            Err(WalkError::TooLarge { bytes, .. }) => assert!(bytes > 10),
            other => panic!("expected TooLarge, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn walk_includes_empty_directory_as_entry() {
        let (view, _dir) = seed_view().await;
        let plan = walk_for_caps(
            &view,
            &UserPath::new("/Photos").unwrap(),
            &ZipCaps {
                max_entries: 100,
                max_bytes: 1024,
            },
        )
        .await
        .unwrap();
        let empty = plan
            .entries
            .iter()
            .find(|e| e.zip_name == "Photos/vacation/empty");
        assert!(
            empty.is_some(),
            "empty dir must appear as a planned Dir entry"
        );
        assert_eq!(empty.unwrap().kind, PlanKind::Dir);
    }

    #[tokio::test]
    async fn walk_returns_view_error_for_unknown_path() {
        let (view, _tmp) = seed_view().await;
        let r = walk_for_caps(
            &view,
            &UserPath::new("/does-not-exist").unwrap(),
            &ZipCaps {
                max_entries: 100,
                max_bytes: 1024,
            },
        )
        .await;
        match r {
            Err(WalkError::View(_)) => {}
            other => panic!("expected WalkError::View, got {other:?}"),
        }
    }
}
