//! ETag + mimetype xattr persistence.
//!
//! - **Unix**: real extended attributes via the `xattr` crate (`user.crabcloud.etag`,
//!   `user.crabcloud.mimetype`). Best-effort: a failure is logged and swallowed
//!   so the operation still succeeds against filesystems without xattr support.
//! - **Windows**: NTFS alternate data streams (`<path>:crabcloud.etag`,
//!   `<path>:crabcloud.mimetype`) — the closest portable analogue. On non-NTFS
//!   volumes the stream write fails and we silently fall through to the
//!   mtime-derived ETag and recomputed mimetype paths.

use crate::error::StorageError;
use crate::meta::{ETag, Mimetype};
use std::path::Path;

#[cfg(unix)]
const ETAG_KEY: &str = "user.crabcloud.etag";
#[cfg(unix)]
const MIME_KEY: &str = "user.crabcloud.mimetype";

#[cfg(unix)]
pub fn read_etag(p: &Path) -> Option<ETag> {
    xattr::get(p, ETAG_KEY)
        .ok()
        .flatten()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|s| ETag::from_hex(&s).ok())
}

#[cfg(unix)]
pub fn write_etag(p: &Path, etag: &ETag) -> Result<(), StorageError> {
    // Best-effort. If xattr is unsupported, swallow + log; ETag fallback
    // path produces a usable (deterministic) value.
    if let Err(e) = xattr::set(p, ETAG_KEY, etag.as_str().as_bytes()) {
        tracing::debug!(error = %e, path = %p.display(), "xattr etag write failed");
    }
    Ok(())
}

#[cfg(unix)]
pub fn read_mimetype(p: &Path) -> Option<Mimetype> {
    xattr::get(p, MIME_KEY)
        .ok()
        .flatten()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|s| Mimetype::parse(&s).ok())
}

#[cfg(unix)]
pub fn write_mimetype(p: &Path, m: &Mimetype) -> Result<(), StorageError> {
    if let Err(e) = xattr::set(p, MIME_KEY, m.as_str().as_bytes()) {
        tracing::debug!(error = %e, path = %p.display(), "xattr mimetype write failed");
    }
    Ok(())
}

// Windows: NTFS alternate data streams. Open `<path>:streamname` like any
// other path; on non-NTFS volumes the open fails and we treat the metadata
// as unpersisted (falling back to mtime-based recomputation, same as Unix
// xattr-unsupported filesystems).

#[cfg(windows)]
const ETAG_STREAM: &str = ":crabcloud.etag";
#[cfg(windows)]
const MIME_STREAM: &str = ":crabcloud.mimetype";

#[cfg(windows)]
fn stream_path(p: &Path, suffix: &str) -> std::path::PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    std::path::PathBuf::from(s)
}

#[cfg(windows)]
pub fn read_etag(p: &Path) -> Option<ETag> {
    let sp = stream_path(p, ETAG_STREAM);
    std::fs::read(&sp)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|s| ETag::from_hex(&s).ok())
}

#[cfg(windows)]
pub fn write_etag(p: &Path, etag: &ETag) -> Result<(), StorageError> {
    let sp = stream_path(p, ETAG_STREAM);
    if let Err(e) = std::fs::write(&sp, etag.as_str().as_bytes()) {
        tracing::debug!(error = %e, path = %p.display(), "ADS etag write failed");
    }
    Ok(())
}

#[cfg(windows)]
pub fn read_mimetype(p: &Path) -> Option<Mimetype> {
    let sp = stream_path(p, MIME_STREAM);
    std::fs::read(&sp)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|s| Mimetype::parse(&s).ok())
}

#[cfg(windows)]
pub fn write_mimetype(p: &Path, m: &Mimetype) -> Result<(), StorageError> {
    let sp = stream_path(p, MIME_STREAM);
    if let Err(e) = std::fs::write(&sp, m.as_str().as_bytes()) {
        tracing::debug!(error = %e, path = %p.display(), "ADS mimetype write failed");
    }
    Ok(())
}

// Other non-Unix, non-Windows targets: no persistence layer available.
#[cfg(not(any(unix, windows)))]
use tracing as _;

#[cfg(not(any(unix, windows)))]
pub fn read_etag(_p: &Path) -> Option<ETag> {
    None
}
#[cfg(not(any(unix, windows)))]
pub fn write_etag(_p: &Path, _etag: &ETag) -> Result<(), StorageError> {
    Ok(())
}
#[cfg(not(any(unix, windows)))]
pub fn read_mimetype(_p: &Path) -> Option<Mimetype> {
    None
}
#[cfg(not(any(unix, windows)))]
pub fn write_mimetype(_p: &Path, _m: &Mimetype) -> Result<(), StorageError> {
    Ok(())
}
