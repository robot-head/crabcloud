//! Memory-backend-specific tests: concurrent writes.

mod support;

// Anchor workspace-level dev/transitive deps so `-D warnings` +
// `unused_crate_dependencies` stays quiet for this integration-test binary.
use bytes as _;
use hex as _;
use infer as _;
use phf as _;
use rand as _;
use serde as _;
use sha2 as _;
use tempfile as _;
use thiserror as _;
use tracing as _;
// xattr is unix-only ([target.'cfg(unix)'.dependencies]); anchor it under the
// same cfg so Linux CI doesn't flag it as unused in this test binary while
// Windows-local builds don't see it at all.
#[cfg(unix)]
use xattr as _;

use crabcloud_storage::memory::MemoryStorage;
use crabcloud_storage::{NoopEventSink, Storage, StoragePath};
use std::sync::Arc;
use tokio::io::AsyncReadExt;

fn body(bytes: Vec<u8>) -> std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>> {
    Box::pin(std::io::Cursor::new(bytes))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_distinct_paths_all_succeed() {
    let storage = Arc::new(MemoryStorage::new("concurrent-distinct"));
    let mut handles = Vec::new();
    for i in 0..100u32 {
        let storage = storage.clone();
        handles.push(tokio::spawn(async move {
            let path = StoragePath::new(format!("f-{i:03}.txt")).unwrap();
            storage
                .put_file(&path, body(format!("v-{i}").into_bytes()), &NoopEventSink)
                .await
                .unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    let listing = storage.list(&StoragePath::root()).await.unwrap();
    assert_eq!(listing.len(), 100);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_same_path_last_writer_wins() {
    let storage = Arc::new(MemoryStorage::new("concurrent-same"));
    let path = StoragePath::new("contended.txt").unwrap();
    let mut handles = Vec::new();
    for i in 0..100u32 {
        let storage = storage.clone();
        let path = path.clone();
        handles.push(tokio::spawn(async move {
            storage
                .put_file(&path, body(format!("{i:03}").into_bytes()), &NoopEventSink)
                .await
                .unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    let mut reader = storage.read(&path).await.unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    let s = String::from_utf8(buf).unwrap();
    // Some value in 000..=099 won; we don't care which.
    assert_eq!(s.len(), 3);
    assert!(s.chars().all(|c| c.is_ascii_digit()));
}
