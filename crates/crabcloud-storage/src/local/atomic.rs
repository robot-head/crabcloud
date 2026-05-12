//! Atomic-write sequence for `put_file` and `commit_multipart`. Stream to a
//! sibling temp file, fsync, rename, fsync parent dir. Temp file is cleaned
//! up on Drop if the rename hasn't fired.

use crate::error::{map_io, StorageError, StorageResult};
use rand::RngExt;
use std::path::{Path, PathBuf};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

/// RAII guard that removes the temp file on drop unless `forget()` is called.
pub struct TempFileGuard {
    path: Option<PathBuf>,
}

impl TempFileGuard {
    pub fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    pub fn path(&self) -> &Path {
        self.path.as_ref().expect("guard already consumed")
    }

    pub fn forget(mut self) {
        self.path.take();
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if let Some(p) = self.path.take() {
            let _ = std::fs::remove_file(&p);
        }
    }
}

/// Make a sibling temp path under the same directory as `target`.
pub fn sibling_temp(target: &Path) -> StorageResult<PathBuf> {
    let parent = target
        .parent()
        .ok_or_else(|| StorageError::InvalidPath(format!("no parent for {}", target.display())))?;
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    Ok(parent.join(format!(".tmp-crabcloud-{}", hex::encode(bytes))))
}

/// fsync a file handle.
pub async fn fsync_file(f: &File) -> StorageResult<()> {
    f.sync_all().await.map_err(map_io)?;
    Ok(())
}

/// fsync a directory (POSIX only — no-op on Windows).
pub async fn fsync_dir(dir: &Path) -> StorageResult<()> {
    #[cfg(unix)]
    {
        let f = tokio::fs::File::open(dir).await.map_err(map_io)?;
        f.sync_all().await.map_err(map_io)?;
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
    Ok(())
}

/// Atomic-rename a file: rename + fsync parent.
pub async fn atomic_rename(from: &Path, to: &Path) -> StorageResult<()> {
    tokio::fs::rename(from, to).await.map_err(map_io)?;
    if let Some(parent) = to.parent() {
        fsync_dir(parent).await?;
    }
    Ok(())
}

/// Stream `body` into a fresh temp file at `temp_path`, fsync, return the
/// open file handle (for callers that want to set xattrs before rename).
pub async fn stream_to_temp(
    temp_path: &Path,
    mut body: std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>>,
) -> StorageResult<File> {
    use tokio::io::AsyncReadExt;
    let mut f = File::create(temp_path).await.map_err(map_io)?;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = body.read(&mut buf).await.map_err(map_io)?;
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n]).await.map_err(map_io)?;
    }
    fsync_file(&f).await?;
    Ok(f)
}
