//! Row shapes used by `FileCache`. `FilecacheRow` is the public type;
//! `FilecacheRowRaw` is the per-dialect FromRow target that maps directly
//! to `oc_filecache` columns + the joined storage/mimetype strings.

use crabcloud_storage::{ETag, FileKind, Mimetype, Permissions, StoragePath};

use crate::error::{FileCacheError, FileCacheResult};

/// Public cache row. Fields are typed (StoragePath, ETag, etc.) — convert
/// from `FilecacheRowRaw` via [`FilecacheRowRaw::into_row`].
#[derive(Debug, Clone)]
pub struct FilecacheRow {
    pub fileid: i64,
    pub storage_id: String,
    pub path: StoragePath,
    pub parent: Option<i64>,
    pub name: String,
    pub kind: FileKind,
    pub mimetype: Mimetype,
    pub size: u64,
    pub mtime: u64,
    pub storage_mtime: u64,
    pub etag: ETag,
    pub permissions: Permissions,
}

/// SQL row shape: scalar columns + the two joined strings (storage id,
/// mimetype). Construct via the per-dialect SELECT queries in `populate.rs` /
/// `propagate.rs` (added in Batch B/C).
#[derive(Debug, Clone)]
pub struct FilecacheRowRaw {
    pub fileid: i64,
    pub storage_id: String,
    pub path: String,
    pub parent: Option<i64>,
    pub name: String,
    pub mimetype: String,
    pub size: i64,
    pub mtime: i64,
    pub storage_mtime: i64,
    pub etag: String,
    pub permissions: i64,
}

impl FilecacheRowRaw {
    pub fn into_row(self) -> FileCacheResult<FilecacheRow> {
        let path = StoragePath::new(self.path)
            .map_err(|e| FileCacheError::Invalid(format!("filecache row has invalid path: {e}")))?;
        let etag = ETag::from_hex(&self.etag)
            .map_err(|e| FileCacheError::Invalid(format!("filecache row has invalid etag: {e}")))?;
        let mimetype = Mimetype::parse(&self.mimetype).map_err(|e| {
            FileCacheError::Invalid(format!("filecache row has invalid mimetype: {e}"))
        })?;
        let kind = if self.mimetype.starts_with(DIRECTORY_MIMETYPE) {
            FileKind::Directory
        } else {
            FileKind::File
        };
        Ok(FilecacheRow {
            fileid: self.fileid,
            storage_id: self.storage_id,
            path,
            parent: self.parent,
            name: self.name,
            kind,
            mimetype,
            size: self.size as u64,
            mtime: self.mtime as u64,
            storage_mtime: self.storage_mtime as u64,
            etag,
            permissions: Permissions::new(self.permissions as u8),
        })
    }
}

/// hex(md5(path)). Used as the indexed lookup column on `oc_filecache`.
/// Matches upstream Nextcloud's path_hash convention.
pub fn path_hash(path: &StoragePath) -> String {
    use md5::Digest;
    let digest = md5::Md5::digest(path.as_str().as_bytes());
    hex::encode(digest)
}

/// "image/png" -> "image" (the part used for `oc_filecache.mimepart`).
/// "x" without a slash returns "x" (mimetype constructor already rejects
/// missing-slash strings, but defend against bad rows from older DBs).
pub fn type_half(mimetype: &str) -> &str {
    mimetype.split_once('/').map(|(t, _)| t).unwrap_or(mimetype)
}

/// Marker mimetype for directories. Matches upstream Nextcloud.
pub const DIRECTORY_MIMETYPE: &str = "httpd/unix-directory";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_hash_is_32_hex_chars() {
        let p = StoragePath::new("a/b/c.txt").unwrap();
        let h = path_hash(&p);
        assert_eq!(h.len(), 32);
        assert!(h.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
    }

    #[test]
    fn path_hash_root_is_md5_of_empty() {
        let h = path_hash(&StoragePath::root());
        // md5("") = d41d8cd98f00b204e9800998ecf8427e
        assert_eq!(h, "d41d8cd98f00b204e9800998ecf8427e");
    }

    #[test]
    fn path_hash_known_value() {
        let p = StoragePath::new("hello.txt").unwrap();
        // md5("hello.txt") = 2e54144ba487ae25d03a3caba233da71
        // (verified: `printf "%s" hello.txt | md5sum`)
        assert_eq!(path_hash(&p), "2e54144ba487ae25d03a3caba233da71");
    }

    #[test]
    fn type_half_splits_mimetype() {
        assert_eq!(type_half("image/png"), "image");
        assert_eq!(type_half("application/octet-stream"), "application");
        assert_eq!(type_half("text/x-rust"), "text");
    }

    #[test]
    fn type_half_passes_through_malformed() {
        assert_eq!(type_half("malformed"), "malformed");
    }

    #[test]
    fn directory_mimetype_parses() {
        // Sanity-check that the marker mimetype satisfies `Mimetype::parse`
        // — we use it as a regular Mimetype value when building rows for
        // newly-mkdir'd directories.
        let m = Mimetype::parse(DIRECTORY_MIMETYPE).unwrap();
        assert_eq!(m.as_str(), DIRECTORY_MIMETYPE);
    }
}
