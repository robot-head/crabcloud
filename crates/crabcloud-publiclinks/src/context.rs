//! `PublicLinkAuthContext` — the request-extension identity carried by
//! anonymous public-link traffic.
//!
//! `PublicLinkAuthLayer` resolves an opaque token, enforces expiration and
//! the password gate, then attaches this value as a request extension so
//! downstream handlers can build a per-request `View` rooted at the linked
//! subtree without re-querying the sharing service.
//!
//! This type is intentionally minimal: it carries only what the storage
//! layer needs (`owner_uid`, `owner_path`, `permissions`) plus the share
//! row id (for audit / telemetry / OCS lookups). Anything else — the
//! resolved Storage handle, the View itself — is built downstream from
//! these three coordinates.

use crabcloud_storage::StoragePath;
use crabcloud_users::UserId;

/// Identity carried by anonymous public-link requests. Stored as a request
/// extension by `PublicLinkAuthLayer`; downstream handlers extract it to
/// build the per-request `View`.
///
/// Permission bits are stored as the raw normalised `u32` (post-`from_wire`,
/// so bit 16 is already cleared) rather than the `SharePermissions` newtype.
/// This avoids a cyclic crate dependency — `crabcloud-sharing` already
/// depends on this crate for `Tokens`/`Passwords`, so we can't pull it back
/// in. Downstream handlers in `crabcloud-fs` / `crabcloud-http` wrap the
/// raw `u32` via `SharePermissions::from_wire` at the boundary.
#[derive(Debug, Clone)]
pub struct PublicLinkAuthContext {
    /// `oc_share.id` of the link row this request authenticated against.
    pub link_share_id: i64,
    /// Owner of the linked subtree. The `View` is rooted in this user's
    /// home storage; permission bits gate what the anonymous visitor can
    /// do inside that subtree.
    pub owner_uid: UserId,
    /// Path inside the owner's home storage where the linked subtree starts.
    /// `SharedSubrootStorage` wraps the home storage at this path.
    pub owner_path: StoragePath,
    /// Permission bits applied to the wrapped subroot, normalised through
    /// `SharePermissions::from_wire` upstream (re-share bit cleared).
    pub permissions: u32,
    /// `true` on a password-protected Browser request whose cookie is missing
    /// or invalid. Downstream handlers MUST check this before serving content;
    /// the only legitimate consumer in this state is the unlock POST handler,
    /// which needs the resolved share to mint a cookie. The viewer page should
    /// render the gate variant in response to this flag.
    pub password_gate_required: bool,
}
