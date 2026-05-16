//! PROPFIND handler. Returns 207 Multi-Status with the 10-prop set per
//! spec §7.3. Depth 0 = resource only; Depth 1 = resource + children.
//! Depth: infinity is rejected with 403 + `<d:propfind-finite-depth/>`.
//!
//! SP5 ships the live (non-allprop) shape: every response carries the same
//! 10 props regardless of the request body's `<d:prop>` selector. Future
//! hardening will parse the request body and segregate 200 vs 404 propstats.

use crabcloud_core::AppState;
use crabcloud_fs::UserPath;
use crabcloud_storage::{FileKind, Permissions, StoragePath};
use crabcloud_users::UserId;
use quick_xml::events::{BytesEnd, BytesStart, Event};

use crate::routes::dav::error::{DavError, DavResult};
use crate::routes::dav::headers::{parse_depth, Depth};
use crate::routes::dav::xml::{
    multistatus, write_empty, write_leaf, write_propstat, write_response,
};
use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

const FAVORITE_PROP: &str = "{http://owncloud.org/ns}favorite";
const HREF_PREFIX: &str = "/remote.php/dav/files";

/// Display-name shown in `<d:displayname>` for the root of the surface.
/// For the authed surface this is the user id; for the public-link surface
/// it's the link token. The caller picks per-surface.
pub struct PropfindContext<'a> {
    /// HREF prefix up to but not including the leading `/{root_label}/`.
    /// e.g. `/remote.php/dav/files` for the authed surface,
    /// `/public.php/dav/files` for the public surface.
    pub href_prefix: &'a str,
    /// Root segment under `href_prefix`: the user id for the authed
    /// surface, the link token for the public surface.
    pub root_label: &'a str,
    /// Instance identifier for `oc:id` tail; used for cross-host stable
    /// identifiers.
    pub instanceid: &'a str,
}

/// Encode the permission bitmap to the Nextcloud letter-string convention
/// (spec §7.4). The order matters — desktop clients pattern-match the
/// string. Directories pick up an additional `K` when they accept children.
fn permission_str(p: Permissions, kind: FileKind) -> String {
    let mut s = String::new();
    if p.contains(Permissions::new(Permissions::SHARE)) {
        s.push('R');
    }
    if p.contains(Permissions::new(Permissions::DELETE)) {
        s.push('D');
    }
    if p.contains(Permissions::new(Permissions::UPDATE)) {
        s.push('N');
        s.push('V');
        s.push('W');
    }
    if p.contains(Permissions::new(Permissions::CREATE)) {
        s.push('C');
        if matches!(kind, FileKind::Directory) {
            s.push('K');
        }
    }
    s
}

/// Build the `oc:id` value: zero-padded fileid + per-installation
/// instance identifier. Stable across renames; clients use it for
/// cross-host deduplication.
fn oc_id(fileid: i64, instanceid: &str) -> String {
    format!("{:020}{}", fileid, instanceid)
}

/// Build the href value for a response entry. Root paths produce a
/// trailing-slash href so clients reliably detect the collection.
fn href_for(prefix: &str, label: &str, path: &UserPath) -> String {
    if path.is_root() {
        format!("{prefix}/{label}/")
    } else {
        format!("{prefix}/{label}{}", path.as_str())
    }
}

pub async fn handle(
    state: AppState,
    uid: &UserId,
    user_path: &UserPath,
    headers: &HeaderMap,
) -> DavResult<Response> {
    let view = state.view_for(uid).await?;
    let ctx = PropfindContext {
        href_prefix: HREF_PREFIX,
        root_label: uid.as_str(),
        instanceid: state.config.instanceid.as_str(),
    };
    handle_with_view(&view, &state.filecache, uid, user_path, headers, &ctx).await
}

/// Surface-neutral PROPFIND core. Accepts a pre-built `View` and a
/// `PropfindContext` describing the href prefix + display label so the
/// public-link DAV surface (`/public.php/dav/files/{token}`) can reuse
/// the exact same body shape with token-rooted hrefs.
pub async fn handle_with_view(
    view: &crabcloud_fs::View,
    filecache: &crabcloud_filecache::FileCache,
    uid: &UserId,
    user_path: &UserPath,
    headers: &HeaderMap,
    ctx: &PropfindContext<'_>,
) -> DavResult<Response> {
    let depth = parse_depth(headers, Depth::One)?;
    if matches!(depth, Depth::Infinity) {
        return Err(DavError::PropfindFiniteDepth);
    }

    let meta = view.stat(user_path).await?;

    // Compute the filecache key for the resource: routes through any
    // `SharedSubrootStorage` wrapper so cache rows are looked up under the
    // OWNING storage_id + owner-relative path. Falls through to the home
    // storage id + recipient-relative path on the authed surface (where
    // the wrapper isn't present).
    let (self_cache_storage, self_cache_path) = view.cache_key_for(user_path)?;
    let self_storage_id = self_cache_storage.id().to_string();

    // Build the list of (user_path, metadata, fileid) tuples we want to
    // emit one `<d:response>` block for. Depth 0 → just the resource.
    // Each entry also carries its cache (storage_id, path) for the
    // per-entry favorite lookup downstream.
    let mut entries: Vec<(
        UserPath,
        crabcloud_storage::FileMetadata,
        i64,
        String,
        StoragePath,
    )> = Vec::new();
    let self_row = filecache
        .lookup(&self_storage_id, &self_cache_path)
        .await
        .map_err(DavError::from)?;
    let self_fileid = self_row.map(|r| r.fileid).unwrap_or(0);
    entries.push((
        user_path.clone(),
        meta.clone(),
        self_fileid,
        self_storage_id.clone(),
        self_cache_path.clone(),
    ));

    // Depth 1 → enumerate children when the resource is a directory.
    if matches!(depth, Depth::One) && matches!(meta.kind, FileKind::Directory) {
        let children = view.list(user_path).await?;
        for entry in children {
            let child_user_path = if user_path.is_root() {
                UserPath::new(format!("/{}", entry.name))?
            } else {
                user_path.join(&entry.name)?
            };
            let (child_cache_storage, child_cache_path) = view.cache_key_for(&child_user_path)?;
            let child_storage_id = child_cache_storage.id().to_string();
            let row = filecache
                .lookup(&child_storage_id, &child_cache_path)
                .await
                .map_err(DavError::from)?;
            let fileid = row.map(|r| r.fileid).unwrap_or(0);
            entries.push((
                child_user_path,
                entry.metadata,
                fileid,
                child_storage_id,
                child_cache_path,
            ));
        }
    }

    // Batched favorite lookup across the entire entry set. One round-trip,
    // regardless of the directory's child count. Favorites are stored
    // per-user against owner-relative cache paths; for the public-link
    // surface the `uid` is the owner uid (set by the resolver) so this
    // continues to work without per-surface branching.
    let storage_paths: Vec<String> = entries
        .iter()
        .map(|(_, _, _, _, sp)| sp.as_str().to_string())
        .collect();
    let favorites = filecache
        .get_property_many(uid, &storage_paths, FAVORITE_PROP)
        .await
        .map_err(DavError::from)?;
    let favorite_map: std::collections::HashMap<String, Option<String>> =
        favorites.into_iter().collect();

    let instanceid = ctx.instanceid.to_string();
    let href_prefix = ctx.href_prefix.to_string();
    let root_label = ctx.root_label.to_string();

    let body = multistatus(|w| {
        for (path, m, fileid, _sid, cache_path) in &entries {
            let href = href_for(&href_prefix, &root_label, path);
            let favorite = favorite_map
                .get(cache_path.as_str())
                .and_then(|v| v.as_deref())
                .unwrap_or("0");
            let displayname = if path.is_root() {
                root_label.as_str()
            } else {
                path.basename()
            };
            write_response(w, &href, |w| {
                write_propstat(w, "HTTP/1.1 200 OK", |w| {
                    if matches!(m.kind, FileKind::File) {
                        write_leaf(w, "d:getcontentlength", &m.size.to_string())?;
                        write_leaf(w, "d:getcontenttype", m.mimetype.as_str())?;
                    }
                    write_leaf(w, "d:getetag", &format!("\"{}\"", m.etag.as_str()))?;
                    write_leaf(w, "d:getlastmodified", &httpdate::fmt_http_date(m.mtime))?;
                    w.write_event(Event::Start(BytesStart::new("d:resourcetype")))?;
                    if matches!(m.kind, FileKind::Directory) {
                        write_empty(w, "d:collection")?;
                    }
                    w.write_event(Event::End(BytesEnd::new("d:resourcetype")))?;
                    write_leaf(w, "d:displayname", displayname)?;
                    write_leaf(w, "oc:id", &oc_id(*fileid, &instanceid))?;
                    write_leaf(w, "oc:permissions", &permission_str(m.permissions, m.kind))?;
                    write_leaf(w, "oc:size", &m.size.to_string())?;
                    write_leaf(w, "oc:favorite", favorite)?;
                    Ok(())
                })
            })?;
        }
        Ok(())
    });

    Ok((
        StatusCode::from_u16(207).expect("207 is a valid status code"),
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/xml; charset=utf-8"),
        )],
        Body::from(body),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_str_full_file() {
        let s = permission_str(Permissions::full(), FileKind::File);
        // R (share), D (delete), NVW (update), C (create) — no K on a file.
        assert_eq!(s, "RDNVWC");
    }

    #[test]
    fn permission_str_full_dir_includes_k() {
        let s = permission_str(Permissions::full(), FileKind::Directory);
        assert_eq!(s, "RDNVWCK");
    }

    #[test]
    fn permission_str_readonly() {
        // READ alone has no letter in the upstream encoding — read access is
        // implied; only mutation/share rights are surfaced.
        let s = permission_str(Permissions::readonly(), FileKind::File);
        assert_eq!(s, "");
    }

    #[test]
    fn oc_id_zero_pads_fileid() {
        assert_eq!(oc_id(42, "abc"), "00000000000000000042abc");
        assert_eq!(oc_id(0, "x"), "00000000000000000000x");
    }
}
