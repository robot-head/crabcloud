//! Metadata types: `FileMetadata`, `DirEntry`, `FileKind`, `ETag`,
//! `Mimetype`, `Permissions`, `MultipartHandle`, `PartTag`.

use crate::error::{StorageError, StorageResult};
use crate::path::StoragePath;
use rand::RngExt;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    File,
    Directory,
}

#[derive(Debug, Clone)]
pub struct FileMetadata {
    pub path: StoragePath,
    pub kind: FileKind,
    pub size: u64,
    pub mtime: SystemTime,
    pub etag: ETag,
    pub mimetype: Mimetype,
    pub permissions: Permissions,
}

#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub metadata: FileMetadata,
}

/// 40-char lowercase hex string. Match upstream Nextcloud's ETag shape so
/// existing desktop/iOS/Android clients can detect changes byte-identically.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ETag(String);

impl ETag {
    /// Generate a fresh ETag from the workspace CSPRNG.
    pub fn new() -> Self {
        let mut bytes = [0u8; 20];
        rand::rng().fill(&mut bytes);
        Self(hex::encode(bytes))
    }

    /// Parse a pre-existing ETag string. Validates length + hex.
    pub fn from_hex(s: &str) -> StorageResult<Self> {
        if s.len() != 40 {
            return Err(StorageError::Other(format!(
                "etag length: expected 40 hex chars, got {}",
                s.len()
            )));
        }
        if !s.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) {
            return Err(StorageError::Other(
                "etag: non-lowercase-hex character".into(),
            ));
        }
        Ok(Self(s.to_string()))
    }

    /// Derive an ETag from mtime + an opaque identifier (e.g. inode number).
    /// Stable across reads, changes on mutation. Lower entropy than `new`,
    /// but required when xattr storage is unavailable.
    pub fn from_mtime_and_id(mtime: SystemTime, id: u64) -> Self {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        mtime
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .hash(&mut h);
        id.hash(&mut h);
        // Spread 8 hasher bytes into 20 by mixing with rotations.
        let base = h.finish();
        let mut bytes = [0u8; 20];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = ((base.rotate_left((i * 5) as u32)) & 0xff) as u8;
        }
        Self(hex::encode(bytes))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ETag {
    fn default() -> Self {
        Self::new()
    }
}

/// Canonical "type/subtype" string. Construct via [`Mimetype::parse`] (validates)
/// or [`Mimetype::octet_stream`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Mimetype(String);

impl Mimetype {
    pub fn parse(s: &str) -> StorageResult<Self> {
        if s.is_empty() {
            return Err(StorageError::Other("empty mimetype".into()));
        }
        let mut parts = s.splitn(2, '/');
        let ty = parts
            .next()
            .ok_or_else(|| StorageError::Other("mimetype missing type".into()))?;
        let sub = parts
            .next()
            .ok_or_else(|| StorageError::Other("mimetype missing subtype".into()))?;
        if ty.is_empty() || sub.is_empty() {
            return Err(StorageError::Other("mimetype empty component".into()));
        }
        Ok(Self(s.to_lowercase()))
    }

    pub fn octet_stream() -> Self {
        Self("application/octet-stream".into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Bitmap matching upstream Nextcloud's per-file permission model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions(u8);

impl Permissions {
    pub const READ: u8 = 1;
    pub const UPDATE: u8 = 2;
    pub const CREATE: u8 = 4;
    pub const DELETE: u8 = 8;
    pub const SHARE: u8 = 16;
    pub const ALL: u8 = Self::READ | Self::UPDATE | Self::CREATE | Self::DELETE | Self::SHARE;

    pub fn new(bits: u8) -> Self {
        Self(bits & Self::ALL)
    }

    pub fn full() -> Self {
        Self(Self::ALL)
    }

    pub fn readonly() -> Self {
        Self(Self::READ)
    }

    pub fn bits(self) -> u8 {
        self.0
    }

    pub fn contains(self, other: Permissions) -> bool {
        (self.0 & other.0) == other.0
    }
}

/// Opaque handle to an in-progress multipart upload. The `upload_id` shape
/// is backend-defined (local-fs uses `"local-mp-{random_32}"`; S3 will use
/// AWS's UploadId).
#[derive(Debug, Clone)]
pub struct MultipartHandle {
    pub upload_id: String,
    pub target: StoragePath,
}

/// Caller-replay token for one part of a multipart upload. The `etag` field
/// is backend-defined (S3 returns part ETag; local-fs returns sha256 hex).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartTag {
    pub part_number: u32,
    pub etag: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etag_new_is_40_hex_chars() {
        let e = ETag::new();
        assert_eq!(e.as_str().len(), 40);
        assert!(e
            .as_str()
            .chars()
            .all(|c| matches!(c, '0'..='9' | 'a'..='f')));
    }

    #[test]
    fn etag_new_is_random() {
        let a = ETag::new();
        let b = ETag::new();
        assert_ne!(a, b);
    }

    #[test]
    fn etag_from_hex_validates_length() {
        assert!(ETag::from_hex("abc").is_err());
    }

    #[test]
    fn etag_from_hex_validates_charset() {
        let s: String = "g".repeat(40);
        assert!(ETag::from_hex(&s).is_err());
    }

    #[test]
    fn etag_from_hex_accepts_valid() {
        let s: String = "0123456789abcdef".repeat(2) + "01234567";
        let e = ETag::from_hex(&s).unwrap();
        assert_eq!(e.as_str(), s);
    }

    #[test]
    fn etag_from_mtime_and_id_is_deterministic() {
        let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(123);
        let a = ETag::from_mtime_and_id(t, 42);
        let b = ETag::from_mtime_and_id(t, 42);
        assert_eq!(a, b);
        assert_eq!(a.as_str().len(), 40);
    }

    #[test]
    fn etag_from_mtime_and_id_changes_on_mutation() {
        let t1 = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
        let t2 = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(2);
        assert_ne!(
            ETag::from_mtime_and_id(t1, 42),
            ETag::from_mtime_and_id(t2, 42)
        );
        assert_ne!(
            ETag::from_mtime_and_id(t1, 42),
            ETag::from_mtime_and_id(t1, 43)
        );
    }

    #[test]
    fn mimetype_parse_accepts_simple() {
        let m = Mimetype::parse("text/plain").unwrap();
        assert_eq!(m.as_str(), "text/plain");
    }

    #[test]
    fn mimetype_parse_lowercases() {
        let m = Mimetype::parse("Image/PNG").unwrap();
        assert_eq!(m.as_str(), "image/png");
    }

    #[test]
    fn mimetype_parse_rejects_missing_slash() {
        assert!(Mimetype::parse("plain").is_err());
    }

    #[test]
    fn mimetype_parse_rejects_empty_components() {
        assert!(Mimetype::parse("/plain").is_err());
        assert!(Mimetype::parse("text/").is_err());
    }

    #[test]
    fn mimetype_octet_stream() {
        assert_eq!(
            Mimetype::octet_stream().as_str(),
            "application/octet-stream"
        );
    }

    #[test]
    fn permissions_constants() {
        assert_eq!(Permissions::READ, 1);
        assert_eq!(Permissions::UPDATE, 2);
        assert_eq!(Permissions::CREATE, 4);
        assert_eq!(Permissions::DELETE, 8);
        assert_eq!(Permissions::SHARE, 16);
        assert_eq!(Permissions::ALL, 31);
    }

    #[test]
    fn permissions_full_and_readonly() {
        assert!(Permissions::full().contains(Permissions::new(Permissions::READ)));
        assert!(Permissions::full().contains(Permissions::new(Permissions::DELETE)));
        assert!(!Permissions::readonly().contains(Permissions::new(Permissions::UPDATE)));
    }

    #[test]
    fn permissions_strips_unknown_bits() {
        let p = Permissions::new(0xff);
        assert_eq!(p.bits(), Permissions::ALL);
    }
}
