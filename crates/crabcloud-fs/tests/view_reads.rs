mod support;

use crabcloud_fs::{FsError, MountKind, UserPath};
use crabcloud_storage::{memory::MemoryStorage, FileKind, NoopEventSink, Storage, StoragePath};
use std::sync::Arc;
use support::{harness, view_home, view_home_for, view_with_share_mount};
use tokio::io::AsyncReadExt;

// Silence unused-crate-dependencies for deps the lib uses but this
// integration-test target doesn't reference directly.
use async_trait as _;
use base64 as _;
use crabcloud_cache as _;
use crabcloud_core as _;
use crabcloud_search as _;
use crabcloud_sharing as _;
use thiserror as _;
use tracing as _;

use chrono as _;
use serde_json as _;
fn body(bytes: Vec<u8>) -> std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>> {
    Box::pin(std::io::Cursor::new(bytes))
}

#[tokio::test]
async fn view_stat_returns_metadata_for_existing_file() {
    let h = harness().await;
    // Seed via the storage directly (NoopEventSink — the View's stat goes
    // through cache populate on miss, hitting the backend stat).
    h.storage
        .put_file(
            &StoragePath::new("hello.txt").unwrap(),
            body(b"hi".to_vec()),
            &NoopEventSink,
        )
        .await
        .unwrap();

    let view = view_home(&h);
    let meta = view
        .stat(&UserPath::new("/hello.txt").unwrap())
        .await
        .unwrap();
    assert_eq!(meta.kind, FileKind::File);
    assert_eq!(meta.size, 2);
}

#[tokio::test]
async fn view_put_then_read_roundtrip() {
    let h = harness().await;
    let view = view_home(&h);

    let meta = view
        .put_file(&UserPath::new("/hi.txt").unwrap(), body(b"hello".to_vec()))
        .await
        .unwrap();
    assert_eq!(meta.size, 5);
    let fresh_etag = meta.etag.clone();

    // Read back via the View.
    let mut reader = view.read(&UserPath::new("/hi.txt").unwrap()).await.unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"hello");

    // The fresh etag is returned by put_file directly (no scanner lag).
    assert_eq!(fresh_etag.as_str().len(), 40);
}

#[tokio::test]
async fn view_list_returns_children() {
    let h = harness().await;
    let view = view_home(&h);
    view.mkdir(&UserPath::new("/d").unwrap()).await.unwrap();
    view.put_file(&UserPath::new("/d/a.txt").unwrap(), body(b"a".to_vec()))
        .await
        .unwrap();
    view.put_file(&UserPath::new("/d/b.txt").unwrap(), body(b"b".to_vec()))
        .await
        .unwrap();
    view.put_file(&UserPath::new("/d/c.txt").unwrap(), body(b"c".to_vec()))
        .await
        .unwrap();

    let entries = view.list(&UserPath::new("/d").unwrap()).await.unwrap();
    assert_eq!(entries.len(), 3);
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"a.txt"));
    assert!(names.contains(&"b.txt"));
    assert!(names.contains(&"c.txt"));
}

#[tokio::test]
async fn view_mkdir_creates_directory() {
    let h = harness().await;
    let view = view_home(&h);
    view.mkdir(&UserPath::new("/newdir").unwrap())
        .await
        .unwrap();
    let meta = view.stat(&UserPath::new("/newdir").unwrap()).await.unwrap();
    assert_eq!(meta.kind, FileKind::Directory);
}

#[tokio::test]
async fn view_delete_removes_file() {
    // SP12: `View::delete` routes through `Trash::soft_delete`, which
    // moves on-disk bytes under `<datadir>/<uid>/files/...`. This test
    // uses `MemoryStorage` (no on-disk bytes), so it asserts the
    // hard-delete contract directly. The soft-delete reroute is
    // exercised by `view_reroutes_delete_to_trash` below + the
    // `crabcloud-trash` e2e tests.
    let h = harness().await;
    let view = view_home(&h);
    view.put_file(&UserPath::new("/del.txt").unwrap(), body(b"x".to_vec()))
        .await
        .unwrap();
    view.hard_delete(&UserPath::new("/del.txt").unwrap())
        .await
        .unwrap();
    let r = view.stat(&UserPath::new("/del.txt").unwrap()).await;
    assert!(matches!(
        r,
        Err(FsError::FileCache(
            crabcloud_filecache::FileCacheError::NotFound
        ))
    ));
}

#[tokio::test]
async fn view_delete_removes_empty_directory() {
    // Same rationale as `view_delete_removes_file`; uses
    // `hard_delete` because the harness storage is in-memory.
    let h = harness().await;
    let view = view_home(&h);
    view.mkdir(&UserPath::new("/empty").unwrap()).await.unwrap();
    view.hard_delete(&UserPath::new("/empty").unwrap())
        .await
        .unwrap();
    let r = view.stat(&UserPath::new("/empty").unwrap()).await;
    assert!(matches!(
        r,
        Err(FsError::FileCache(
            crabcloud_filecache::FileCacheError::NotFound
        ))
    ));
}

#[tokio::test]
async fn view_invalid_user_path_rejected() {
    // No leading slash.
    assert!(matches!(
        UserPath::new("photos/cat.jpg"),
        Err(FsError::InvalidPath(_))
    ));
}

#[tokio::test]
async fn view_path_escape_via_dotdot_rejected() {
    // Path escape via .. is caught at UserPath construction.
    assert!(matches!(
        UserPath::new("/photos/../../etc/passwd"),
        Err(FsError::InvalidPath(_))
    ));
}

#[tokio::test]
async fn view_list_root_surfaces_share_mount_entry() {
    // bob lists `/`; he sees his own home entries plus one synthetic
    // entry for the share alice gave him. The synthetic entry's name is
    // the mount's basename ("Vacation Photos"), and its metadata carries
    // the share's `MountMetadata` so the server fn can decorate.
    let h = harness().await;

    // bob's home: one folder.
    let bob_home: Arc<dyn Storage> = Arc::new(MemoryStorage::new("bob"));
    bob_home
        .mkdir(&StoragePath::new("MyStuff").unwrap(), &NoopEventSink)
        .await
        .unwrap();
    // alice's home: the directory that backs the share.
    let alice_home: Arc<dyn Storage> = Arc::new(MemoryStorage::new("alice"));
    alice_home
        .mkdir(
            &StoragePath::new("Vacation Photos").unwrap(),
            &NoopEventSink,
        )
        .await
        .unwrap();

    let view = view_with_share_mount(
        &h,
        bob_home,
        alice_home,
        "Vacation Photos",
        "Vacation Photos",
    );
    let entries = view
        .list_with_meta(&UserPath::new("/").unwrap())
        .await
        .unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.entry.name.as_str()).collect();
    assert!(names.contains(&"MyStuff"));
    assert!(names.contains(&"Vacation Photos"));

    let share_le = entries
        .iter()
        .find(|e| e.entry.name == "Vacation Photos")
        .expect("share-mount entry present");
    let md = share_le
        .mount_metadata
        .as_ref()
        .expect("share entry carries mount metadata");
    assert_eq!(md.kind, MountKind::Share);
    assert_eq!(md.owner_uid.as_deref(), Some("alice"));
    assert_eq!(share_le.entry.metadata.kind, FileKind::Directory);

    // The plain `list` API also surfaces the entry (PROPFIND path),
    // just without the metadata side-band.
    let plain = view.list(&UserPath::new("/").unwrap()).await.unwrap();
    let plain_names: Vec<&str> = plain.iter().map(|e| e.name.as_str()).collect();
    assert!(plain_names.contains(&"Vacation Photos"));
}

#[tokio::test]
async fn view_list_root_home_only_user_unchanged() {
    let h = harness().await;
    let view = view_home(&h);
    view.mkdir(&UserPath::new("/folder").unwrap())
        .await
        .unwrap();
    let entries = view
        .list_with_meta(&UserPath::new("/").unwrap())
        .await
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].entry.name, "folder");
    assert!(entries[0].mount_metadata.is_none());
}

#[tokio::test]
async fn view_list_share_mount_entry_carries_owners_metadata() {
    // The synthetic entry's size / mtime / etag come from stat'ing the
    // OWNER's backing folder through the share wrapper. We deliberately
    // bypass the filecache for this stat (so we don't poison the cache
    // by writing alice's `/` row with Photos-shaped metadata) — but the
    // returned metadata's etag still equals what alice's storage reports
    // for `Photos`, which is the spec §3.2 "fileid stays stable across
    // recipients" invariant at the storage layer.
    let h = harness().await;
    let bob_home: Arc<dyn Storage> = Arc::new(MemoryStorage::new("bob"));
    let alice_home: Arc<dyn Storage> = Arc::new(MemoryStorage::new("alice"));
    alice_home
        .mkdir(&StoragePath::new("Photos").unwrap(), &NoopEventSink)
        .await
        .unwrap();
    let alice_direct = alice_home
        .stat(&StoragePath::new("Photos").unwrap())
        .await
        .unwrap();

    let view = view_with_share_mount(&h, bob_home, alice_home, "Photos", "Photos");
    let entries = view
        .list_with_meta(&UserPath::new("/").unwrap())
        .await
        .unwrap();
    let share_le = entries
        .iter()
        .find(|e| e.entry.name == "Photos")
        .expect("share entry");
    assert_eq!(share_le.entry.metadata.kind, FileKind::Directory);
    // The etag matches alice's direct stat — the synthetic entry is a
    // round-trip of the owner's metadata, not a freshly minted one.
    assert_eq!(
        share_le.entry.metadata.etag.as_str(),
        alice_direct.etag.as_str()
    );
}

#[tokio::test]
async fn view_list_inside_share_mount_returns_owners_children() {
    // Bob descends into `/Photos` (the share mount). View resolves to the
    // share mount, storage_path = root. The wrapper translates root →
    // alice's `/Photos`, so bob must see alice's actual /Photos contents.
    //
    // Regression guard for the read-correctness side of the documented
    // filecache-poisoning issue: even if the cache rows are written with
    // recipient-relative paths (Batch F / SP8 follow-up), the immediate
    // entries returned to bob must be the owner's children.
    let h = harness().await;
    let bob_home: Arc<dyn Storage> = Arc::new(MemoryStorage::new("bob"));
    let alice_home: Arc<dyn Storage> = Arc::new(MemoryStorage::new("alice"));
    alice_home
        .mkdir(&StoragePath::new("Photos").unwrap(), &NoopEventSink)
        .await
        .unwrap();
    alice_home
        .put_file(
            &StoragePath::new("Photos/sunset.jpg").unwrap(),
            body(b"jpeg".to_vec()),
            &NoopEventSink,
        )
        .await
        .unwrap();
    alice_home
        .put_file(
            &StoragePath::new("Photos/note.txt").unwrap(),
            body(b"hi".to_vec()),
            &NoopEventSink,
        )
        .await
        .unwrap();

    let view = view_with_share_mount(&h, bob_home, alice_home, "Photos", "Photos");
    let entries = view.list(&UserPath::new("/Photos").unwrap()).await.unwrap();
    let mut names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
    names.sort();
    assert_eq!(names, vec!["note.txt", "sunset.jpg"]);
}

#[tokio::test]
async fn view_read_range_returns_slice() {
    let h = harness().await;
    let view = view_home(&h);
    view.put_file(
        &UserPath::new("/range.txt").unwrap(),
        body(b"abcdefghij".to_vec()),
    )
    .await
    .unwrap();
    let mut reader = view
        .read_range(&UserPath::new("/range.txt").unwrap(), 2..5)
        .await
        .unwrap();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await.unwrap();
    assert_eq!(buf, b"cde");
}

#[tokio::test]
async fn view_descend_into_share_does_not_poison_owner_cache() {
    // After bob descends into a share, alice's cache row at her actual
    // home root must NOT be poisoned with the share's metadata. Before
    // the cache-key-translation fix, bob's `view.list("/Photos")` was
    // populating `(alice_id, root)` with `/Photos`-shaped metadata,
    // which then leaked into alice's own `view.stat("/")` calls.
    let h = harness().await;

    let alice_home: Arc<dyn Storage> = Arc::new(MemoryStorage::new("alice"));
    alice_home
        .mkdir(&StoragePath::new("Photos").unwrap(), &NoopEventSink)
        .await
        .unwrap();
    alice_home
        .put_file(
            &StoragePath::new("Photos/sunset.jpg").unwrap(),
            body(b"jpeg".to_vec()),
            &NoopEventSink,
        )
        .await
        .unwrap();
    alice_home
        .put_file(
            &StoragePath::new("notes.txt").unwrap(),
            body(b"buy milk".to_vec()),
            &NoopEventSink,
        )
        .await
        .unwrap();

    let bob_home: Arc<dyn Storage> = Arc::new(MemoryStorage::new("bob"));
    let bob_view = view_with_share_mount(&h, bob_home, alice_home.clone(), "Photos", "Photos");

    // 1. Bob descends into the share. WITHOUT the fix the wrapper's
    //    `(alice_id, root)` cache key gets populated by `filecache.stat`
    //    with alice's /Photos metadata (because `wrapper.stat(root) =
    //    inner.stat(/Photos)`). WITH the fix that call is rerouted to
    //    `(alice_id, /Photos)`, leaving `(alice_id, root)` alone.
    let bob_entries = bob_view
        .list(&UserPath::new("/Photos").unwrap())
        .await
        .unwrap();
    let bob_names: Vec<&str> = bob_entries.iter().map(|e| e.name.as_str()).collect();
    assert!(
        bob_names.contains(&"sunset.jpg"),
        "bob should see alice's /Photos children: got {bob_names:?}"
    );

    // 2. Inspect the raw cache rows. The fix routes bob's lookup to
    //    `(alice_id, /Photos)`; the bug instead writes `(alice_id, root)`
    //    with /Photos metadata, AND doesn't populate `/Photos`. So:
    //      - WITH fix:    (alice_id, /Photos) is Some; (alice_id, root) is None.
    //      - WITHOUT fix: (alice_id, root)   is Some; (alice_id, /Photos) is None.
    let alice_id = alice_home.id();
    let photos_row = h
        .filecache
        .lookup(alice_id, &StoragePath::new("Photos").unwrap())
        .await
        .unwrap();
    let root_row = h
        .filecache
        .lookup(alice_id, &StoragePath::root())
        .await
        .unwrap();
    assert!(
        photos_row.is_some(),
        "share-mount descend must populate alice's /Photos cache row (got None)"
    );
    assert!(
        root_row.is_none(),
        "share-mount descend must NOT poison alice's root cache row (got {root_row:?})"
    );
}

#[tokio::test]
async fn view_share_mount_preserves_file_id_continuity() {
    // SP7 §3.2: a file accessed through a share mount must have the SAME
    // identity as the owner's direct access — the recipient's sync client
    // should see the same fileid as the owner's. We pin this via the
    // etag: both views, going through the filecache, must read the same
    // cache row for the same underlying file.
    let h = harness().await;
    let alice_home: Arc<dyn Storage> = Arc::new(MemoryStorage::new("alice"));
    alice_home
        .mkdir(&StoragePath::new("Photos").unwrap(), &NoopEventSink)
        .await
        .unwrap();
    alice_home
        .put_file(
            &StoragePath::new("Photos/sunset.jpg").unwrap(),
            body(b"jpeg-bytes".to_vec()),
            &NoopEventSink,
        )
        .await
        .unwrap();

    let alice_view = view_home_for(&h, alice_home.clone());
    let alice_meta = alice_view
        .stat(&UserPath::new("/Photos/sunset.jpg").unwrap())
        .await
        .unwrap();

    let bob_home: Arc<dyn Storage> = Arc::new(MemoryStorage::new("bob"));
    let bob_view = view_with_share_mount(&h, bob_home, alice_home, "Photos", "Photos");
    let bob_meta = bob_view
        .stat(&UserPath::new("/Photos/sunset.jpg").unwrap())
        .await
        .unwrap();

    assert_eq!(
        alice_meta.etag.as_str(),
        bob_meta.etag.as_str(),
        "alice and bob must see the same etag for the same file (cache row)"
    );
    assert_eq!(alice_meta.size, bob_meta.size);
}
