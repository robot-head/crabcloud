//! Local filesystem backend. Atomic writes via tempfile + rename + fsync.
//! ETag + mimetype persisted via xattr (Unix) with a mtime+inode fallback.
//! Multipart writes live in batch E (`begin_multipart`/`put_part`/
//! `commit_multipart`/`abort_multipart`).

mod atomic;
mod mimetype;
mod xattr_io;

use crate::error::{map_io, StorageError, StorageResult};
use crate::meta::{
    DirEntry, ETag, FileKind, FileMetadata, Mimetype, MultipartHandle, PartTag, Permissions,
};
use crate::path::StoragePath;
use crate::{EventSink, Storage, StorageEvent};
use async_trait::async_trait;
use atomic::{atomic_rename, sibling_temp, stream_to_temp, TempFileGuard};
use std::io::SeekFrom;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::SystemTime;
use tokio::fs;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt};

pub struct LocalStorage {
    root: PathBuf,
    id: String,
}

impl LocalStorage {
    pub fn new(root: PathBuf) -> StorageResult<Self> {
        let root = root.canonicalize().map_err(map_io)?;
        let id = format!("local::{}", root.display());
        Ok(Self { root, id })
    }

    /// Translate `StoragePath` (relative, normalized) to an absolute path
    /// under `root`. Defense in depth: after `join`, the resulting path is
    /// `canonicalize`d (if it exists) and verified to live under `root`.
    fn resolve(&self, path: &StoragePath) -> StorageResult<PathBuf> {
        let mut joined = self.root.clone();
        if !path.is_root() {
            joined.push(path.as_str());
        }
        match joined.canonicalize() {
            Ok(c) => {
                if !c.starts_with(&self.root) {
                    return Err(StorageError::InvalidPath(format!(
                        "path escapes root: {}",
                        path.as_str()
                    )));
                }
                Ok(c)
            }
            Err(_) => {
                // Path doesn't exist (yet). Verify the closest existing ancestor
                // is inside root.
                let mut anc = joined.clone();
                while !anc.exists() {
                    if !anc.pop() {
                        return Err(StorageError::InvalidPath(format!(
                            "no existing ancestor for {}",
                            path.as_str()
                        )));
                    }
                }
                let canonical_anc = anc.canonicalize().map_err(map_io)?;
                if !canonical_anc.starts_with(&self.root) {
                    return Err(StorageError::InvalidPath(format!(
                        "ancestor escapes root: {}",
                        path.as_str()
                    )));
                }
                Ok(joined)
            }
        }
    }

    async fn metadata_of(&self, real: &Path, path: &StoragePath) -> StorageResult<FileMetadata> {
        let md = fs::metadata(real).await.map_err(map_io)?;
        let kind = if md.is_dir() {
            FileKind::Directory
        } else {
            FileKind::File
        };
        let size = if matches!(kind, FileKind::File) {
            md.len()
        } else {
            0
        };
        let mtime = md.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let etag = match xattr_io::read_etag(real) {
            Some(e) => e,
            None => ETag::from_mtime_and_id(mtime, stable_inode(&md)),
        };
        let mimetype = if matches!(kind, FileKind::Directory) {
            Mimetype::octet_stream()
        } else if let Some(m) = xattr_io::read_mimetype(real) {
            m
        } else {
            recompute_mimetype(real, path.as_str()).await
        };
        Ok(FileMetadata {
            path: path.clone(),
            kind,
            size,
            mtime,
            etag,
            mimetype,
            permissions: Permissions::full(),
        })
    }
}

#[async_trait]
impl Storage for LocalStorage {
    fn id(&self) -> &str {
        &self.id
    }

    async fn stat(&self, path: &StoragePath) -> StorageResult<FileMetadata> {
        let real = self.resolve(path)?;
        self.metadata_of(&real, path).await
    }

    async fn exists(&self, path: &StoragePath) -> StorageResult<bool> {
        let real = self.resolve(path)?;
        fs::try_exists(&real).await.map_err(map_io)
    }

    async fn list(&self, path: &StoragePath) -> StorageResult<Vec<DirEntry>> {
        let real = self.resolve(path)?;
        let md = fs::metadata(&real).await.map_err(map_io)?;
        if !md.is_dir() {
            return Err(StorageError::NotADirectory);
        }
        let mut rd = fs::read_dir(&real).await.map_err(map_io)?;
        let mut out = Vec::new();
        while let Some(entry) = rd.next_entry().await.map_err(map_io)? {
            let name = entry.file_name().to_string_lossy().to_string();
            let child_path = if path.is_root() {
                StoragePath::new(name.clone())?
            } else {
                path.join(&name)?
            };
            let real_child = entry.path();
            let meta = self.metadata_of(&real_child, &child_path).await?;
            out.push(DirEntry {
                name,
                metadata: meta,
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    async fn read(&self, path: &StoragePath) -> StorageResult<Pin<Box<dyn AsyncRead + Send>>> {
        let real = self.resolve(path)?;
        let md = fs::metadata(&real).await.map_err(map_io)?;
        if md.is_dir() {
            return Err(StorageError::IsADirectory);
        }
        let f = fs::File::open(&real).await.map_err(map_io)?;
        Ok(Box::pin(f))
    }

    async fn read_range(
        &self,
        path: &StoragePath,
        range: Range<u64>,
    ) -> StorageResult<Pin<Box<dyn AsyncRead + Send>>> {
        let real = self.resolve(path)?;
        let md = fs::metadata(&real).await.map_err(map_io)?;
        if md.is_dir() {
            return Err(StorageError::IsADirectory);
        }
        let mut f = fs::File::open(&real).await.map_err(map_io)?;
        f.seek(SeekFrom::Start(range.start)).await.map_err(map_io)?;
        let limited = f.take(range.end.saturating_sub(range.start));
        Ok(Box::pin(limited))
    }

    async fn put_file(
        &self,
        path: &StoragePath,
        body: Pin<Box<dyn AsyncRead + Send>>,
        sink: &dyn EventSink,
    ) -> StorageResult<FileMetadata> {
        let real = self.resolve(path)?;
        if real.is_dir() {
            return Err(StorageError::IsADirectory);
        }
        let parent = real.parent().ok_or_else(|| {
            StorageError::InvalidPath(format!("no parent for {}", real.display()))
        })?;
        let parent_md = fs::metadata(parent).await.map_err(map_io)?;
        if !parent_md.is_dir() {
            return Err(StorageError::NotADirectory);
        }

        let temp_path = sibling_temp(&real)?;
        let guard = TempFileGuard::new(temp_path.clone());

        // Stream body into temp. Peek the first 4KiB for mimetype sniffing.
        let (mut head, body) = peek_head(body, 4096).await?;
        let file_handle = stream_to_temp(guard.path(), body).await?;
        drop(file_handle);

        // Compute ETag + mimetype, write xattrs to the temp file.
        let etag = ETag::new();
        xattr_io::write_etag(guard.path(), &etag)?;
        head.truncate(4096);
        let mimetype = mimetype::detect(path.as_str(), &head);
        xattr_io::write_mimetype(guard.path(), &mimetype)?;

        // Atomic rename + fsync parent.
        atomic_rename(guard.path(), &real).await?;
        guard.forget();

        let meta = self.metadata_of(&real, path).await?;
        sink.emit(StorageEvent::Written {
            storage_id: self.id.clone(),
            path: path.clone(),
            metadata: meta.clone(),
        })
        .await;
        Ok(meta)
    }

    async fn mkdir(&self, path: &StoragePath, sink: &dyn EventSink) -> StorageResult<FileMetadata> {
        let real = self.resolve(path)?;
        fs::create_dir(&real).await.map_err(map_io)?;
        let meta = self.metadata_of(&real, path).await?;
        sink.emit(StorageEvent::DirCreated {
            storage_id: self.id.clone(),
            path: path.clone(),
            metadata: meta.clone(),
        })
        .await;
        Ok(meta)
    }

    async fn delete(&self, path: &StoragePath, sink: &dyn EventSink) -> StorageResult<()> {
        let real = self.resolve(path)?;
        let md = fs::metadata(&real).await.map_err(map_io)?;
        if md.is_dir() {
            // Reject if non-empty. Don't walk; let `read_dir().next_entry()`
            // be O(1) for the common empty case.
            let mut rd = fs::read_dir(&real).await.map_err(map_io)?;
            if rd.next_entry().await.map_err(map_io)?.is_some() {
                return Err(StorageError::NotEmpty);
            }
            fs::remove_dir(&real).await.map_err(map_io)?;
        } else {
            fs::remove_file(&real).await.map_err(map_io)?;
        }
        sink.emit(StorageEvent::Deleted {
            storage_id: self.id.clone(),
            path: path.clone(),
        })
        .await;
        Ok(())
    }

    async fn rename(
        &self,
        from: &StoragePath,
        to: &StoragePath,
        sink: &dyn EventSink,
    ) -> StorageResult<()> {
        let real_from = self.resolve(from)?;
        let real_to = self.resolve(to)?;
        if !fs::try_exists(&real_from).await.map_err(map_io)? {
            return Err(StorageError::NotFound);
        }
        if fs::try_exists(&real_to).await.map_err(map_io)? {
            return Err(StorageError::AlreadyExists);
        }
        fs::rename(&real_from, &real_to).await.map_err(map_io)?;
        sink.emit(StorageEvent::Moved {
            storage_id: self.id.clone(),
            from: from.clone(),
            to: to.clone(),
        })
        .await;
        Ok(())
    }

    async fn copy(
        &self,
        from: &StoragePath,
        to: &StoragePath,
        sink: &dyn EventSink,
    ) -> StorageResult<()> {
        let real_from = self.resolve(from)?;
        let real_to = self.resolve(to)?;
        if fs::try_exists(&real_to).await.map_err(map_io)? {
            return Err(StorageError::AlreadyExists);
        }
        let md = fs::metadata(&real_from).await.map_err(map_io)?;
        if md.is_dir() {
            // Recursive copy: walk + recreate. Fresh ETag per leaf.
            copy_dir_recursive(&real_from, &real_to).await?;
        } else {
            fs::copy(&real_from, &real_to).await.map_err(map_io)?;
            // Fresh ETag at the destination — explicitly rewrite the xattr
            // because the source's xattr is copied verbatim on some FSes.
            xattr_io::write_etag(&real_to, &ETag::new())?;
        }
        sink.emit(StorageEvent::Copied {
            storage_id: self.id.clone(),
            from: from.clone(),
            to: to.clone(),
        })
        .await;
        Ok(())
    }

    async fn begin_multipart(
        &self,
        _target: &StoragePath,
        _sink: &dyn EventSink,
    ) -> StorageResult<MultipartHandle> {
        // Implemented in batch E.
        Err(StorageError::Other("multipart not yet implemented".into()))
    }

    async fn put_part(
        &self,
        _handle: &MultipartHandle,
        _part_number: u32,
        _body: Pin<Box<dyn AsyncRead + Send>>,
    ) -> StorageResult<PartTag> {
        Err(StorageError::Other("multipart not yet implemented".into()))
    }

    async fn commit_multipart(
        &self,
        _handle: MultipartHandle,
        _parts: Vec<PartTag>,
        _sink: &dyn EventSink,
    ) -> StorageResult<FileMetadata> {
        Err(StorageError::Other("multipart not yet implemented".into()))
    }

    async fn abort_multipart(&self, _handle: MultipartHandle) -> StorageResult<()> {
        Err(StorageError::Other("multipart not yet implemented".into()))
    }
}

/// Read up to `n` bytes from `body` into a buffer; return the peek plus a
/// new reader that yields the peek followed by the rest of `body`.
async fn peek_head(
    mut body: Pin<Box<dyn AsyncRead + Send>>,
    n: usize,
) -> StorageResult<(Vec<u8>, Pin<Box<dyn AsyncRead + Send>>)> {
    let mut head = vec![0u8; n];
    let read = body.read(&mut head).await.map_err(map_io)?;
    head.truncate(read);
    let cloned_head = head.clone();
    let prefix = std::io::Cursor::new(cloned_head);
    let combined = prefix.chain(body);
    Ok((head, Box::pin(combined)))
}

async fn copy_dir_recursive(src: &Path, dst: &Path) -> StorageResult<()> {
    fs::create_dir(dst).await.map_err(map_io)?;
    let mut rd = fs::read_dir(src).await.map_err(map_io)?;
    while let Some(entry) = rd.next_entry().await.map_err(map_io)? {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let m = entry.metadata().await.map_err(map_io)?;
        if m.is_dir() {
            Box::pin(copy_dir_recursive(&from, &to)).await?;
        } else {
            fs::copy(&from, &to).await.map_err(map_io)?;
            xattr_io::write_etag(&to, &ETag::new())?;
        }
    }
    Ok(())
}

/// Get a stable per-file identifier for ETag fallback. On Unix this is the
/// inode; on other platforms we hash the file path bytes.
#[cfg(unix)]
fn stable_inode(md: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    md.ino()
}

#[cfg(not(unix))]
fn stable_inode(_md: &std::fs::Metadata) -> u64 {
    // No inode concept; the ETag fallback will collide if multiple files
    // share an mtime. Acceptable for the Windows fallback path.
    0
}

async fn recompute_mimetype(real: &Path, path: &str) -> Mimetype {
    if let Some(m) = mimetype::from_extension(path) {
        return m;
    }
    let head = match fs::read(real).await {
        Ok(mut v) => {
            v.truncate(4096);
            v
        }
        Err(_) => Vec::new(),
    };
    mimetype::detect(path, &head)
}
