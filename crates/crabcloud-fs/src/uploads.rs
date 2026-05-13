//! `Uploads` — chunked upload façade. Translates Nextcloud's chunked-upload
//! HTTP protocol (PUT chunks to `/dav/uploads/<user>/<upload_id>/<n>` +
//! MOVE-with-Destination to commit) into the Storage trait's multipart
//! primitives.
//!
//! The `upload_id` returned to the client is opaque + self-describing.
//! Format: `"{path_prefix_b64}:{dest_path_b64}:{backend_upload_id_b64}"`.
//! Each `*_b64` is URL-safe (no-pad) base64 of the raw UTF-8 string. The
//! backend id is whatever the storage backend returned (e.g.,
//! `local-mp-<random>` for LocalStorage).

use crate::error::{FsError, FsResult};
use crate::mount::Mount;
use crate::path::UserPath;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use crabcloud_filecache::FileCache;
use crabcloud_storage::{ChannelEventSink, FileMetadata, MultipartHandle, PartTag, StoragePath};
use crabcloud_users::UserId;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::AsyncRead;

pub struct Uploads {
    pub(crate) uid: UserId,
    pub(crate) mounts: Vec<Mount>,
    pub(crate) storage_sink: Arc<ChannelEventSink>,
    /// Held for post-commit filecache invalidation (wired in Batch E /
    /// when the scanner doesn't auto-update). Currently the storage
    /// `Written` event drives the filecache update via the sink.
    #[allow(dead_code)]
    pub(crate) filecache: Arc<FileCache>,
}

#[derive(Debug, Clone)]
pub struct UploadHandle {
    /// Opaque, self-describing upload id. Pass back to `put_part`/`commit`/
    /// `abort`. Survives server restarts as long as the backing storage's
    /// multipart state survives (LocalStorage tempdir / S3 UploadId).
    pub upload_id: String,
    pub destination: UserPath,
}

impl Uploads {
    pub fn new(
        uid: UserId,
        mounts: Vec<Mount>,
        storage_sink: Arc<ChannelEventSink>,
        filecache: Arc<FileCache>,
    ) -> Self {
        Self {
            uid,
            mounts,
            storage_sink,
            filecache,
        }
    }

    pub fn uid(&self) -> &UserId {
        &self.uid
    }

    fn resolve(&self, user_path: &UserPath) -> FsResult<(&Mount, StoragePath)> {
        // Same algorithm as View::resolve. Duplicated to keep Uploads
        // independent of View — they share a trait surface naturally
        // (both consume `Vec<Mount>`).
        let trimmed = user_path.as_str().trim_start_matches('/');
        let best = self
            .mounts
            .iter()
            .filter(|m| {
                let prefix = m.path_prefix.as_str();
                prefix.is_empty() || trimmed == prefix || trimmed.starts_with(&format!("{prefix}/"))
            })
            .max_by_key(|m| m.path_prefix.as_str().len())
            .ok_or(FsError::MountNotFound)?;
        let suffix = if best.path_prefix.is_root() {
            trimmed.to_string()
        } else {
            let with_slash = format!("{}/", best.path_prefix.as_str());
            trimmed
                .strip_prefix(&with_slash)
                .map(String::from)
                .unwrap_or_default()
        };
        let storage_path = StoragePath::new(suffix)?;
        Ok((best, storage_path))
    }

    /// Begin a new upload. Returns an opaque `upload_id`.
    pub async fn begin(&self, destination: &UserPath) -> FsResult<UploadHandle> {
        let (mount, storage_path) = self.resolve(destination)?;
        let handle = mount
            .storage
            .begin_multipart(&storage_path, &*self.storage_sink)
            .await?;
        let upload_id = encode_upload_id(&mount.path_prefix, &storage_path, &handle.upload_id);
        Ok(UploadHandle {
            upload_id,
            destination: destination.clone(),
        })
    }

    /// Receive a chunk for an in-progress upload.
    pub async fn put_part(
        &self,
        upload_id: &str,
        part_number: u32,
        body: Pin<Box<dyn AsyncRead + Send>>,
    ) -> FsResult<PartTag> {
        let (mount, storage_path, backend_id) = decode_upload_id(upload_id, &self.mounts)?;
        let handle = MultipartHandle {
            upload_id: backend_id,
            target: storage_path,
        };
        let tag = mount.storage.put_part(&handle, part_number, body).await?;
        Ok(tag)
    }

    /// Abort an in-progress upload. Idempotent on unknown `upload_id`.
    ///
    /// The backend's `abort_multipart` is best-effort: a tempdir teardown
    /// failure isn't actionable from the HTTP layer, so we swallow it.
    pub async fn abort(&self, upload_id: &str) -> FsResult<()> {
        let (mount, storage_path, backend_id) = match decode_upload_id(upload_id, &self.mounts) {
            Ok(x) => x,
            Err(_) => return Ok(()),
        };
        let handle = MultipartHandle {
            upload_id: backend_id,
            target: storage_path,
        };
        let _ = mount.storage.abort_multipart(handle).await;
        Ok(())
    }

    /// Commit the upload at the supplied destination. Errors if
    /// `destination` doesn't match what was passed to `begin`.
    pub async fn commit(
        &self,
        upload_id: &str,
        destination: &UserPath,
        parts: Vec<PartTag>,
    ) -> FsResult<FileMetadata> {
        let (mount, storage_path, backend_id) = decode_upload_id(upload_id, &self.mounts)?;
        let (dest_mount, dest_path) = self.resolve(destination)?;
        if dest_mount.path_prefix != mount.path_prefix || dest_path != storage_path {
            return Err(FsError::Upload("destination mismatch".into()));
        }
        let handle = MultipartHandle {
            upload_id: backend_id,
            target: storage_path,
        };
        let meta = mount
            .storage
            .commit_multipart(handle, parts, &*self.storage_sink)
            .await?;
        Ok(meta)
    }
}

fn encode_upload_id(prefix: &StoragePath, dest: &StoragePath, backend: &str) -> String {
    let p = URL_SAFE_NO_PAD.encode(prefix.as_str().as_bytes());
    let d = URL_SAFE_NO_PAD.encode(dest.as_str().as_bytes());
    let b = URL_SAFE_NO_PAD.encode(backend.as_bytes());
    format!("{p}:{d}:{b}")
}

/// Decode an `upload_id` produced by `encode_upload_id`. Returns
/// `(mount, dest_path, backend_upload_id)`. Errors if the id is malformed
/// or if no mount matches the encoded prefix.
fn decode_upload_id<'m>(
    encoded: &str,
    mounts: &'m [Mount],
) -> FsResult<(&'m Mount, StoragePath, String)> {
    let parts: Vec<&str> = encoded.splitn(3, ':').collect();
    if parts.len() != 3 {
        return Err(FsError::Upload("malformed upload id".into()));
    }
    let prefix_bytes = URL_SAFE_NO_PAD
        .decode(parts[0])
        .map_err(|_| FsError::Upload("malformed upload id (prefix not base64)".into()))?;
    let dest_bytes = URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|_| FsError::Upload("malformed upload id (dest not base64)".into()))?;
    let backend_bytes = URL_SAFE_NO_PAD
        .decode(parts[2])
        .map_err(|_| FsError::Upload("malformed upload id (backend not base64)".into()))?;
    let prefix_str =
        String::from_utf8(prefix_bytes).map_err(|_| FsError::Upload("prefix not utf-8".into()))?;
    let dest_str =
        String::from_utf8(dest_bytes).map_err(|_| FsError::Upload("dest not utf-8".into()))?;
    let backend_str = String::from_utf8(backend_bytes)
        .map_err(|_| FsError::Upload("backend not utf-8".into()))?;

    let mount = mounts
        .iter()
        .find(|m| m.path_prefix.as_str() == prefix_str)
        .ok_or_else(|| FsError::Upload("unknown mount".into()))?;
    let storage_path =
        StoragePath::new(dest_str).map_err(|e| FsError::Upload(format!("invalid dest: {e}")))?;
    Ok((mount, storage_path, backend_str))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crabcloud_storage::{memory::MemoryStorage, Storage};

    fn mount_for(prefix: &str, id: &str) -> Mount {
        let p = if prefix.is_empty() {
            StoragePath::root()
        } else {
            StoragePath::new(prefix).unwrap()
        };
        Mount {
            path_prefix: p,
            storage: Arc::new(MemoryStorage::new(id)) as Arc<dyn Storage>,
            metadata: None,
        }
    }

    #[test]
    fn upload_id_round_trip_root_mount() {
        let prefix = StoragePath::root();
        let dest = StoragePath::new("photos/cat.jpg").unwrap();
        let backend = "local-mp-abc123";
        let encoded = encode_upload_id(&prefix, &dest, backend);
        assert!(encoded.contains(':'));

        let mounts = vec![mount_for("", "home")];
        let (mount, decoded_dest, decoded_backend) = decode_upload_id(&encoded, &mounts).unwrap();
        assert_eq!(mount.path_prefix, prefix);
        assert_eq!(decoded_dest, dest);
        assert_eq!(decoded_backend, backend);
    }

    #[test]
    fn upload_id_round_trip_shared_mount() {
        let prefix = StoragePath::new("Shared").unwrap();
        let dest = StoragePath::new("joe/photos/cat.jpg").unwrap();
        let backend = "local-mp-xyz";
        let encoded = encode_upload_id(&prefix, &dest, backend);

        let mounts = vec![mount_for("", "home"), mount_for("Shared", "shared")];
        let (mount, decoded_dest, decoded_backend) = decode_upload_id(&encoded, &mounts).unwrap();
        assert_eq!(mount.path_prefix.as_str(), "Shared");
        assert_eq!(decoded_dest, dest);
        assert_eq!(decoded_backend, backend);
    }

    #[test]
    fn malformed_upload_id_rejected() {
        let mounts = vec![mount_for("", "home")];
        assert!(matches!(
            decode_upload_id("not-base64", &mounts),
            Err(FsError::Upload(_))
        ));
        assert!(matches!(
            decode_upload_id("a:b", &mounts),
            Err(FsError::Upload(_))
        ));
        assert!(matches!(
            decode_upload_id("!@:#$:%^", &mounts),
            Err(FsError::Upload(_))
        ));
    }

    #[test]
    fn unknown_mount_prefix_rejected() {
        let mounts = vec![mount_for("", "home")];
        // Encode with a "Phantom" prefix that doesn't exist in mounts.
        let encoded = encode_upload_id(
            &StoragePath::new("Phantom").unwrap(),
            &StoragePath::new("x").unwrap(),
            "id",
        );
        assert!(matches!(
            decode_upload_id(&encoded, &mounts),
            Err(FsError::Upload(_))
        ));
    }
}
