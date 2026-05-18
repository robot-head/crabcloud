//! sqlite e2e for the Search service.

use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_db::{core_set, DbPool, MigrationRunner};
use crabcloud_search::{parse_query, Search};
use std::sync::Arc;
use tempfile::TempDir;

async fn setup() -> (Arc<DbPool>, TempDir) {
    let db_dir = TempDir::new().unwrap();
    let cfg = minimal_sqlite_config(db_dir.path().join("test.db"));
    let pool = DbPool::connect(&cfg).await.unwrap();
    let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
    runner.register(core_set());
    runner.run().await.unwrap();
    (Arc::new(pool), db_dir)
}

#[tokio::test]
async fn empty_query_returns_empty() {
    let (pool, _d) = setup().await;
    let search = Search::new(pool);
    let hits = search
        .query("alice", &parse_query(""), 10, None)
        .await
        .unwrap();
    assert!(hits.is_empty());
}

#[tokio::test]
async fn upsert_then_query_returns_hit() {
    let (pool, _d) = setup().await;
    let search = Search::new(pool);
    search
        .upsert_for_file(
            "alice",
            100,
            "local::/alice/files",
            "report.docx",
            "/docs/report.docx",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            1_700_000_000,
            12345,
        )
        .await
        .unwrap();

    let hits = search
        .query("alice", &parse_query("report"), 10, None)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].fileid, 100);
    assert_eq!(hits[0].basename, "report.docx");
    assert_eq!(hits[0].path, "/docs/report.docx");
}

#[tokio::test]
async fn query_filters_by_mime() {
    let (pool, _d) = setup().await;
    let search = Search::new(pool);
    search
        .upsert_for_file(
            "alice",
            100,
            "local::/alice/files",
            "photo.jpg",
            "/pics/photo.jpg",
            "image/jpeg",
            1_700_000_000,
            200_000,
        )
        .await
        .unwrap();
    search
        .upsert_for_file(
            "alice",
            101,
            "local::/alice/files",
            "report.docx",
            "/docs/report.docx",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            1_700_000_000,
            12345,
        )
        .await
        .unwrap();

    let hits = search
        .query("alice", &parse_query("photo mime:image/*"), 10, None)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].fileid, 100);
}

#[tokio::test]
async fn query_filters_by_modified_range() {
    let (pool, _d) = setup().await;
    let search = Search::new(pool);
    search
        .upsert_for_file(
            "alice",
            100,
            "s",
            "old.txt",
            "/o/old.txt",
            "text/plain",
            1_500_000_000,
            1,
        )
        .await
        .unwrap();
    search
        .upsert_for_file(
            "alice",
            101,
            "s",
            "new.txt",
            "/o/new.txt",
            "text/plain",
            1_700_000_000,
            1,
        )
        .await
        .unwrap();

    let hits = search
        .query("alice", &parse_query("txt modified:>1600000000"), 10, None)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].fileid, 101);
}

#[tokio::test]
async fn query_filters_by_size_min() {
    let (pool, _d) = setup().await;
    let search = Search::new(pool);
    search
        .upsert_for_file(
            "alice",
            100,
            "s",
            "small.bin",
            "/s/small.bin",
            "application/octet-stream",
            1_700_000_000,
            500,
        )
        .await
        .unwrap();
    search
        .upsert_for_file(
            "alice",
            101,
            "s",
            "big.bin",
            "/s/big.bin",
            "application/octet-stream",
            1_700_000_000,
            5_000_000,
        )
        .await
        .unwrap();

    let hits = search
        .query("alice", &parse_query("bin size:>1MB"), 10, None)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].fileid, 101);
}

#[tokio::test]
async fn query_isolates_per_viewer() {
    let (pool, _d) = setup().await;
    let search = Search::new(pool);
    search
        .upsert_for_file(
            "alice",
            100,
            "s",
            "report.docx",
            "/docs/report.docx",
            "application/octet-stream",
            1_700_000_000,
            1,
        )
        .await
        .unwrap();
    let alice_hits = search
        .query("alice", &parse_query("report"), 10, None)
        .await
        .unwrap();
    let bob_hits = search
        .query("bob", &parse_query("report"), 10, None)
        .await
        .unwrap();
    assert_eq!(alice_hits.len(), 1);
    assert!(bob_hits.is_empty());
}

#[tokio::test]
async fn upsert_updates_existing_row() {
    let (pool, _d) = setup().await;
    let search = Search::new(pool);
    search
        .upsert_for_file(
            "alice",
            100,
            "s",
            "old.txt",
            "/x/old.txt",
            "text/plain",
            1_700_000_000,
            1,
        )
        .await
        .unwrap();
    search
        .upsert_for_file(
            "alice",
            100,
            "s",
            "new.txt",
            "/x/new.txt",
            "text/plain",
            1_700_000_100,
            2,
        )
        .await
        .unwrap();
    let hits = search
        .query("alice", &parse_query("new"), 10, None)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].basename, "new.txt");
    let stale = search
        .query("alice", &parse_query("old"), 10, None)
        .await
        .unwrap();
    assert!(stale.is_empty());
}

#[tokio::test]
async fn delete_for_file_removes_all_viewers() {
    let (pool, _d) = setup().await;
    let search = Search::new(pool);
    search
        .upsert_for_file(
            "alice",
            100,
            "s",
            "x.txt",
            "/x.txt",
            "text/plain",
            1_700_000_000,
            1,
        )
        .await
        .unwrap();
    search
        .upsert_for_file(
            "bob",
            100,
            "s",
            "x.txt",
            "/x.txt",
            "text/plain",
            1_700_000_000,
            1,
        )
        .await
        .unwrap();
    search.delete_for_file(100).await.unwrap();
    assert!(search
        .query("alice", &parse_query("x"), 10, None)
        .await
        .unwrap()
        .is_empty());
    assert!(search
        .query("bob", &parse_query("x"), 10, None)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn delete_for_viewer_file_targets_one_row() {
    let (pool, _d) = setup().await;
    let search = Search::new(pool);
    search
        .upsert_for_file(
            "alice",
            100,
            "s",
            "x.txt",
            "/x.txt",
            "text/plain",
            1_700_000_000,
            1,
        )
        .await
        .unwrap();
    search
        .upsert_for_file(
            "bob",
            100,
            "s",
            "x.txt",
            "/x.txt",
            "text/plain",
            1_700_000_000,
            1,
        )
        .await
        .unwrap();
    search.delete_for_viewer_file("bob", 100).await.unwrap();
    assert!(!search
        .query("alice", &parse_query("x"), 10, None)
        .await
        .unwrap()
        .is_empty());
    assert!(search
        .query("bob", &parse_query("x"), 10, None)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn query_pagination_cursor() {
    let (pool, _d) = setup().await;
    let search = Search::new(pool);
    for i in 0..5 {
        search
            .upsert_for_file(
                "alice",
                100 + i,
                "s",
                &format!("rpt-{i}.txt"),
                &format!("/r/rpt-{i}.txt"),
                "text/plain",
                1_700_000_000 + i,
                1,
            )
            .await
            .unwrap();
    }
    let page1 = search
        .query("alice", &parse_query("rpt"), 2, None)
        .await
        .unwrap();
    assert_eq!(page1.len(), 2);
    let cursor = (page1.last().unwrap().rank, page1.last().unwrap().fileid);
    let page2 = search
        .query("alice", &parse_query("rpt"), 2, Some(cursor))
        .await
        .unwrap();
    assert_eq!(page2.len(), 2);
    let p1_ids: std::collections::HashSet<_> = page1.iter().map(|h| h.fileid).collect();
    for h in &page2 {
        assert!(!p1_ids.contains(&h.fileid));
    }
}

#[tokio::test]
async fn fileid_for_storage_path_finds_owner_row() {
    let (pool, _d) = setup().await;
    let search = Search::new(pool);
    search
        .upsert_for_file(
            "alice",
            42,
            "local::/alice/files",
            "report.docx",
            "/docs/report.docx",
            "text/plain",
            1_700_000_000,
            1,
        )
        .await
        .unwrap();
    let fid = search
        .fileid_for_storage_path("local::/alice/files", "/docs/report.docx")
        .await
        .unwrap();
    assert_eq!(fid, Some(42));

    let missing = search
        .fileid_for_storage_path("local::/alice/files", "/no/such/path")
        .await
        .unwrap();
    assert_eq!(missing, None);
}

#[tokio::test]
async fn phrase_query_matches_adjacent_tokens() {
    let (pool, _d) = setup().await;
    let search = Search::new(pool);
    search
        .upsert_for_file(
            "alice",
            100,
            "s",
            "q3 report.docx",
            "/q3 report.docx",
            "text/plain",
            1_700_000_000,
            1,
        )
        .await
        .unwrap();
    search
        .upsert_for_file(
            "alice",
            101,
            "s",
            "report q3.docx",
            "/report q3.docx",
            "text/plain",
            1_700_000_000,
            1,
        )
        .await
        .unwrap();
    let hits = search
        .query("alice", &parse_query("\"q3 report\""), 10, None)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].fileid, 100);
}
