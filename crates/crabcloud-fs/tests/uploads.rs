mod support;

use crabcloud_fs::{FsError, Mount, Uploads, UserPath};
use crabcloud_storage::StoragePath;
use crabcloud_users::UserId;
use support::harness;
use tokio::io::AsyncReadExt;

// Silence unused-crate-dependencies for deps the lib uses but this
// integration-test target doesn't reference directly.
use async_trait as _;
use base64 as _;
use crabcloud_cache as _;
use crabcloud_core as _;
use thiserror as _;
use tracing as _;

fn body(bytes: Vec<u8>) -> std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>> {
    Box::pin(std::io::Cursor::new(bytes))
}

fn uploads_home(h: &support::Harness) -> Uploads {
    Uploads::new(
        UserId::new("alice").unwrap(),
        vec![Mount {
            path_prefix: StoragePath::root(),
            storage: h.storage.clone(),
            metadata: None,
        }],
        h.sink.clone(),
        h.filecache.clone(),
    )
}

#[tokio::test]
async fn uploads_begin_put_commit_roundtrip() {
    let h = harness().await;
    let u = uploads_home(&h);

    let dest = UserPath::new("/big.bin").unwrap();
    let handle = u.begin(&dest).await.unwrap();
    assert!(!handle.upload_id.is_empty());

    let t1 = u
        .put_part(&handle.upload_id, 1, body(b"AAA".to_vec()))
        .await
        .unwrap();
    let t2 = u
        .put_part(&handle.upload_id, 2, body(b"BBB".to_vec()))
        .await
        .unwrap();

    let meta = u
        .commit(&handle.upload_id, &dest, vec![t1, t2])
        .await
        .unwrap();
    assert_eq!(meta.size, 6);

    // Read assembled bytes back through the storage directly.
    let mut reader = h
        .storage
        .read(&StoragePath::new("big.bin").unwrap())
        .await
        .unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"AAABBB");
}

#[tokio::test]
async fn uploads_destination_mismatch_errors_on_commit() {
    let h = harness().await;
    let u = uploads_home(&h);
    let begin_dest = UserPath::new("/a.bin").unwrap();
    let handle = u.begin(&begin_dest).await.unwrap();
    let t1 = u
        .put_part(&handle.upload_id, 1, body(b"x".to_vec()))
        .await
        .unwrap();
    // Commit to a DIFFERENT destination — should error.
    let wrong = UserPath::new("/b.bin").unwrap();
    let r = u.commit(&handle.upload_id, &wrong, vec![t1]).await;
    assert!(matches!(r, Err(FsError::Upload(_))));
}

#[tokio::test]
async fn uploads_abort_idempotent_on_unknown_id() {
    let h = harness().await;
    let u = uploads_home(&h);
    // Never call begin — just abort a fabricated id.
    u.abort("AA:BB:CC").await.unwrap();
    // And again.
    u.abort("AA:BB:CC").await.unwrap();
}

#[tokio::test]
async fn uploads_abort_then_commit_errors() {
    let h = harness().await;
    let u = uploads_home(&h);
    let dest = UserPath::new("/aborted.bin").unwrap();
    let handle = u.begin(&dest).await.unwrap();
    let t1 = u
        .put_part(&handle.upload_id, 1, body(b"x".to_vec()))
        .await
        .unwrap();
    u.abort(&handle.upload_id).await.unwrap();

    // Commit on the same upload_id should now fail (the backend's
    // multipart state is gone).
    let r = u.commit(&handle.upload_id, &dest, vec![t1]).await;
    assert!(r.is_err());
}

#[tokio::test]
async fn uploads_part_tag_round_trip_assembles_in_order() {
    let h = harness().await;
    let u = uploads_home(&h);
    let dest = UserPath::new("/ordered.bin").unwrap();
    let handle = u.begin(&dest).await.unwrap();

    // Submit parts out of natural order; tags carry their part_number.
    let t3 = u
        .put_part(&handle.upload_id, 3, body(b"CCC".to_vec()))
        .await
        .unwrap();
    let t1 = u
        .put_part(&handle.upload_id, 1, body(b"AAA".to_vec()))
        .await
        .unwrap();
    let t2 = u
        .put_part(&handle.upload_id, 2, body(b"BBB".to_vec()))
        .await
        .unwrap();

    // Submitted out of natural order; commit expects tags sorted by
    // part_number (the storage layer concatenates in the supplied order).
    let mut tags = vec![t3, t1, t2];
    tags.sort_by_key(|t| t.part_number);
    let meta = u.commit(&handle.upload_id, &dest, tags).await.unwrap();
    assert_eq!(meta.size, 9);

    let mut reader = h
        .storage
        .read(&StoragePath::new("ordered.bin").unwrap())
        .await
        .unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"AAABBBCCC");
}
