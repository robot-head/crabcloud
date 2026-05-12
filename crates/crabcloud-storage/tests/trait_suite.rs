//! Parametrized trait suite. Both backends (MemoryStorage in Batch C,
//! LocalStorage in Batch D) invoke `run_storage_suite` via their own
//! top-level test functions.
//!
//! Adding a backend in this crate? Add a top-level `#[tokio::test]` that
//! calls `run_storage_suite("backend_name", || your_factory()).await`.

mod support;

// Workspace lints set `unused_crate_dependencies = warn`, and `cargo xtask
// check-all` runs clippy with `-D warnings`. Each integration-test binary is
// its own crate, so dev-deps + transitive workspace deps (used by the
// library) appear unused here. Anchor them so the test binary compiles
// clean. Later batches will reuse some of these (e.g. `tempfile` in Batch D).
use bytes as _;
use hex as _;
use infer as _;
use phf as _;
use rand as _;
use tempfile as _;
use thiserror as _;
use tracing as _;

use crabcloud_storage::{DirEntry, FileKind, NoopEventSink, Storage, StorageError, StoragePath};
use std::sync::Arc;
use support::RecordingSink;
use tokio::io::AsyncReadExt;

/// Drive the full battery of trait-level assertions against `factory()`,
/// which must produce a fresh, empty storage on each call.
pub async fn run_storage_suite<S: Storage + 'static>(
    name: &str,
    factory: impl Fn() -> S + Send + Sync,
) {
    eprintln!("--- storage suite: {name} ---");

    path_invariants();
    write_then_read(&factory).await;
    write_overwrite_changes_etag(&factory).await;
    stat_after_write(&factory).await;
    read_range_returns_slice(&factory).await;
    mkdir_then_list_includes_dir(&factory).await;
    write_to_dir_lists_correctly(&factory).await;
    delete_file_then_stat_404(&factory).await;
    delete_empty_dir_ok_nonempty_errs(&factory).await;
    rename_moves(&factory).await;
    copy_preserves_contents_changes_etag(&factory).await;
    multipart_happy_path(&factory).await;
    multipart_abort_drops_target(&factory).await;
    multipart_gap_rejected(&factory).await;
    multipart_duplicate_rejected(&factory).await;
    event_sink_emits_one_per_mutation(&factory).await;
}

// --- individual assertions (each is a pure async fn against a fresh storage) ---

fn path_invariants() {
    // These don't depend on the backend; assert constructor behavior here
    // so `StoragePath` is sanity-checked at the integration boundary too.
    assert!(StoragePath::new("").is_ok());
    assert!(matches!(
        StoragePath::new("/abs"),
        Err(StorageError::InvalidPath(_))
    ));
    assert!(matches!(
        StoragePath::new("a/../b"),
        Err(StorageError::InvalidPath(_))
    ));
    assert!(matches!(
        StoragePath::new("a/./b"),
        Err(StorageError::InvalidPath(_))
    ));
    assert!(matches!(
        StoragePath::new("a\0b"),
        Err(StorageError::InvalidPath(_))
    ));
}

async fn write_then_read<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let path = StoragePath::new("hello.txt").unwrap();
    let body = make_body(b"hi");
    let sink = NoopEventSink;
    let meta = storage.put_file(&path, body, &sink).await.unwrap();
    assert_eq!(meta.kind, FileKind::File);
    assert_eq!(meta.size, 2);
    let mut reader = storage.read(&path).await.unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"hi");
}

async fn write_overwrite_changes_etag<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let path = StoragePath::new("over.txt").unwrap();
    let sink = NoopEventSink;
    let a = storage
        .put_file(&path, make_body(b"v1"), &sink)
        .await
        .unwrap();
    let b = storage
        .put_file(&path, make_body(b"v2"), &sink)
        .await
        .unwrap();
    assert_ne!(a.etag, b.etag);
}

async fn stat_after_write<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let path = StoragePath::new("stat.txt").unwrap();
    let sink = NoopEventSink;
    storage
        .put_file(&path, make_body(b"data"), &sink)
        .await
        .unwrap();
    let meta = storage.stat(&path).await.unwrap();
    assert_eq!(meta.size, 4);
    assert_eq!(meta.kind, FileKind::File);
    assert_eq!(meta.etag.as_str().len(), 40);
}

async fn read_range_returns_slice<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let path = StoragePath::new("range.txt").unwrap();
    let sink = NoopEventSink;
    storage
        .put_file(&path, make_body(b"abcdefghij"), &sink)
        .await
        .unwrap();
    let mut reader = storage.read_range(&path, 2..5).await.unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"cde");
}

async fn mkdir_then_list_includes_dir<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let dir = StoragePath::new("d1").unwrap();
    let sink = NoopEventSink;
    storage.mkdir(&dir, &sink).await.unwrap();
    let listing = storage.list(&StoragePath::root()).await.unwrap();
    let found = listing
        .iter()
        .find(|e: &&DirEntry| e.name == "d1")
        .expect("d1 in root listing");
    assert_eq!(found.metadata.kind, FileKind::Directory);
}

async fn write_to_dir_lists_correctly<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let sink = NoopEventSink;
    storage
        .mkdir(&StoragePath::new("d").unwrap(), &sink)
        .await
        .unwrap();
    storage
        .put_file(
            &StoragePath::new("d/x.txt").unwrap(),
            make_body(b"x"),
            &sink,
        )
        .await
        .unwrap();
    let listing = storage.list(&StoragePath::new("d").unwrap()).await.unwrap();
    assert_eq!(listing.len(), 1);
    assert_eq!(listing[0].name, "x.txt");
}

async fn delete_file_then_stat_404<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let path = StoragePath::new("del.txt").unwrap();
    let sink = NoopEventSink;
    storage
        .put_file(&path, make_body(b"x"), &sink)
        .await
        .unwrap();
    storage.delete(&path, &sink).await.unwrap();
    assert!(matches!(
        storage.stat(&path).await,
        Err(StorageError::NotFound)
    ));
}

async fn delete_empty_dir_ok_nonempty_errs<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let sink = NoopEventSink;
    storage
        .mkdir(&StoragePath::new("empty").unwrap(), &sink)
        .await
        .unwrap();
    storage
        .delete(&StoragePath::new("empty").unwrap(), &sink)
        .await
        .unwrap();

    storage
        .mkdir(&StoragePath::new("full").unwrap(), &sink)
        .await
        .unwrap();
    storage
        .put_file(
            &StoragePath::new("full/x.txt").unwrap(),
            make_body(b"x"),
            &sink,
        )
        .await
        .unwrap();
    assert!(matches!(
        storage
            .delete(&StoragePath::new("full").unwrap(), &sink)
            .await,
        Err(StorageError::NotEmpty)
    ));
}

async fn rename_moves<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let from = StoragePath::new("from.txt").unwrap();
    let to = StoragePath::new("to.txt").unwrap();
    let sink = NoopEventSink;
    storage
        .put_file(&from, make_body(b"x"), &sink)
        .await
        .unwrap();
    storage.rename(&from, &to, &sink).await.unwrap();
    assert!(matches!(
        storage.stat(&from).await,
        Err(StorageError::NotFound)
    ));
    let mut buf = Vec::new();
    storage
        .read(&to)
        .await
        .unwrap()
        .read_to_end(&mut buf)
        .await
        .unwrap();
    assert_eq!(buf, b"x");
}

async fn copy_preserves_contents_changes_etag<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let src = StoragePath::new("src.txt").unwrap();
    let dst = StoragePath::new("dst.txt").unwrap();
    let sink = NoopEventSink;
    let a = storage
        .put_file(&src, make_body(b"copy-me"), &sink)
        .await
        .unwrap();
    storage.copy(&src, &dst, &sink).await.unwrap();
    let b = storage.stat(&dst).await.unwrap();
    assert_ne!(a.etag, b.etag);
    let mut buf = Vec::new();
    storage
        .read(&dst)
        .await
        .unwrap()
        .read_to_end(&mut buf)
        .await
        .unwrap();
    assert_eq!(buf, b"copy-me");
}

async fn multipart_happy_path<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let target = StoragePath::new("big.bin").unwrap();
    let sink = NoopEventSink;
    let handle = storage.begin_multipart(&target, &sink).await.unwrap();
    let t1 = storage
        .put_part(&handle, 1, make_body(b"AAA"))
        .await
        .unwrap();
    let t2 = storage
        .put_part(&handle, 2, make_body(b"BBB"))
        .await
        .unwrap();
    storage
        .commit_multipart(handle, vec![t1, t2], &sink)
        .await
        .unwrap();
    let mut buf = Vec::new();
    storage
        .read(&target)
        .await
        .unwrap()
        .read_to_end(&mut buf)
        .await
        .unwrap();
    assert_eq!(buf, b"AAABBB");
}

async fn multipart_abort_drops_target<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let target = StoragePath::new("aborted.bin").unwrap();
    let sink = NoopEventSink;
    let handle = storage.begin_multipart(&target, &sink).await.unwrap();
    storage
        .put_part(&handle, 1, make_body(b"AAA"))
        .await
        .unwrap();
    storage.abort_multipart(handle).await.unwrap();
    assert!(matches!(
        storage.stat(&target).await,
        Err(StorageError::NotFound)
    ));
}

async fn multipart_gap_rejected<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let target = StoragePath::new("gap.bin").unwrap();
    let sink = NoopEventSink;
    let handle = storage.begin_multipart(&target, &sink).await.unwrap();
    let t1 = storage
        .put_part(&handle, 1, make_body(b"AAA"))
        .await
        .unwrap();
    let t3 = storage
        .put_part(&handle, 3, make_body(b"CCC"))
        .await
        .unwrap();
    let err = storage
        .commit_multipart(handle, vec![t1, t3], &sink)
        .await
        .unwrap_err();
    assert!(matches!(err, StorageError::Multipart(_)));
}

async fn multipart_duplicate_rejected<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let target = StoragePath::new("dup.bin").unwrap();
    let sink = NoopEventSink;
    let handle = storage.begin_multipart(&target, &sink).await.unwrap();
    let t1a = storage
        .put_part(&handle, 1, make_body(b"AAA"))
        .await
        .unwrap();
    let t1b = storage
        .put_part(&handle, 1, make_body(b"BBB"))
        .await
        .unwrap();
    let err = storage
        .commit_multipart(handle, vec![t1a, t1b], &sink)
        .await
        .unwrap_err();
    assert!(matches!(err, StorageError::Multipart(_)));
}

async fn event_sink_emits_one_per_mutation<S: Storage>(factory: &impl Fn() -> S) {
    let storage = factory();
    let sink = RecordingSink::new();
    storage
        .put_file(&StoragePath::new("a").unwrap(), make_body(b"x"), &sink)
        .await
        .unwrap();
    storage
        .mkdir(&StoragePath::new("d").unwrap(), &sink)
        .await
        .unwrap();
    storage
        .rename(
            &StoragePath::new("a").unwrap(),
            &StoragePath::new("b").unwrap(),
            &sink,
        )
        .await
        .unwrap();
    storage
        .copy(
            &StoragePath::new("b").unwrap(),
            &StoragePath::new("c").unwrap(),
            &sink,
        )
        .await
        .unwrap();
    storage
        .delete(&StoragePath::new("c").unwrap(), &sink)
        .await
        .unwrap();
    let events = sink.snapshot();
    assert_eq!(events.len(), 5);
    let _ = Arc::new(events); // keep variable used; aids future inspection
}

// --- helpers ---

fn make_body(bytes: &'static [u8]) -> std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>> {
    Box::pin(tokio::io::BufReader::new(bytes))
}
