//! `#[server]` functions for the public-link viewer (`/s/:token` …).
//!
//! These run inside the dx fullstack server. The axum auth middleware
//! (`public_link_auth`) is mounted on `/s/{token}` upstream of dx, so by the
//! time these server fns execute, the request already carries a
//! `PublicLinkAuthContext` request extension built from the resolved share
//! row. The fns extract that context, build a one-shot `View` via
//! `PublicLinkMountResolver`, and either return a folder listing or a small
//! metadata DTO that tells the page whether to render the password gate /
//! upload widget.
//!
//! NOTE on the meta fn: even when `password_gate_required == true`, the auth
//! context is present (the layer attaches it specifically so the unlock
//! endpoint and the gate-rendering page can both reach it). The
//! list endpoint refuses to act in that state; the meta endpoint reports
//! the flag so the page can render the gate instead.

use crate::server_fns::FileEntry;
use dioxus::fullstack::get;
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// Per-link metadata used by the viewer to pick which UI variant to render.
/// Carrying `path_basename` separately from the link path saves a stat round
/// trip on the client when it wants to put a folder name in the header.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicLinkMeta {
    /// True if a valid unlock cookie is required and either missing or
    /// invalid. The page renders the password form in this state.
    pub password_required: bool,
    pub can_read: bool,
    pub can_create: bool,
    pub can_update: bool,
    pub can_delete: bool,
    /// Last segment of the linked subroot, or `"/"` if the link points at
    /// the owner's root (currently impossible because link rows always
    /// point at a non-root subtree, but we don't rely on that here).
    pub root_name: String,
}

/// `GET /api/public_link/meta?token=…` — lightweight probe used by the
/// public-link viewer page to choose a UI variant (password gate vs. folder
/// listing vs. file-drop). Splitting this off from the listing endpoint
/// means the gate page never triggers a `list` call that would 403 against
/// `password_gate_required == true`.
///
/// The route is `#[get]` so the dx fullstack server can serve it without a
/// CSRF token (the public-link surface is anonymous; CSRF protection is
/// scoped to authenticated session POSTs in `csrf.rs`).
#[get("/api/public_link/meta")]
pub async fn meta_public_link(token: String) -> Result<PublicLinkMeta, ServerFnError> {
    use crabcloud_publiclinks::{PublicLinkAuthContext, Token};
    use dioxus::fullstack::FullstackContext;

    let _validated = Token::parse(&token).ok_or_else(|| ServerFnError::new("bad_token"))?;
    let fs =
        FullstackContext::current().ok_or_else(|| ServerFnError::new("not running on server"))?;
    let ctx = fs
        .extension::<PublicLinkAuthContext>()
        .ok_or_else(|| ServerFnError::new("public_link_context_missing"))?;

    let perms = crabcloud_sharing::SharePermissions::from_wire(ctx.permissions);
    let root_name = if ctx.owner_path.is_root() {
        "/".to_string()
    } else {
        ctx.owner_path.basename().to_string()
    };
    Ok(PublicLinkMeta {
        password_required: ctx.password_gate_required,
        can_read: perms.contains_read(),
        can_create: perms.allows_create(),
        can_update: perms.allows_update(),
        can_delete: perms.allows_delete(),
        root_name,
    })
}

/// `GET /api/public_link/list?token=…&path=…` — folder listing for the
/// public-link viewer. Refuses when the request still requires a password
/// (the page should render the gate, not the listing).
#[get("/api/public_link/list")]
pub async fn list_public_link(
    token: String,
    path: String,
) -> Result<Vec<FileEntry>, ServerFnError> {
    use crabcloud_fs::{PublicLinkMountResolver, UserPath, View};
    use crabcloud_publiclinks::{PublicLinkAuthContext, Token};
    use crabcloud_users::UserId;
    use dioxus::fullstack::FullstackContext;
    use std::sync::Arc;

    let _validated = Token::parse(&token).ok_or_else(|| ServerFnError::new("bad_token"))?;
    let fs =
        FullstackContext::current().ok_or_else(|| ServerFnError::new("not running on server"))?;
    let state = fs
        .extension::<crabcloud_core::AppState>()
        .ok_or_else(|| ServerFnError::new("AppState extension missing"))?;
    let ctx = fs
        .extension::<PublicLinkAuthContext>()
        .ok_or_else(|| ServerFnError::new("public_link_context_missing"))?;
    if ctx.password_gate_required {
        return Err(ServerFnError::new("password_required"));
    }

    let user_path =
        UserPath::new(&path).map_err(|e| ServerFnError::new(format!("invalid_path: {e}")))?;
    let perms = crabcloud_sharing::SharePermissions::from_wire(ctx.permissions);

    let resolver = Arc::new(PublicLinkMountResolver::new(
        state.storage_factory.clone(),
        ctx.owner_uid.clone(),
        ctx.owner_path.clone(),
        perms,
    ));
    // Synthetic "anonymous" identity — the resolver ignores the uid argument,
    // and the View only uses it for filecache routing, which already happens
    // through the owner's storage id because `SharedSubrootStorage::id()` is
    // the inner storage id.
    let anon = UserId::new("public-link").map_err(|e| ServerFnError::new(format!("uid: {e}")))?;
    let _ = anon; // kept for clarity; the resolver ignores its argument
    let mounts = resolver
        .mounts_for(&ctx.owner_uid)
        .await
        .map_err(|e| ServerFnError::new(format!("mounts: {e}")))?;
    let view = View::new(
        ctx.owner_uid.clone(),
        mounts,
        state.filecache.clone(),
        state.storage_sink.clone(),
        state.trash.clone(),
    );

    let entries = view
        .list(&user_path)
        .await
        .map_err(|e| ServerFnError::new(format!("list: {e}")))?;
    let mut out: Vec<FileEntry> = entries
        .into_iter()
        .map(|de| dir_entry_to_dto(&user_path, de))
        .collect();
    out.sort_by(|a, b| match (b.is_dir, a.is_dir) {
        (true, false) => std::cmp::Ordering::Greater,
        (false, true) => std::cmp::Ordering::Less,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    Ok(out)
}

/// Need MountResolver in scope for `.mounts_for`.
#[cfg(feature = "server")]
use crabcloud_fs::MountResolver as _MountResolver;

#[cfg(feature = "server")]
fn dir_entry_to_dto(
    parent: &crabcloud_fs::UserPath,
    entry: crabcloud_storage::DirEntry,
) -> FileEntry {
    use std::time::UNIX_EPOCH;
    let full_path = parent
        .join(&entry.name)
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|_| entry.name.clone());
    let mtime_ms = entry
        .metadata
        .mtime
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let is_dir = matches!(entry.metadata.kind, crabcloud_storage::FileKind::Directory);
    FileEntry {
        name: entry.name,
        path: full_path,
        is_dir,
        size: entry.metadata.size,
        mtime_ms,
        mime: (!is_dir).then(|| entry.metadata.mimetype.as_str().to_string()),
        etag: entry.metadata.etag.as_str().to_string(),
        // Public-link previews are keyed by `(token, path)`, not fileid;
        // leave this `None` to avoid exposing internal cache ids to
        // anonymous viewers.
        fileid: None,
        shared_by: None,
        share_count: 0,
    }
}
