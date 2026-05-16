//! On-disk preview cache + per-key dedup lock.
//!
//! Cache layout:
//!   <preview_root>/<storage_id>/<fileid>/<size>-<source_etag>.jpg
//!
//! Reads check the exact path; writes atomically rename a tempfile into
//! place and then sweep any sibling files matching `<size>-*` that don't
//! match the current etag. The dedup [`DashMap`] ensures concurrent first-
//! request renders for the same `(storage_id, fileid, size)` share one
//! task.

use crate::error::PreviewError;
use crate::ladder::round_up_to_ladder;
use crate::provider::{PreviewProvider, ProviderResult};
use dashmap::DashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::OnceCell;

type DedupKey = (String, i64, u32);
type DedupCell = Arc<OnceCell<ProviderResult<PathBuf>>>;

pub struct PreviewCache {
    root: PathBuf,
    max_pixels: u32,
    locks: DashMap<DedupKey, DedupCell>,
}

impl PreviewCache {
    pub fn new(root: PathBuf, max_pixels: u32) -> Self {
        Self {
            root,
            max_pixels,
            locks: DashMap::new(),
        }
    }

    /// Returns a path to a JPEG containing the requested preview, along
    /// with the ladder rung that was actually used. If the cache file
    /// already exists, returns immediately without invoking `read_source`
    /// or `provider`. Otherwise reads the source via the caller-supplied
    /// closure (so the cache layer doesn't depend on `View`), dispatches
    /// to `provider`, and writes the result atomically.
    ///
    /// Many arguments are deliberate: a `PreviewRequest` struct would just
    /// shuffle the same fields one indirection deeper and the handler
    /// always knows all of them upfront.
    #[allow(clippy::too_many_arguments)]
    pub async fn get_or_render<F, Fut>(
        &self,
        storage_id: &str,
        fileid: i64,
        requested_size: u32,
        source_etag: &str,
        source_mime: &str,
        provider: &'static dyn PreviewProvider,
        read_source: F,
    ) -> Result<(PathBuf, u32), PreviewError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<u8>, PreviewError>>,
    {
        let _ = source_mime; // discriminator owned by the caller; we cache by id
        let size = round_up_to_ladder(requested_size)?;
        let cache_path = self.path_for(storage_id, fileid, size, source_etag);

        // Fast path: cache hit. Skip the dedup lock entirely.
        if tokio::fs::try_exists(&cache_path).await? {
            return Ok((cache_path, size));
        }

        // Slow path: per-key dedup lock. Concurrent first-request renders
        // share one task so we don't render the same preview N times.
        let key: DedupKey = (storage_id.to_string(), fileid, size);
        let cell: DedupCell = self
            .locks
            .entry(key.clone())
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone();

        // Wrap `read_source` so we only ever call it once (the winning
        // task). Followers see the cell already-initialized and don't
        // touch the closure.
        let mut reader = Some(read_source);
        let init_result = cell
            .get_or_init(|| async {
                let reader = reader.take().expect("get_or_init runs once");
                let bytes = reader().await?;
                self.render_and_write(storage_id, fileid, size, source_etag, provider, bytes)
                    .await
            })
            .await
            .clone();

        // Drop the dedup entry so subsequent (post-cache-miss) reads hit
        // the on-disk fast path with no map lookup.
        self.locks.remove(&key);

        let path = init_result?;
        Ok((path, size))
    }

    fn path_for(&self, storage_id: &str, fileid: i64, size: u32, etag: &str) -> PathBuf {
        let safe_storage = sanitize_path_component(storage_id);
        let safe_etag = sanitize_path_component(etag);
        self.root
            .join(safe_storage)
            .join(fileid.to_string())
            .join(format!("{size}-{safe_etag}.jpg"))
    }

    async fn render_and_write(
        &self,
        storage_id: &str,
        fileid: i64,
        size: u32,
        etag: &str,
        provider: &'static dyn PreviewProvider,
        source_bytes: Vec<u8>,
    ) -> ProviderResult<PathBuf> {
        let cache_path = self.path_for(storage_id, fileid, size, etag);
        if let Some(parent) = cache_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let jpeg = provider.render(source_bytes, size, self.max_pixels).await?;
        // Atomic write: tempfile in the same directory + rename. Use a
        // random suffix to avoid collisions between concurrent renders of
        // different keys writing into the same directory.
        let tmp = cache_path.with_extension(format!("jpg.tmp.{}", std::process::id()));
        tokio::fs::write(&tmp, &jpeg).await?;
        tokio::fs::rename(&tmp, &cache_path).await?;
        // Best-effort: sweep stale siblings (same fileid+size, different etag).
        if let Some(parent) = cache_path.parent() {
            let _ = self.sweep_stale_siblings(parent, size, etag).await;
        }
        Ok(cache_path)
    }

    async fn sweep_stale_siblings(
        &self,
        dir: &Path,
        size: u32,
        keep_etag: &str,
    ) -> std::io::Result<()> {
        let prefix = format!("{size}-");
        let keep_name = format!("{}{}.jpg", prefix, sanitize_path_component(keep_etag));
        let mut entries = tokio::fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name();
            let name_str = match name.to_str() {
                Some(s) => s.to_string(),
                None => continue,
            };
            if !name_str.starts_with(&prefix) || !name_str.ends_with(".jpg") {
                continue;
            }
            if name_str == keep_name {
                continue;
            }
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
        Ok(())
    }
}

/// Strip characters that would be hostile to a path component. Storage ids
/// in Crabcloud are ASCII-restricted (per `oc_storages`) and ETags from
/// `crabcloud-storage` are 40 lowercase-hex chars (per SP6), but defensive
/// sanitization costs nothing.
fn sanitize_path_component(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ImageProvider;
    use image::ImageFormat;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    fn synth_jpeg() -> Vec<u8> {
        let img = image::RgbImage::from_fn(100, 80, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        });
        let mut buf = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut buf, ImageFormat::Jpeg)
            .unwrap();
        buf.into_inner()
    }

    #[tokio::test]
    async fn cache_miss_renders_and_writes_then_hits() {
        let tmp = TempDir::new().unwrap();
        let cache = PreviewCache::new(tmp.path().to_path_buf(), 64 * 1024 * 1024);
        let bytes = synth_jpeg();

        let (p1, size1) = cache
            .get_or_render(
                "sid",
                7,
                64,
                "abc123def456",
                "image/jpeg",
                &ImageProvider,
                || async { Ok(bytes.clone()) },
            )
            .await
            .unwrap();
        assert_eq!(size1, 64);
        assert!(p1.exists());

        // Second call MUST NOT invoke read_source (we'd panic).
        let (p2, _) = cache
            .get_or_render(
                "sid",
                7,
                64,
                "abc123def456",
                "image/jpeg",
                &ImageProvider,
                || async { panic!("read_source called on cache hit") },
            )
            .await
            .unwrap();
        assert_eq!(p1, p2);
    }

    #[tokio::test]
    async fn cache_rounds_up_to_ladder() {
        let tmp = TempDir::new().unwrap();
        let cache = PreviewCache::new(tmp.path().to_path_buf(), 64 * 1024 * 1024);
        let bytes = synth_jpeg();
        let (path, size) = cache
            .get_or_render(
                "sid",
                1,
                200,
                "etag1234567890aa",
                "image/jpeg",
                &ImageProvider,
                || async { Ok(bytes.clone()) },
            )
            .await
            .unwrap();
        assert_eq!(size, 256);
        assert!(path.to_string_lossy().contains("256-etag1234567890aa.jpg"));
    }

    #[tokio::test]
    async fn cache_rejects_oversize_request() {
        let tmp = TempDir::new().unwrap();
        let cache = PreviewCache::new(tmp.path().to_path_buf(), 64 * 1024 * 1024);
        let r = cache
            .get_or_render(
                "sid",
                1,
                4096,
                "etag",
                "image/jpeg",
                &ImageProvider,
                || async { Ok(vec![]) },
            )
            .await;
        assert!(matches!(r, Err(PreviewError::SizeOutOfRange(4096))));
    }

    #[tokio::test]
    async fn cache_sweeps_stale_etag_siblings() {
        let tmp = TempDir::new().unwrap();
        let cache = PreviewCache::new(tmp.path().to_path_buf(), 64 * 1024 * 1024);
        // Seed an old preview file directly.
        let dir = tmp.path().join("sid").join("3");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let stale = dir.join("64-oldetag.jpg");
        tokio::fs::write(&stale, b"old").await.unwrap();

        let bytes = synth_jpeg();
        let (fresh_path, _) = cache
            .get_or_render(
                "sid",
                3,
                64,
                "newetag",
                "image/jpeg",
                &ImageProvider,
                || async { Ok(bytes.clone()) },
            )
            .await
            .unwrap();
        assert!(fresh_path.exists());
        assert!(
            !stale.exists(),
            "stale sibling must be deleted after fresh render"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn dedup_lock_serializes_concurrent_renders() {
        let tmp = TempDir::new().unwrap();
        let cache = Arc::new(PreviewCache::new(
            tmp.path().to_path_buf(),
            64 * 1024 * 1024,
        ));
        let counter = Arc::new(AtomicUsize::new(0));

        // Spawn N concurrent renders of the same (sid, fileid, size).
        let mut tasks = Vec::new();
        for _ in 0..10 {
            let cache = cache.clone();
            let counter = counter.clone();
            let bytes = synth_jpeg();
            tasks.push(tokio::spawn(async move {
                cache
                    .get_or_render(
                        "sid",
                        9,
                        64,
                        "concurrentetag123",
                        "image/jpeg",
                        &ImageProvider,
                        || async {
                            counter.fetch_add(1, Ordering::SeqCst);
                            // Slow the winner just enough that followers
                            // pile up on the OnceCell rather than racing
                            // through the cache-hit fast path.
                            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                            Ok(bytes)
                        },
                    )
                    .await
                    .unwrap();
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }
        // Only the winning task actually called read_source.
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "dedup must collapse concurrent reads"
        );
    }
}
