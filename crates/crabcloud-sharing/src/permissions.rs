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

    /// The bit 16 ("share") position. SP7 invariant: this bit is never set on
    /// stored values — `from_wire` strips it. Kept `pub(crate)` so callers can
    /// reason about the mask without being tempted to construct values that
    /// carry it.
    pub(crate) const SHARE_BIT: u8 = 16;

    /// Construct from a raw wire bitmask (e.g. the OCS `permissions=` form
    /// field). Masks to the bits SP7 understands (`0x1F`) and strips the
    /// re-share bit per the spec invariant. The caller must still verify
    /// that bit 1 (read) is set in the original input — see `Shares::create`.
    pub fn from_wire(b: u32) -> Self {
        Self(((b & 0x1F) & !u32::from(Self::SHARE_BIT)) as u8)
    }

    /// Raw bits as stored. Always in `0..=0x0F` post-`from_wire`.
    pub fn as_u8(self) -> u8 {
        self.0
    }

    /// Same value widened to `u32` for arithmetic / OCS wire shape.
    pub fn as_u32(self) -> u32 {
        u32::from(self.0)
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
        Self::from_wire(v as u32)
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
        let p = SharePermissions::from_wire(0b11111); // 31
        assert_eq!(p.as_u8(), 0b01111); // 15
        assert!(p.as_u8() & SharePermissions::SHARE_BIT == 0);
    }

    #[test]
    fn drops_bits_above_31() {
        let p = SharePermissions::from_wire(0xFF);
        assert_eq!(p.as_u8(), 0b01111);
    }

    #[test]
    fn read_only_does_not_allow_write_or_delete() {
        let p = SharePermissions::from_wire(1);
        assert!(p.contains_read());
        assert!(!p.allows_write());
        assert!(!p.allows_delete());
    }

    #[test]
    fn update_allows_write_but_not_create_or_delete() {
        let p = SharePermissions::from_wire(1 | 2);
        assert!(p.allows_write());
        assert!(p.allows_update());
        assert!(!p.allows_create());
        assert!(!p.allows_delete());
    }

    #[test]
    fn create_allows_write_too() {
        let p = SharePermissions::from_wire(1 | 4);
        assert!(p.allows_write());
        assert!(p.allows_create());
        assert!(!p.allows_update());
    }

    #[test]
    fn full_perms_minus_share() {
        let p = SharePermissions::from_wire(31);
        assert!(p.contains_read());
        assert!(p.allows_update());
        assert!(p.allows_create());
        assert!(p.allows_delete());
        assert_eq!(p.as_u8(), 15);
    }

    #[test]
    fn roundtrip_i32() {
        let p = SharePermissions::from_wire(7);
        let n: i32 = p.into();
        assert_eq!(n, 7);
        let p2: SharePermissions = n.into();
        assert_eq!(p2, p);
    }

    #[test]
    fn negative_i32_treated_as_all_bits_then_masked() {
        // -1 as u32 == 0xFFFFFFFF; from_wire masks to 0x0F.
        let p: SharePermissions = (-1_i32).into();
        assert_eq!(p.as_u8(), 0x0F);
    }
}
