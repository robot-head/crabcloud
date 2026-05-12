//! In-memory backend. `Arc<RwLock<MemTree>>` around a `BTreeMap` keyed by
//! [`StoragePath`]. Coarse but adequate for test + dev workloads.
//!
//! Multipart uploads buffer parts in a per-handle `Mutex<BTreeMap<u32, Bytes>>`
//! stored in a sibling `BTreeMap` keyed by upload id.

use crate::error::{StorageError, StorageResult};
use crate::meta::{
    DirEntry, ETag, FileKind, FileMetadata, Mimetype, MultipartHandle, PartTag, Permissions,
};
use crate::path::StoragePath;
use crate::{EventSink, Storage, StorageEvent};
use async_trait::async_trait;
use bytes::Bytes;
use rand::RngExt;
use std::collections::BTreeMap;
use std::ops::Range;
use std::pin::Pin;
use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;
use tokio::io::{AsyncRead, AsyncReadExt};

#[derive(Clone)]
enum MemEntry {
    File {
        bytes: Bytes,
        etag: ETag,
        mtime: SystemTime,
        mimetype: Mimetype,
    },
    Directory {
        etag: ETag,
        mtime: SystemTime,
    },
}

impl MemEntry {
    fn to_metadata(&self, path: StoragePath) -> FileMetadata {
        match self {
            MemEntry::File {
                bytes,
                etag,
                mtime,
                mimetype,
            } => FileMetadata {
                path,
                kind: FileKind::File,
                size: bytes.len() as u64,
                mtime: *mtime,
                etag: etag.clone(),
                mimetype: mimetype.clone(),
                permissions: Permissions::full(),
            },
            MemEntry::Directory { etag, mtime } => FileMetadata {
                path,
                kind: FileKind::Directory,
                size: 0,
                mtime: *mtime,
                etag: etag.clone(),
                mimetype: Mimetype::octet_stream(),
                permissions: Permissions::full(),
            },
        }
    }
}

#[derive(Default)]
struct MemTree {
    entries: BTreeMap<StoragePath, MemEntry>,
    /// Upload-id → ordered map of part-number → bytes. Each insertion replaces
    /// any existing key, so `put_part(n, …)` overwrites.
    uploads: BTreeMap<String, Arc<Mutex<BTreeMap<u32, Bytes>>>>,
}

pub struct MemoryStorage {
    id: String,
    inner: Arc<RwLock<MemTree>>,
}

impl MemoryStorage {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: format!("memory::{}", id.into()),
            inner: Arc::new(RwLock::new(MemTree::default())),
        }
    }

    fn ensure_parents(tree: &mut MemTree, path: &StoragePath) -> StorageResult<()> {
        // Implicit-mkdir: traverse path segments, materialize each ancestor as
        // a Directory entry if absent. Documented asymmetry vs LocalStorage.
        let segs: Vec<&str> = path.as_str().split('/').collect();
        if segs.len() <= 1 {
            return Ok(());
        }
        let mut cur = String::new();
        for seg in &segs[..segs.len() - 1] {
            if !cur.is_empty() {
                cur.push('/');
            }
            cur.push_str(seg);
            let p = StoragePath::new(cur.clone())?;
            match tree.entries.get(&p) {
                Some(MemEntry::File { .. }) => return Err(StorageError::NotADirectory),
                Some(MemEntry::Directory { .. }) => {}
                None => {
                    tree.entries.insert(
                        p,
                        MemEntry::Directory {
                            etag: ETag::new(),
                            mtime: SystemTime::now(),
                        },
                    );
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Storage for MemoryStorage {
    fn id(&self) -> &str {
        &self.id
    }

    async fn stat(&self, path: &StoragePath) -> StorageResult<FileMetadata> {
        let tree = self.inner.read().unwrap();
        tree.entries
            .get(path)
            .map(|e| e.to_metadata(path.clone()))
            .ok_or(StorageError::NotFound)
    }

    async fn exists(&self, path: &StoragePath) -> StorageResult<bool> {
        Ok(self.inner.read().unwrap().entries.contains_key(path))
    }

    async fn list(&self, path: &StoragePath) -> StorageResult<Vec<DirEntry>> {
        let tree = self.inner.read().unwrap();
        if !path.is_root() {
            match tree.entries.get(path) {
                Some(MemEntry::Directory { .. }) => {}
                Some(MemEntry::File { .. }) => return Err(StorageError::NotADirectory),
                None => return Err(StorageError::NotFound),
            }
        }
        let prefix = if path.is_root() {
            String::new()
        } else {
            format!("{}/", path.as_str())
        };
        let mut out = Vec::new();
        for (k, v) in &tree.entries {
            let s = k.as_str();
            if !s.starts_with(&prefix) {
                continue;
            }
            let rest = &s[prefix.len()..];
            if rest.is_empty() || rest.contains('/') {
                // Either the dir itself, or a deeper grandchild.
                continue;
            }
            out.push(DirEntry {
                name: rest.to_string(),
                metadata: v.to_metadata(k.clone()),
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    async fn read(&self, path: &StoragePath) -> StorageResult<Pin<Box<dyn AsyncRead + Send>>> {
        let tree = self.inner.read().unwrap();
        match tree.entries.get(path) {
            Some(MemEntry::File { bytes, .. }) => {
                let buf = bytes.clone();
                Ok(Box::pin(std::io::Cursor::new(buf.to_vec())) as Pin<Box<dyn AsyncRead + Send>>)
            }
            Some(MemEntry::Directory { .. }) => Err(StorageError::IsADirectory),
            None => Err(StorageError::NotFound),
        }
    }

    async fn read_range(
        &self,
        path: &StoragePath,
        range: Range<u64>,
    ) -> StorageResult<Pin<Box<dyn AsyncRead + Send>>> {
        let tree = self.inner.read().unwrap();
        match tree.entries.get(path) {
            Some(MemEntry::File { bytes, .. }) => {
                let start = range.start as usize;
                let end = (range.end as usize).min(bytes.len());
                let slice = bytes.slice(start..end);
                Ok(Box::pin(std::io::Cursor::new(slice.to_vec()))
                    as Pin<Box<dyn AsyncRead + Send>>)
            }
            Some(MemEntry::Directory { .. }) => Err(StorageError::IsADirectory),
            None => Err(StorageError::NotFound),
        }
    }

    async fn put_file(
        &self,
        path: &StoragePath,
        mut body: Pin<Box<dyn AsyncRead + Send>>,
        sink: &dyn EventSink,
    ) -> StorageResult<FileMetadata> {
        let mut buf = Vec::new();
        body.read_to_end(&mut buf).await?;
        let bytes = Bytes::from(buf);
        let mimetype = sniff_mimetype(path.as_str(), &bytes);

        let metadata: FileMetadata;
        {
            let mut tree = self.inner.write().unwrap();
            if let Some(MemEntry::Directory { .. }) = tree.entries.get(path) {
                return Err(StorageError::IsADirectory);
            }
            Self::ensure_parents(&mut tree, path)?;
            let entry = MemEntry::File {
                bytes: bytes.clone(),
                etag: ETag::new(),
                mtime: SystemTime::now(),
                mimetype,
            };
            metadata = entry.to_metadata(path.clone());
            tree.entries.insert(path.clone(), entry);
        }

        sink.emit(StorageEvent::Written {
            storage_id: self.id.clone(),
            path: path.clone(),
            metadata: metadata.clone(),
        })
        .await;
        Ok(metadata)
    }

    async fn mkdir(&self, path: &StoragePath, sink: &dyn EventSink) -> StorageResult<FileMetadata> {
        let metadata: FileMetadata;
        {
            let mut tree = self.inner.write().unwrap();
            if tree.entries.contains_key(path) {
                return Err(StorageError::AlreadyExists);
            }
            Self::ensure_parents(&mut tree, path)?;
            let entry = MemEntry::Directory {
                etag: ETag::new(),
                mtime: SystemTime::now(),
            };
            metadata = entry.to_metadata(path.clone());
            tree.entries.insert(path.clone(), entry);
        }
        sink.emit(StorageEvent::DirCreated {
            storage_id: self.id.clone(),
            path: path.clone(),
            metadata: metadata.clone(),
        })
        .await;
        Ok(metadata)
    }

    async fn delete(&self, path: &StoragePath, sink: &dyn EventSink) -> StorageResult<()> {
        {
            let mut tree = self.inner.write().unwrap();
            let entry = tree
                .entries
                .get(path)
                .ok_or(StorageError::NotFound)?
                .clone();
            if let MemEntry::Directory { .. } = entry {
                let prefix = format!("{}/", path.as_str());
                let has_children = tree.entries.keys().any(|k| k.as_str().starts_with(&prefix));
                if has_children {
                    return Err(StorageError::NotEmpty);
                }
            }
            tree.entries.remove(path);
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
        {
            let mut tree = self.inner.write().unwrap();
            let entry = tree.entries.remove(from).ok_or(StorageError::NotFound)?;
            if tree.entries.contains_key(to) {
                // Restore original
                tree.entries.insert(from.clone(), entry);
                return Err(StorageError::AlreadyExists);
            }
            Self::ensure_parents(&mut tree, to)?;
            tree.entries.insert(to.clone(), entry);
        }
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
        {
            let mut tree = self.inner.write().unwrap();
            let entry = tree
                .entries
                .get(from)
                .ok_or(StorageError::NotFound)?
                .clone();
            if tree.entries.contains_key(to) {
                return Err(StorageError::AlreadyExists);
            }
            Self::ensure_parents(&mut tree, to)?;
            let new_entry = match entry {
                MemEntry::File {
                    bytes, mimetype, ..
                } => MemEntry::File {
                    bytes,
                    etag: ETag::new(),
                    mtime: SystemTime::now(),
                    mimetype,
                },
                MemEntry::Directory { .. } => MemEntry::Directory {
                    etag: ETag::new(),
                    mtime: SystemTime::now(),
                },
            };
            tree.entries.insert(to.clone(), new_entry);
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
        target: &StoragePath,
        _sink: &dyn EventSink,
    ) -> StorageResult<MultipartHandle> {
        let mut id_bytes = [0u8; 16];
        rand::rng().fill(&mut id_bytes);
        let upload_id = format!("mem-mp-{}", hex::encode(id_bytes));
        let mut tree = self.inner.write().unwrap();
        tree.uploads
            .insert(upload_id.clone(), Arc::new(Mutex::new(BTreeMap::new())));
        Ok(MultipartHandle {
            upload_id,
            target: target.clone(),
        })
    }

    async fn put_part(
        &self,
        handle: &MultipartHandle,
        part_number: u32,
        mut body: Pin<Box<dyn AsyncRead + Send>>,
    ) -> StorageResult<PartTag> {
        let mut buf = Vec::new();
        body.read_to_end(&mut buf).await?;
        let bytes = Bytes::from(buf);
        let etag = hex::encode({
            use std::collections::hash_map::DefaultHasher;
            use std::hash::Hasher;
            let mut h = DefaultHasher::new();
            h.write(&bytes);
            h.write_u32(part_number);
            h.finish().to_le_bytes()
        });
        let parts = {
            let tree = self.inner.read().unwrap();
            tree.uploads
                .get(&handle.upload_id)
                .cloned()
                .ok_or_else(|| StorageError::Multipart("unknown upload id".into()))?
        };
        parts.lock().unwrap().insert(part_number, bytes);
        Ok(PartTag { part_number, etag })
    }

    async fn commit_multipart(
        &self,
        handle: MultipartHandle,
        parts: Vec<PartTag>,
        sink: &dyn EventSink,
    ) -> StorageResult<FileMetadata> {
        if parts.is_empty() {
            return Err(StorageError::Multipart("no parts".into()));
        }
        // Validate contiguous, starts at 1, no duplicates.
        let mut nums: Vec<u32> = parts.iter().map(|p| p.part_number).collect();
        nums.sort_unstable();
        for (i, n) in nums.iter().enumerate() {
            if (*n as usize) != i + 1 {
                return Err(StorageError::Multipart(format!(
                    "expected contiguous parts starting at 1; got {n} at index {i}"
                )));
            }
        }
        let mut prev = 0u32;
        for n in &nums {
            if *n == prev {
                return Err(StorageError::Multipart(format!("duplicate part {n}")));
            }
            prev = *n;
        }

        let upload = {
            let mut tree = self.inner.write().unwrap();
            tree.uploads
                .remove(&handle.upload_id)
                .ok_or_else(|| StorageError::Multipart("unknown upload id".into()))?
        };
        let buf = {
            let map = upload.lock().unwrap();
            let mut buf = Vec::new();
            for tag in &parts {
                let part_bytes = map.get(&tag.part_number).ok_or_else(|| {
                    StorageError::Multipart(format!("missing part {}", tag.part_number))
                })?;
                buf.extend_from_slice(part_bytes);
            }
            buf
        };
        let bytes = Bytes::from(buf);
        let mimetype = sniff_mimetype(handle.target.as_str(), &bytes);

        let metadata: FileMetadata;
        {
            let mut tree = self.inner.write().unwrap();
            Self::ensure_parents(&mut tree, &handle.target)?;
            let entry = MemEntry::File {
                bytes: bytes.clone(),
                etag: ETag::new(),
                mtime: SystemTime::now(),
                mimetype,
            };
            metadata = entry.to_metadata(handle.target.clone());
            tree.entries.insert(handle.target.clone(), entry);
        }
        sink.emit(StorageEvent::Written {
            storage_id: self.id.clone(),
            path: handle.target.clone(),
            metadata: metadata.clone(),
        })
        .await;
        Ok(metadata)
    }

    async fn abort_multipart(&self, handle: MultipartHandle) -> StorageResult<()> {
        let mut tree = self.inner.write().unwrap();
        tree.uploads.remove(&handle.upload_id);
        Ok(())
    }
}

/// Mini extension-or-octet-stream mimetype guesser for the memory backend.
/// Used by the trait-suite tests; doesn't go through the phf table (the
/// Local backend handles that more thoroughly in batch D).
fn sniff_mimetype(path: &str, _bytes: &[u8]) -> Mimetype {
    if let Some(idx) = path.rfind('.') {
        let ext = &path[idx + 1..].to_ascii_lowercase();
        if ext == "txt" {
            return Mimetype::parse("text/plain").unwrap();
        }
    }
    Mimetype::octet_stream()
}
