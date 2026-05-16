//! Public types for the sharing service.

use crate::permissions::SharePermissions;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "i16", try_from = "i16")]
pub enum ShareType {
    User = 0,
    Group = 1,
    Link = 3,
    /// "Email-link" share — a public-link row whose `share_with` is an
    /// email address rather than a user/group id. Routes through
    /// `create_link` like `ShareType::Link`, additionally enqueuing a
    /// `link_emailed` notification to the recipient.
    Email = 4,
}

impl TryFrom<i16> for ShareType {
    type Error = &'static str;
    fn try_from(v: i16) -> Result<Self, Self::Error> {
        match v {
            0 => Ok(ShareType::User),
            1 => Ok(ShareType::Group),
            3 => Ok(ShareType::Link),
            4 => Ok(ShareType::Email),
            _ => Err("unsupported share_type"),
        }
    }
}

impl From<ShareType> for i16 {
    fn from(v: ShareType) -> Self {
        v as i16
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemType {
    File,
    Folder,
}

impl ItemType {
    pub fn as_db_str(&self) -> &'static str {
        match self {
            ItemType::File => "file",
            ItemType::Folder => "folder",
        }
    }
    pub fn from_db_str(s: &str) -> Option<Self> {
        match s {
            "file" => Some(Self::File),
            "folder" => Some(Self::Folder),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ShareRow {
    pub id: i64,
    pub share_type: ShareType,
    pub share_with: Option<String>,
    pub uid_owner: String,
    pub uid_initiator: String,
    pub parent: Option<i64>,
    pub item_type: ItemType,
    pub item_source: i64,
    pub file_source: i64,
    pub file_target: String,
    pub permissions: SharePermissions,
    pub stime: i64,
    pub accepted: bool,
    pub expiration: Option<DateTime<Utc>>,
    pub token: Option<String>,
    pub password_hash: Option<String>,
    /// Timestamp when the expiration-warning mail was enqueued for this
    /// row. `None` means "never warned"; the sweeper stamps this once it
    /// has handed off (whether or not the user opted in).
    pub last_warned: Option<DateTime<Utc>>,
}

/// Caller-supplied create request. The service validates and normalizes
/// before insertion. `requester` is the authenticated user driving the
/// request; SP7 requires `requester == owner`.
#[derive(Debug, Clone)]
pub struct CreateShareRequest {
    pub requester: String,
    /// Path inside the requester's home, as received on the wire. A leading
    /// `/` is tolerated and stripped during lookup.
    pub path: String,
    pub share_type: ShareType,
    pub share_with: String,
    /// Raw OCS bitmask. `Shares::create` strips bit 16 and enforces bit 1.
    pub permissions: u32,
    /// The storage id under which the requester's filecache rows live. The
    /// service does not depend on `crabcloud-fs`; the caller (handler /
    /// `AppState` wiring) resolves this via the home `StorageFactory` and
    /// passes the resulting string in.
    pub home_storage_id: String,
    /// Link-only. `None` = no password. User/group shares ignore this field.
    pub password: Option<String>,
    /// Link-only. `None` = no expiration. User/group shares ignore this field.
    pub expire_date: Option<NaiveDate>,
}

/// Slim projection of an `oc_share` row used by the
/// `ExpirationWarningSweeper`. Carries just the fields the sweeper
/// needs to render the warning template + stamp `last_warned`.
#[derive(Debug, Clone)]
pub struct ExpiringLink {
    pub id: i64,
    pub uid_owner: String,
    /// For Link/Email rows, this is the FULL owner-relative path
    /// (with leading slash) — `file_target` carries the path so the
    /// sweeper can compute a sensible basename for the template.
    pub file_target: String,
    pub token: String,
    pub expiration: chrono::NaiveDateTime,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateShareFields {
    pub permissions: Option<u32>,
    pub expire_date: Option<Option<NaiveDate>>,
    pub password: Option<Option<String>>,
    pub note: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_type_round_trips_via_i16() {
        for v in [0_i16, 1, 3, 4] {
            let st = ShareType::try_from(v).unwrap();
            assert_eq!(i16::from(st), v);
        }
    }

    #[test]
    fn share_type_rejects_unknown() {
        assert!(ShareType::try_from(2_i16).is_err());
        assert!(ShareType::try_from(5_i16).is_err());
        assert!(ShareType::try_from(99_i16).is_err());
    }

    #[test]
    fn item_type_db_round_trip() {
        for it in [ItemType::File, ItemType::Folder] {
            assert_eq!(ItemType::from_db_str(it.as_db_str()), Some(it));
        }
        assert!(ItemType::from_db_str("symlink").is_none());
    }
}
