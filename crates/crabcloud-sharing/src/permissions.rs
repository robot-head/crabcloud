//! Permission bitmask wrapper for shares. Layout matches Nextcloud:
//! bit 1 = read, 2 = update, 4 = create, 8 = delete, 16 = share.
//!
//! SP7 invariant: stored values always have bit 16 cleared (no re-share).

use serde::{Deserialize, Serialize};

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SharePermissions(u8);

impl SharePermissions {
    pub const READ: Self = Self(1);
    pub const UPDATE: Self = Self(2);
    pub const CREATE: Self = Self(4);
    pub const DELETE: Self = Self(8);
    pub const SHARE: Self = Self(16);

    /// Mask off bits we don't track (>= 32) and the SP7-prohibited share bit
    /// (16). The caller is responsible for asserting that bit 1 (read) is
    /// still set after this — see `Shares::create`.
    pub fn from_bitmask_strip_share(b: u32) -> Self {
        Self(((b & 0x1F) & !(Self::SHARE.0 as u32)) as u8)
    }

    pub fn bits(self) -> u8 {
        self.0
    }
    pub fn bitmask(self) -> u32 {
        self.0 as u32
    }

    pub fn contains_read(self) -> bool {
        (self.0 & Self::READ.0) != 0
    }
    pub fn allows_write(self) -> bool {
        (self.0 & (Self::UPDATE.0 | Self::CREATE.0)) != 0
    }
    pub fn allows_update(self) -> bool {
        (self.0 & Self::UPDATE.0) != 0
    }
    pub fn allows_create(self) -> bool {
        (self.0 & Self::CREATE.0) != 0
    }
    pub fn allows_delete(self) -> bool {
        (self.0 & Self::DELETE.0) != 0
    }
}

impl From<i32> for SharePermissions {
    fn from(v: i32) -> Self {
        Self::from_bitmask_strip_share(v as u32)
    }
}
impl From<SharePermissions> for i32 {
    fn from(v: SharePermissions) -> Self {
        v.0 as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_share_bit() {
        let p = SharePermissions::from_bitmask_strip_share(0b11111); // 31
        assert_eq!(p.bits(), 0b01111); // 15
        assert!((p.bitmask() as u8) & SharePermissions::SHARE.0 == 0);
    }

    #[test]
    fn drops_bits_above_31() {
        let p = SharePermissions::from_bitmask_strip_share(0xFF);
        assert_eq!(p.bits(), 0b01111);
    }

    #[test]
    fn read_only_does_not_allow_write_or_delete() {
        let p = SharePermissions::from_bitmask_strip_share(1);
        assert!(p.contains_read());
        assert!(!p.allows_write());
        assert!(!p.allows_delete());
    }

    #[test]
    fn update_allows_write_but_not_create_or_delete() {
        let p = SharePermissions::from_bitmask_strip_share(1 | 2);
        assert!(p.allows_write());
        assert!(p.allows_update());
        assert!(!p.allows_create());
        assert!(!p.allows_delete());
    }

    #[test]
    fn create_allows_write_too() {
        let p = SharePermissions::from_bitmask_strip_share(1 | 4);
        assert!(p.allows_write());
        assert!(p.allows_create());
        assert!(!p.allows_update());
    }

    #[test]
    fn full_perms_minus_share() {
        let p = SharePermissions::from_bitmask_strip_share(31);
        assert!(p.contains_read());
        assert!(p.allows_update());
        assert!(p.allows_create());
        assert!(p.allows_delete());
        assert_eq!(p.bits(), 15);
    }

    #[test]
    fn roundtrip_i32() {
        let p = SharePermissions::from_bitmask_strip_share(7);
        let n: i32 = p.into();
        assert_eq!(n, 7);
        let p2: SharePermissions = n.into();
        assert_eq!(p2, p);
    }
}
