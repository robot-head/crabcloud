//! COPY handler — restores a version via `Destination:` header.
//!
//! Wire shape matches Nextcloud's `Storage::restoreVersion` surface:
//! a `COPY /dav/versions/{uid}/{fileid}/{version_mtime}` with
//! `Destination: /dav/files/{uid}/<current_path>` snapshots the
//! current bytes (so the pre-restore state is preserved as a NEW
//! version) and then copies the chosen version's bytes over current.
//! Returns 204 No Content on success. The version being restored
//! stays in the versions list — restore is lossless.
//!
//! Validation:
//! - `Destination` header must be present (400 otherwise).
//! - Destination path must equal `/dav/files/{uid}/<entry.path>` or the
//!   `/remote.php` alias (400 otherwise — clients aren't allowed to
//!   restore-into-a-different-path; spec out-of-scope).
//! - `version_mtime` must match an existing row for `(uid, fileid)`
//!   (404 otherwise).

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use crabcloud_core::AppState;
use crabcloud_storage::StoragePath;
use crabcloud_users::UserId;

use crate::routes::dav::error::{DavError, DavResult};

pub async fn restore(
    state: &AppState,
    uid: &str,
    fileid: i64,
    version_mtime: i64,
    headers: &HeaderMap,
) -> DavResult<Response> {
    let dest_raw = headers
        .get("destination")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| DavError::BadRequest("Destination header required".into()))?;
    let dest_path = parse_destination(dest_raw, uid)?;

    let entries = state
        .versions
        .list_for(uid, fileid)
        .await
        .map_err(super::versions_err)?;
    let entry = entries
        .into_iter()
        .find(|e| e.version_mtime == version_mtime)
        .ok_or(DavError::NotFound)?;

    // Restore-into-a-different-path is out of MVP scope: the Destination
    // must match the version row's recorded path (which is the canonical
    // current path of `fileid` in the versions service's view of the
    // world). If the file has been renamed since the snapshot, the row's
    // `path` is the at-snapshot path and the lookup below will mismatch
    // — that's the documented trade-off in spec §6 "MOVE rename".
    if dest_path != entry.path {
        return Err(DavError::BadRequest(format!(
            "Destination must match the file's current path ({})",
            entry.path
        )));
    }

    // Resolve the current filecache size so the snapshot-before-restore
    // can decide whether to actually snapshot. Best-effort: if the
    // current file is missing, `restore` softly skips the pre-snapshot
    // and proceeds (recovery-lever semantics, mirrored from the Versions
    // service).
    let current_size = current_filecache_size(state, uid, &entry.path).await;
    let now = chrono::Utc::now().timestamp();
    let cfg = &state.config;
    state
        .versions
        .restore(
            uid,
            entry.id,
            current_size,
            now,
            cfg.versions_min_interval_secs as i64,
            cfg.versions_max_bytes,
        )
        .await
        .map_err(super::versions_err)?;

    Ok((StatusCode::NO_CONTENT, "").into_response())
}

/// Look up the current size of the owner-relative `path` under `uid`'s
/// home storage. Returns 0 on any lookup failure (missing row, error)
/// — `Versions::restore` interprets 0 as "skip the pre-snapshot",
/// matching the spec §6 "Zero-byte source / source missing" policy.
async fn current_filecache_size(state: &AppState, uid: &str, path: &str) -> i64 {
    let uid_obj = match UserId::new(uid) {
        Ok(u) => u,
        Err(_) => return 0,
    };
    let storage = match state.storage_factory.home_storage(&uid_obj).await {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let storage_id = storage.id().to_string();
    let rel = path.trim_start_matches('/');
    let sp = match StoragePath::new(rel.to_string()) {
        Ok(p) => p,
        Err(_) => return 0,
    };
    match state.filecache.lookup(&storage_id, &sp).await {
        Ok(Some(row)) => row.size as i64,
        _ => 0,
    }
}

/// Strip the surface prefix and the `/files/<uid>/` segment from a
/// `Destination` header; return the user-relative path (with a leading
/// `/`). Mirrors `routes::trashbin::move_::parse_destination` exactly
/// so behavior is consistent across the two restore surfaces.
fn parse_destination(dest: &str, uid: &str) -> DavResult<String> {
    // Strip absolute URL prefix if present (some clients send full URLs).
    let path = match dest.find("://") {
        Some(_) => {
            let (_scheme, after) = dest
                .split_once("://")
                .ok_or_else(|| DavError::BadRequest("malformed Destination URL".into()))?;
            match after.find('/') {
                Some(i) => after[i..].to_string(),
                None => return Err(DavError::BadRequest("Destination missing path".into())),
            }
        }
        None => dest.to_string(),
    };
    // Strip ?query and #fragment before prefix-matching.
    let path = path.split('?').next().unwrap_or("");
    let path = path.split('#').next().unwrap_or("").to_string();
    let decoded = urlencoding::decode(&path)
        .map_err(|e| DavError::BadRequest(format!("invalid url encoding: {e}")))?;
    let decoded = decoded.to_string();
    let prefixes = [
        format!("/remote.php/dav/files/{uid}/"),
        format!("/dav/files/{uid}/"),
    ];
    for p in &prefixes {
        if let Some(rest) = decoded.strip_prefix(p.as_str()) {
            return Ok(format!("/{}", rest.trim_start_matches('/')));
        }
    }
    Err(DavError::BadRequest(format!(
        "Destination not under /dav/files/{uid}/: {dest}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_destination_strips_legacy_prefix() {
        let p = parse_destination("/remote.php/dav/files/alice/foo/bar.txt", "alice").unwrap();
        assert_eq!(p, "/foo/bar.txt");
    }

    #[test]
    fn parse_destination_strips_modern_prefix() {
        let p = parse_destination("/dav/files/alice/x.txt", "alice").unwrap();
        assert_eq!(p, "/x.txt");
    }

    #[test]
    fn parse_destination_rejects_wrong_user() {
        let r = parse_destination("/dav/files/bob/x.txt", "alice");
        assert!(matches!(r, Err(DavError::BadRequest(_))));
    }

    #[test]
    fn parse_destination_decodes_percent_escapes() {
        let p = parse_destination("/dav/files/alice/hello%20world.txt", "alice").unwrap();
        assert_eq!(p, "/hello world.txt");
    }
}
