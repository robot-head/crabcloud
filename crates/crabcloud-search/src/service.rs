//! `Search` — query / upsert / delete / fan-out.
//!
//! Spec sections §4 (schema), §5 (surface contracts), §6 (edge cases).
//! Per-dialect dispatch via `match self.pool.as_ref()`. sqlite uses
//! FTS5; mysql uses FULLTEXT NATURAL LANGUAGE MODE; postgres uses
//! tsvector + plainto_tsquery + ts_rank_cd.

use crate::error::SearchError;
use crate::sql;
use crate::types::{BatchUpsertRow, SearchHit, SearchQuery};
use async_trait::async_trait;
use crabcloud_db::DbPool;
use crabcloud_filecache::FileCache;
use crabcloud_users::UserId;
use sqlx::Row as _;
use std::sync::Arc;

/// Max rows per batched statement. 500 × 8 placeholders = 4000,
/// comfortably under sqlite's default `SQLITE_MAX_VARIABLE_NUMBER`
/// of 32766. Same cap is used for mysql / postgres (they tolerate
/// much larger statements, but a single cap keeps the code simple).
pub const BATCH_CHUNK_SIZE: usize = 500;

#[derive(Clone)]
pub struct Search {
    pool: Arc<DbPool>,
}

impl Search {
    pub fn new(pool: Arc<DbPool>) -> Self {
        Self { pool }
    }

    /// UPSERT one (viewer, fileid) row. sqlite uses DELETE-then-INSERT
    /// in a transaction (FTS5 has no UPSERT); mysql + pg use their
    /// native conflict-resolution syntax.
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_for_file(
        &self,
        viewer_uid: &str,
        fileid: i64,
        storage_id: &str,
        basename: &str,
        path: &str,
        mime: &str,
        mtime: i64,
        size: i64,
    ) -> Result<(), SearchError> {
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let mut tx = p.begin().await?;
                sqlx::query(sql::DELETE_VIEWER_FILE_QM)
                    .bind(viewer_uid)
                    .bind(fileid)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query(sql::INSERT_QM)
                    .bind(viewer_uid)
                    .bind(fileid)
                    .bind(storage_id)
                    .bind(basename)
                    .bind(path)
                    .bind(mime)
                    .bind(mtime)
                    .bind(size)
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
            }
            DbPool::MySql(p) => {
                sqlx::query(sql::INSERT_MYSQL_UPSERT)
                    .bind(viewer_uid)
                    .bind(fileid)
                    .bind(storage_id)
                    .bind(basename)
                    .bind(path)
                    .bind(mime)
                    .bind(mtime)
                    .bind(size)
                    .execute(p)
                    .await?;
            }
            DbPool::Postgres(p) => {
                sqlx::query(sql::INSERT_PG_UPSERT)
                    .bind(viewer_uid)
                    .bind(fileid)
                    .bind(storage_id)
                    .bind(basename)
                    .bind(path)
                    .bind(mime)
                    .bind(mtime)
                    .bind(size)
                    .execute(p)
                    .await?;
            }
        }
        Ok(())
    }

    /// DELETE every viewer row for `fileid`. Called when a file is
    /// hard-deleted or soft-deleted to trash.
    pub async fn delete_for_file(&self, fileid: i64) -> Result<(), SearchError> {
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                sqlx::query(sql::DELETE_FILEID_QM)
                    .bind(fileid)
                    .execute(p)
                    .await?;
            }
            DbPool::MySql(p) => {
                sqlx::query(sql::DELETE_FILEID_QM)
                    .bind(fileid)
                    .execute(p)
                    .await?;
            }
            DbPool::Postgres(p) => {
                sqlx::query(sql::DELETE_FILEID_PG)
                    .bind(fileid)
                    .execute(p)
                    .await?;
            }
        }
        Ok(())
    }

    /// DELETE one (viewer, fileid) row. Used by `fan_out_for_unshare`.
    pub async fn delete_for_viewer_file(
        &self,
        viewer_uid: &str,
        fileid: i64,
    ) -> Result<(), SearchError> {
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                sqlx::query(sql::DELETE_VIEWER_FILE_QM)
                    .bind(viewer_uid)
                    .bind(fileid)
                    .execute(p)
                    .await?;
            }
            DbPool::MySql(p) => {
                sqlx::query(sql::DELETE_VIEWER_FILE_QM)
                    .bind(viewer_uid)
                    .bind(fileid)
                    .execute(p)
                    .await?;
            }
            DbPool::Postgres(p) => {
                sqlx::query(sql::DELETE_VIEWER_FILE_PG)
                    .bind(viewer_uid)
                    .bind(fileid)
                    .execute(p)
                    .await?;
            }
        }
        Ok(())
    }

    /// Batched UPSERT for many `(viewer_uid, fileid)` rows at once.
    ///
    /// Same semantics as calling [`Self::upsert_for_file`] once per
    /// row, but issues O(rows / [`BATCH_CHUNK_SIZE`]) statements
    /// instead of one per row. Used by [`SearchFanout::fan_out_for_share`]
    /// where a single share lifecycle event can touch
    /// `recipients × files_under_subroot` rows.
    ///
    /// - **sqlite**: one transaction per chunk; one tuple-IN DELETE
    ///   followed by one multi-row INSERT (FTS5 has no UPSERT).
    /// - **mysql**: one multi-row INSERT ... ON DUPLICATE KEY UPDATE.
    /// - **postgres**: one multi-row INSERT ... ON CONFLICT ... DO UPDATE.
    ///
    /// Chunk size capped at [`BATCH_CHUNK_SIZE`] (500 rows × 8
    /// columns = 4000 placeholders) to stay under sqlite's default
    /// `SQLITE_MAX_VARIABLE_NUMBER` of 32766.
    pub async fn upsert_many(&self, rows: &[BatchUpsertRow]) -> Result<(), SearchError> {
        if rows.is_empty() {
            return Ok(());
        }
        for chunk in rows.chunks(BATCH_CHUNK_SIZE) {
            match self.pool.as_ref() {
                DbPool::Sqlite(p) => {
                    let mut tx = p.begin().await?;
                    // DELETE WHERE (viewer_uid, fileid) IN ((?,?), ...)
                    let mut del_sql =
                        String::from("DELETE FROM oc_search WHERE (viewer_uid, fileid) IN (");
                    for i in 0..chunk.len() {
                        if i > 0 {
                            del_sql.push(',');
                        }
                        del_sql.push_str("(?,?)");
                    }
                    del_sql.push(')');
                    let mut del_q = sqlx::query(&del_sql);
                    for row in chunk {
                        del_q = del_q.bind(&row.viewer_uid).bind(row.fileid);
                    }
                    del_q.execute(&mut *tx).await?;

                    // Multi-row INSERT.
                    let mut ins_sql = String::from(
                        "INSERT INTO oc_search \
                         (viewer_uid, fileid, storage_id, basename, path, mime, mtime, size) VALUES ",
                    );
                    for i in 0..chunk.len() {
                        if i > 0 {
                            ins_sql.push(',');
                        }
                        ins_sql.push_str("(?,?,?,?,?,?,?,?)");
                    }
                    let mut ins_q = sqlx::query(&ins_sql);
                    for row in chunk {
                        ins_q = ins_q
                            .bind(&row.viewer_uid)
                            .bind(row.fileid)
                            .bind(&row.storage_id)
                            .bind(&row.basename)
                            .bind(&row.path)
                            .bind(&row.mime)
                            .bind(row.mtime)
                            .bind(row.size);
                    }
                    ins_q.execute(&mut *tx).await?;
                    tx.commit().await?;
                }
                DbPool::MySql(p) => {
                    let mut sql = String::from(
                        "INSERT INTO oc_search \
                         (viewer_uid, fileid, storage_id, basename, path, mime, mtime, size) VALUES ",
                    );
                    for i in 0..chunk.len() {
                        if i > 0 {
                            sql.push(',');
                        }
                        sql.push_str("(?,?,?,?,?,?,?,?)");
                    }
                    sql.push_str(
                        " ON DUPLICATE KEY UPDATE \
                         storage_id = VALUES(storage_id), basename = VALUES(basename), \
                         path = VALUES(path), mime = VALUES(mime), \
                         mtime = VALUES(mtime), size = VALUES(size)",
                    );
                    let mut q = sqlx::query(&sql);
                    for row in chunk {
                        q = q
                            .bind(&row.viewer_uid)
                            .bind(row.fileid)
                            .bind(&row.storage_id)
                            .bind(&row.basename)
                            .bind(&row.path)
                            .bind(&row.mime)
                            .bind(row.mtime)
                            .bind(row.size);
                    }
                    q.execute(p).await?;
                }
                DbPool::Postgres(p) => {
                    let mut sql = String::from(
                        "INSERT INTO oc_search \
                         (viewer_uid, fileid, storage_id, basename, path, mime, mtime, size) VALUES ",
                    );
                    let mut n = 1usize;
                    for i in 0..chunk.len() {
                        if i > 0 {
                            sql.push(',');
                        }
                        sql.push_str(&format!(
                            "(${},${},${},${},${},${},${},${})",
                            n,
                            n + 1,
                            n + 2,
                            n + 3,
                            n + 4,
                            n + 5,
                            n + 6,
                            n + 7,
                        ));
                        n += 8;
                    }
                    sql.push_str(
                        " ON CONFLICT (viewer_uid, fileid) DO UPDATE SET \
                         storage_id = EXCLUDED.storage_id, basename = EXCLUDED.basename, \
                         path = EXCLUDED.path, mime = EXCLUDED.mime, \
                         mtime = EXCLUDED.mtime, size = EXCLUDED.size",
                    );
                    let mut q = sqlx::query(&sql);
                    for row in chunk {
                        q = q
                            .bind(&row.viewer_uid)
                            .bind(row.fileid)
                            .bind(&row.storage_id)
                            .bind(&row.basename)
                            .bind(&row.path)
                            .bind(&row.mime)
                            .bind(row.mtime)
                            .bind(row.size);
                    }
                    q.execute(p).await?;
                }
            }
        }
        Ok(())
    }

    /// Batched DELETE for many `(viewer_uid, fileid)` pairs. Same
    /// semantics as calling [`Self::delete_for_viewer_file`] once per
    /// pair, but issues O(pairs / [`BATCH_CHUNK_SIZE`]) statements.
    /// Used by [`SearchFanout::fan_out_for_unshare`].
    ///
    /// Tuple-IN syntax works on all three dialects (sqlite, mysql,
    /// postgres). Chunked at [`BATCH_CHUNK_SIZE`] pairs per
    /// statement.
    pub async fn delete_many_for_viewer_files(
        &self,
        pairs: &[(String, i64)],
    ) -> Result<(), SearchError> {
        if pairs.is_empty() {
            return Ok(());
        }
        for chunk in pairs.chunks(BATCH_CHUNK_SIZE) {
            match self.pool.as_ref() {
                DbPool::Sqlite(p) => {
                    let mut sql =
                        String::from("DELETE FROM oc_search WHERE (viewer_uid, fileid) IN (");
                    for i in 0..chunk.len() {
                        if i > 0 {
                            sql.push(',');
                        }
                        sql.push_str("(?,?)");
                    }
                    sql.push(')');
                    let mut q = sqlx::query(&sql);
                    for (viewer, fid) in chunk {
                        q = q.bind(viewer).bind(*fid);
                    }
                    q.execute(p).await?;
                }
                DbPool::MySql(p) => {
                    let mut sql =
                        String::from("DELETE FROM oc_search WHERE (viewer_uid, fileid) IN (");
                    for i in 0..chunk.len() {
                        if i > 0 {
                            sql.push(',');
                        }
                        sql.push_str("(?,?)");
                    }
                    sql.push(')');
                    let mut q = sqlx::query(&sql);
                    for (viewer, fid) in chunk {
                        q = q.bind(viewer).bind(*fid);
                    }
                    q.execute(p).await?;
                }
                DbPool::Postgres(p) => {
                    let mut sql =
                        String::from("DELETE FROM oc_search WHERE (viewer_uid, fileid) IN (");
                    let mut n = 1usize;
                    for i in 0..chunk.len() {
                        if i > 0 {
                            sql.push(',');
                        }
                        sql.push_str(&format!("(${},${})", n, n + 1));
                        n += 2;
                    }
                    sql.push(')');
                    let mut q = sqlx::query(&sql);
                    for (viewer, fid) in chunk {
                        q = q.bind(viewer).bind(*fid);
                    }
                    q.execute(p).await?;
                }
            }
        }
        Ok(())
    }

    /// Discover the fileid for `(storage_id, path)` by querying the
    /// search index itself. Returns `None` if no matching row exists.
    ///
    /// Used by [`crate::SearchFanout`] consumers and by the indexer's
    /// `Deleted`-event handler when the underlying `StorageEvent`
    /// doesn't carry the fileid: we look it up here, then call
    /// [`Search::delete_for_file`] to cascade across viewers.
    pub async fn fileid_for_storage_path(
        &self,
        storage_id: &str,
        path: &str,
    ) -> Result<Option<i64>, SearchError> {
        let fid: Option<i64> = match self.pool.as_ref() {
            DbPool::Sqlite(p) => sqlx::query(sql::LOOKUP_FILEID_BY_STORAGE_PATH_QM)
                .bind(storage_id)
                .bind(path)
                .fetch_optional(p)
                .await?
                .map(|r| r.try_get::<i64, _>("fileid"))
                .transpose()?,
            DbPool::MySql(p) => sqlx::query(sql::LOOKUP_FILEID_BY_STORAGE_PATH_QM)
                .bind(storage_id)
                .bind(path)
                .fetch_optional(p)
                .await?
                .map(|r| r.try_get::<i64, _>("fileid"))
                .transpose()?,
            DbPool::Postgres(p) => sqlx::query(sql::LOOKUP_FILEID_BY_STORAGE_PATH_PG)
                .bind(storage_id)
                .bind(path)
                .fetch_optional(p)
                .await?
                .map(|r| r.try_get::<i64, _>("fileid"))
                .transpose()?,
        };
        Ok(fid)
    }

    pub async fn query(
        &self,
        viewer_uid: &str,
        q: &SearchQuery,
        limit: i64,
        cursor: Option<(f64, i64)>,
    ) -> Result<Vec<SearchHit>, SearchError> {
        if q.is_empty() || !q.has_text_match() {
            return Ok(Vec::new());
        }
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => self.query_sqlite(p, viewer_uid, q, limit, cursor).await,
            DbPool::MySql(p) => self.query_mysql(p, viewer_uid, q, limit, cursor).await,
            DbPool::Postgres(p) => self.query_pg(p, viewer_uid, q, limit, cursor).await,
        }
    }

    async fn query_sqlite(
        &self,
        pool: &sqlx::SqlitePool,
        viewer_uid: &str,
        q: &SearchQuery,
        limit: i64,
        cursor: Option<(f64, i64)>,
    ) -> Result<Vec<SearchHit>, SearchError> {
        // Build the FTS5 MATCH expression: phrase first as "..."-quoted,
        // then bare tokens AND'd by space.
        let mut match_expr = String::new();
        if let Some(ph) = &q.phrase {
            match_expr.push_str(&format!("\"{}\"", escape_fts5(ph)));
        }
        if !q.text.is_empty() {
            if !match_expr.is_empty() {
                match_expr.push(' ');
            }
            match_expr.push_str(&sanitize_fts5_bare_tokens(&q.text));
        }
        let mut sql = String::from(sql::QUERY_BASE_SQLITE);
        let mut bind_mime = None;
        let mut bind_after = None;
        let mut bind_before = None;
        let mut bind_size_min = None;
        let mut bind_size_max = None;
        if let Some(m) = &q.mime {
            sql.push_str(" AND mime GLOB ?");
            bind_mime = Some(m.clone());
        }
        if let Some(t) = q.modified_after {
            sql.push_str(" AND mtime >= ?");
            bind_after = Some(t);
        }
        if let Some(t) = q.modified_before {
            sql.push_str(" AND mtime <= ?");
            bind_before = Some(t);
        }
        if let Some(n) = q.size_min {
            sql.push_str(" AND size >= ?");
            bind_size_min = Some(n);
        }
        if let Some(n) = q.size_max {
            sql.push_str(" AND size <= ?");
            bind_size_max = Some(n);
        }
        let (cursor_rank, cursor_id) = match cursor {
            Some((r, id)) => (Some(r), Some(id)),
            None => (None, None),
        };
        if cursor_rank.is_some() {
            // sqlite bm25 lower = better; strictly-after the cursor.
            sql.push_str(" AND (bm25(oc_search) > ? OR (bm25(oc_search) = ? AND fileid > ?))");
        }
        sql.push_str(" ORDER BY bm25(oc_search) ASC, fileid ASC LIMIT ?");

        let mut query = sqlx::query(&sql).bind(viewer_uid).bind(&match_expr);
        if let Some(m) = bind_mime {
            query = query.bind(m);
        }
        if let Some(t) = bind_after {
            query = query.bind(t);
        }
        if let Some(t) = bind_before {
            query = query.bind(t);
        }
        if let Some(n) = bind_size_min {
            query = query.bind(n);
        }
        if let Some(n) = bind_size_max {
            query = query.bind(n);
        }
        if let Some(r) = cursor_rank {
            query = query.bind(r).bind(r).bind(cursor_id.unwrap());
        }
        query = query.bind(limit);

        let rows = query.fetch_all(pool).await?;
        rows.into_iter().map(row_from_sqlite).collect()
    }

    async fn query_mysql(
        &self,
        pool: &sqlx::MySqlPool,
        viewer_uid: &str,
        q: &SearchQuery,
        limit: i64,
        cursor: Option<(f64, i64)>,
    ) -> Result<Vec<SearchHit>, SearchError> {
        // mysql NATURAL LANGUAGE MODE has no phrase syntax; collapse the
        // phrase into bare tokens (documented MVP soft-coalesce).
        let mut match_text = q.text.clone();
        if let Some(ph) = &q.phrase {
            if !match_text.is_empty() {
                match_text.push(' ');
            }
            match_text.push_str(ph);
        }

        let mut sql = String::from(sql::QUERY_BASE_MYSQL);
        let mut bind_mime = None;
        let mut bind_after = None;
        let mut bind_before = None;
        let mut bind_size_min = None;
        let mut bind_size_max = None;
        if let Some(m) = &q.mime {
            sql.push_str(" AND mime LIKE ?");
            bind_mime = Some(m.replace('*', "%"));
        }
        if let Some(t) = q.modified_after {
            sql.push_str(" AND mtime >= ?");
            bind_after = Some(t);
        }
        if let Some(t) = q.modified_before {
            sql.push_str(" AND mtime <= ?");
            bind_before = Some(t);
        }
        if let Some(n) = q.size_min {
            sql.push_str(" AND size >= ?");
            bind_size_min = Some(n);
        }
        if let Some(n) = q.size_max {
            sql.push_str(" AND size <= ?");
            bind_size_max = Some(n);
        }
        let (cursor_rank, cursor_id) = match cursor {
            Some((r, id)) => (Some(r), Some(id)),
            None => (None, None),
        };
        if cursor_rank.is_some() {
            // mysql MATCH rank: higher = better. Strictly-after.
            sql.push_str(
                " AND (MATCH(basename, path) AGAINST(? IN NATURAL LANGUAGE MODE) < ? \
                 OR (MATCH(basename, path) AGAINST(? IN NATURAL LANGUAGE MODE) = ? AND fileid > ?))",
            );
        }
        sql.push_str(" ORDER BY rank DESC, fileid ASC LIMIT ?");

        let mut query = sqlx::query(&sql)
            .bind(&match_text)
            .bind(viewer_uid)
            .bind(&match_text);
        if let Some(m) = bind_mime {
            query = query.bind(m);
        }
        if let Some(t) = bind_after {
            query = query.bind(t);
        }
        if let Some(t) = bind_before {
            query = query.bind(t);
        }
        if let Some(n) = bind_size_min {
            query = query.bind(n);
        }
        if let Some(n) = bind_size_max {
            query = query.bind(n);
        }
        if let Some(r) = cursor_rank {
            query = query
                .bind(&match_text)
                .bind(r)
                .bind(&match_text)
                .bind(r)
                .bind(cursor_id.unwrap());
        }
        query = query.bind(limit);

        let rows = query.fetch_all(pool).await?;
        rows.into_iter().map(row_from_mysql).collect()
    }

    async fn query_pg(
        &self,
        pool: &sqlx::PgPool,
        viewer_uid: &str,
        q: &SearchQuery,
        limit: i64,
        cursor: Option<(f64, i64)>,
    ) -> Result<Vec<SearchHit>, SearchError> {
        let mut match_text = q.text.clone();
        if let Some(ph) = &q.phrase {
            if !match_text.is_empty() {
                match_text.push(' ');
            }
            match_text.push_str(ph);
        }

        let mut sql = String::from(sql::QUERY_BASE_PG);
        let mut next_arg = 3; // base template uses $1 (rank-input + tsquery) and $2 (viewer)
        let mut bind_mime = None;
        let mut bind_after = None;
        let mut bind_before = None;
        let mut bind_size_min = None;
        let mut bind_size_max = None;
        if let Some(m) = &q.mime {
            sql.push_str(&format!(" AND mime LIKE ${next_arg}"));
            next_arg += 1;
            bind_mime = Some(m.replace('*', "%"));
        }
        if let Some(t) = q.modified_after {
            sql.push_str(&format!(" AND mtime >= ${next_arg}"));
            next_arg += 1;
            bind_after = Some(t);
        }
        if let Some(t) = q.modified_before {
            sql.push_str(&format!(" AND mtime <= ${next_arg}"));
            next_arg += 1;
            bind_before = Some(t);
        }
        if let Some(n) = q.size_min {
            sql.push_str(&format!(" AND size >= ${next_arg}"));
            next_arg += 1;
            bind_size_min = Some(n);
        }
        if let Some(n) = q.size_max {
            sql.push_str(&format!(" AND size <= ${next_arg}"));
            next_arg += 1;
            bind_size_max = Some(n);
        }
        let (cursor_rank, cursor_id) = match cursor {
            Some((r, id)) => (Some(r), Some(id)),
            None => (None, None),
        };
        if cursor_rank.is_some() {
            let r_pos = next_arg;
            let r_pos2 = next_arg + 1;
            let id_pos = next_arg + 2;
            sql.push_str(&format!(
                " AND (ts_rank_cd(tsv, plainto_tsquery('simple', $1)) < ${r_pos} \
                 OR (ts_rank_cd(tsv, plainto_tsquery('simple', $1)) = ${r_pos2} AND fileid > ${id_pos}))"
            ));
            next_arg += 3;
        }
        let limit_pos = next_arg;
        sql.push_str(&format!(
            " ORDER BY rank DESC, fileid ASC LIMIT ${limit_pos}"
        ));

        let mut query = sqlx::query(&sql).bind(&match_text).bind(viewer_uid);
        if let Some(m) = bind_mime {
            query = query.bind(m);
        }
        if let Some(t) = bind_after {
            query = query.bind(t);
        }
        if let Some(t) = bind_before {
            query = query.bind(t);
        }
        if let Some(n) = bind_size_min {
            query = query.bind(n);
        }
        if let Some(n) = bind_size_max {
            query = query.bind(n);
        }
        if let Some(r) = cursor_rank {
            query = query.bind(r).bind(r).bind(cursor_id.unwrap());
        }
        query = query.bind(limit);

        let rows = query.fetch_all(pool).await?;
        rows.into_iter().map(row_from_pg).collect()
    }
}

fn escape_fts5(s: &str) -> String {
    s.replace('"', "\"\"")
}

/// Sanitize a bare token string for FTS5 by wrapping each
/// whitespace-separated token in a quoted phrase. This makes colons,
/// parens, operators (`*`, `-`, `+`, `^`, `:`), and unknown
/// `key:value` tokens literal — FTS5 would otherwise parse them as
/// column-qualified search, NOT operator, prefix wildcard, etc., and
/// either error with "no such column: foo" or return wrong results.
/// mysql `NATURAL LANGUAGE MODE` and pg `plainto_tsquery` are already
/// permissive, so this helper is sqlite-FTS5 only.
///
/// Empty input → empty output (the caller checks `has_text_match`
/// before invoking us).
fn sanitize_fts5_bare_tokens(text: &str) -> String {
    text.split_whitespace()
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

fn row_from_sqlite(r: sqlx::sqlite::SqliteRow) -> Result<SearchHit, SearchError> {
    Ok(SearchHit {
        fileid: r.try_get("fileid")?,
        storage_id: r.try_get("storage_id")?,
        basename: r.try_get("basename")?,
        path: r.try_get("path")?,
        mime: r.try_get("mime")?,
        mtime: r.try_get("mtime")?,
        size: r.try_get("size")?,
        rank: r.try_get("rank")?,
    })
}

fn row_from_mysql(r: sqlx::mysql::MySqlRow) -> Result<SearchHit, SearchError> {
    Ok(SearchHit {
        fileid: r.try_get("fileid")?,
        storage_id: r.try_get_unchecked("storage_id")?,
        basename: r.try_get_unchecked("basename")?,
        path: r.try_get_unchecked("path")?,
        mime: r.try_get_unchecked("mime")?,
        mtime: r.try_get("mtime")?,
        size: r.try_get("size")?,
        // mysql MATCH() returns FLOAT.
        rank: r.try_get::<f32, _>("rank")? as f64,
    })
}

fn row_from_pg(r: sqlx::postgres::PgRow) -> Result<SearchHit, SearchError> {
    Ok(SearchHit {
        fileid: r.try_get("fileid")?,
        storage_id: r.try_get("storage_id")?,
        basename: r.try_get("basename")?,
        path: r.try_get("path")?,
        mime: r.try_get("mime")?,
        mtime: r.try_get("mtime")?,
        size: r.try_get("size")?,
        // postgres ts_rank_cd returns FLOAT4.
        rank: r.try_get::<f32, _>("rank")? as f64,
    })
}

/// Trait that `crabcloud-sharing` depends on so it can drive bulk
/// fan-out at share lifecycle events without taking a hard dep on
/// `crabcloud-search`. `Search` itself impls this; tests can use
/// [`NoopSearchFanout`].
///
/// The `filecache` reference is passed in so the trait can avoid
/// taking an `Arc<FileCache>` field on every implementor — the share
/// service already holds one and threads it through.
#[async_trait]
pub trait SearchFanout: Send + Sync {
    /// Walk every fileid under `owner_subroot_path` in `owner_uid`'s
    /// home storage (looked up via `filecache.lookup` against the
    /// owner's storage id, then `walk_under` per the helper added in
    /// Task A6) and UPSERT one row per recipient with the
    /// share-mount-translated path.
    async fn fan_out_for_share(
        &self,
        filecache: &FileCache,
        recipients: Vec<UserId>,
        owner_uid: &str,
        owner_subroot_path: &str,
        recipient_path_prefix: &str,
    ) -> Result<(), SearchError>;

    /// Inverse: walk the same subroot and DELETE per (recipient, fileid).
    async fn fan_out_for_unshare(
        &self,
        filecache: &FileCache,
        former_recipients: Vec<UserId>,
        owner_uid: &str,
        owner_subroot_path: &str,
    ) -> Result<(), SearchError>;
}

#[async_trait]
impl SearchFanout for Search {
    /// Walk every filecache row under `owner_subroot_path` in
    /// `owner_storage_id` (passed via `owner_uid` — the parameter name
    /// is historical; we treat it as the storage id) and UPSERT one
    /// search row per recipient with the share-mount-translated path.
    ///
    /// Convention: `owner_uid` is the OWNER's storage id (e.g.
    /// `"local::/var/.../alice/files"`). This matches what
    /// `Shares::create` already has on hand (`req.home_storage_id`).
    ///
    /// Scale: this materializes `recipients.len() × files_under_subroot`
    /// `(viewer, fileid)` rows and pushes them through
    /// [`Search::upsert_many`], batched in chunks of
    /// [`BATCH_CHUNK_SIZE`] rows per statement. A 100-member group
    /// share of a 10k-file folder issues `(100 × 10k) / 500 = 2000`
    /// DB statements instead of the 1M sequential writes the
    /// original one-row-at-a-time implementation produced.
    async fn fan_out_for_share(
        &self,
        filecache: &FileCache,
        recipients: Vec<UserId>,
        owner_uid: &str,
        owner_subroot_path: &str,
        recipient_path_prefix: &str,
    ) -> Result<(), SearchError> {
        if recipients.is_empty() {
            return Ok(());
        }
        let rows = filecache.walk_under(owner_uid, owner_subroot_path).await?;
        let mut batch: Vec<BatchUpsertRow> = Vec::with_capacity(rows.len() * recipients.len());
        for row in rows {
            let owner_path_str = row.path.as_str();
            let viewer_path =
                translate_path(owner_subroot_path, recipient_path_prefix, owner_path_str);
            let basename = std::path::Path::new(&viewer_path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(&row.name)
                .to_string();
            for r in &recipients {
                batch.push(BatchUpsertRow {
                    viewer_uid: r.as_str().to_string(),
                    fileid: row.fileid,
                    storage_id: row.storage_id.clone(),
                    basename: basename.clone(),
                    path: viewer_path.clone(),
                    mime: row.mimetype.as_str().to_string(),
                    mtime: row.mtime as i64,
                    size: row.size as i64,
                });
            }
        }
        self.upsert_many(&batch).await
    }

    /// Inverse: walk the same subroot and DELETE per-(recipient,
    /// fileid). `owner_uid` carries the OWNER's storage id (same
    /// convention as `fan_out_for_share`). Uses
    /// [`Search::delete_many_for_viewer_files`] to batch the
    /// per-pair DELETEs into chunked tuple-IN statements.
    async fn fan_out_for_unshare(
        &self,
        filecache: &FileCache,
        former_recipients: Vec<UserId>,
        owner_uid: &str,
        owner_subroot_path: &str,
    ) -> Result<(), SearchError> {
        if former_recipients.is_empty() {
            return Ok(());
        }
        let rows = filecache.walk_under(owner_uid, owner_subroot_path).await?;
        let mut pairs: Vec<(String, i64)> =
            Vec::with_capacity(rows.len() * former_recipients.len());
        for row in rows {
            for r in &former_recipients {
                pairs.push((r.as_str().to_string(), row.fileid));
            }
        }
        self.delete_many_for_viewer_files(&pairs).await
    }
}

/// Translate an owner-relative path to a viewer-relative path. Given
/// owner_subroot=`/docs` and recipient_prefix=`/from-alice`,
/// owner_path=`/docs/q1/r.docx` becomes `/from-alice/q1/r.docx`.
///
/// Strips leading slashes from both inputs so it's tolerant of the
/// "no leading slash" StoragePath representation as well as the
/// "leading-slash" web-facing form.
///
/// Public so the search indexer's per-write path can reuse the bulk
/// fan-out translation rule.
pub fn translate_path(owner_subroot: &str, recipient_prefix: &str, owner_path: &str) -> String {
    let owner_subroot_trim = owner_subroot.trim_matches('/');
    let owner_path_trim = owner_path.trim_start_matches('/');
    let suffix = if owner_subroot_trim.is_empty() {
        owner_path_trim
    } else if owner_path_trim == owner_subroot_trim {
        ""
    } else if let Some(rest) = owner_path_trim.strip_prefix(&format!("{owner_subroot_trim}/")) {
        rest
    } else {
        owner_path_trim
    };
    let prefix = recipient_prefix.trim_end_matches('/');
    if suffix.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}/{suffix}")
    }
}

/// No-op implementation. Tests / fixtures that don't need search
/// fan-out can pass `Arc::new(NoopSearchFanout)` into `SharesConfig`.
pub struct NoopSearchFanout;

#[async_trait]
impl SearchFanout for NoopSearchFanout {
    async fn fan_out_for_share(
        &self,
        _filecache: &FileCache,
        _recipients: Vec<UserId>,
        _owner_uid: &str,
        _owner_subroot_path: &str,
        _recipient_path_prefix: &str,
    ) -> Result<(), SearchError> {
        Ok(())
    }

    async fn fan_out_for_unshare(
        &self,
        _filecache: &FileCache,
        _former_recipients: Vec<UserId>,
        _owner_uid: &str,
        _owner_subroot_path: &str,
    ) -> Result<(), SearchError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crabcloud_config::test_support::minimal_sqlite_config;
    use crabcloud_db::{core_set, MigrationRunner};

    async fn setup_pool() -> (Arc<DbPool>, tempfile::TempDir) {
        let db_dir = tempfile::TempDir::new().unwrap();
        let cfg = minimal_sqlite_config(db_dir.path().join("test.db"));
        let pool = DbPool::connect(&cfg).await.unwrap();
        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();
        (Arc::new(pool), db_dir)
    }

    fn sample_row(viewer: &str, fileid: i64, name: &str) -> BatchUpsertRow {
        BatchUpsertRow {
            viewer_uid: viewer.to_string(),
            fileid,
            storage_id: "s".to_string(),
            basename: name.to_string(),
            path: format!("/{name}"),
            mime: "text/plain".to_string(),
            mtime: 1_700_000_000,
            size: 1,
        }
    }

    #[tokio::test]
    async fn upsert_many_matches_sequential_singles() {
        // Two parallel pools — drive one through `upsert_many` and the
        // other through repeated `upsert_for_file`, then assert the
        // resulting rows are identical via the public `query` API.
        let (pool_batch, _d1) = setup_pool().await;
        let (pool_seq, _d2) = setup_pool().await;
        let batch_search = Search::new(pool_batch);
        let seq_search = Search::new(pool_seq);

        let rows = vec![
            sample_row("alice", 1, "alpha.txt"),
            sample_row("alice", 2, "beta.txt"),
            sample_row("bob", 1, "alpha.txt"),
            sample_row("bob", 2, "beta.txt"),
        ];

        batch_search.upsert_many(&rows).await.unwrap();
        for r in &rows {
            seq_search
                .upsert_for_file(
                    &r.viewer_uid,
                    r.fileid,
                    &r.storage_id,
                    &r.basename,
                    &r.path,
                    &r.mime,
                    r.mtime,
                    r.size,
                )
                .await
                .unwrap();
        }

        for viewer in ["alice", "bob"] {
            let bh = batch_search
                .query(viewer, &crate::parse_query("txt"), 100, None)
                .await
                .unwrap();
            let sh = seq_search
                .query(viewer, &crate::parse_query("txt"), 100, None)
                .await
                .unwrap();
            // Same fileids should appear for both flows.
            let bids: std::collections::BTreeSet<_> = bh.iter().map(|h| h.fileid).collect();
            let sids: std::collections::BTreeSet<_> = sh.iter().map(|h| h.fileid).collect();
            assert_eq!(bids, sids, "viewer={viewer}");
            assert_eq!(bh.len(), 2, "viewer={viewer}");
        }
    }

    #[tokio::test]
    async fn upsert_many_updates_existing_rows() {
        let (pool, _d) = setup_pool().await;
        let search = Search::new(pool);
        // Seed.
        search
            .upsert_many(&[sample_row("alice", 1, "old.txt")])
            .await
            .unwrap();
        // Overwrite via batched upsert (different basename).
        let mut replacement = sample_row("alice", 1, "new.txt");
        replacement.mtime = 1_700_000_100;
        search.upsert_many(&[replacement]).await.unwrap();

        let new_hits = search
            .query("alice", &crate::parse_query("new"), 10, None)
            .await
            .unwrap();
        assert_eq!(new_hits.len(), 1);
        assert_eq!(new_hits[0].basename, "new.txt");
        let stale = search
            .query("alice", &crate::parse_query("old"), 10, None)
            .await
            .unwrap();
        assert!(stale.is_empty(), "old row should have been replaced");
    }

    #[tokio::test]
    async fn delete_many_for_viewer_files_targets_only_listed_pairs() {
        let (pool, _d) = setup_pool().await;
        let search = Search::new(pool);
        let rows = vec![
            sample_row("alice", 1, "a.txt"),
            sample_row("alice", 2, "b.txt"),
            sample_row("bob", 1, "a.txt"),
            sample_row("bob", 2, "b.txt"),
        ];
        search.upsert_many(&rows).await.unwrap();

        // Delete only (alice, 1) and (bob, 2).
        search
            .delete_many_for_viewer_files(&[("alice".to_string(), 1), ("bob".to_string(), 2)])
            .await
            .unwrap();

        let alice_hits = search
            .query("alice", &crate::parse_query("txt"), 10, None)
            .await
            .unwrap();
        let alice_ids: std::collections::BTreeSet<_> =
            alice_hits.iter().map(|h| h.fileid).collect();
        assert_eq!(alice_ids, [2].into_iter().collect());

        let bob_hits = search
            .query("bob", &crate::parse_query("txt"), 10, None)
            .await
            .unwrap();
        let bob_ids: std::collections::BTreeSet<_> = bob_hits.iter().map(|h| h.fileid).collect();
        assert_eq!(bob_ids, [1].into_iter().collect());
    }

    #[tokio::test]
    async fn upsert_many_empty_is_noop() {
        let (pool, _d) = setup_pool().await;
        let search = Search::new(pool);
        search.upsert_many(&[]).await.unwrap();
        search.delete_many_for_viewer_files(&[]).await.unwrap();
    }

    #[tokio::test]
    async fn upsert_many_chunks_larger_than_batch_size() {
        // Validate the chunking loop: push more than BATCH_CHUNK_SIZE
        // rows and confirm they all land.
        let (pool, _d) = setup_pool().await;
        let search = Search::new(pool);
        let n = BATCH_CHUNK_SIZE + 50;
        let rows: Vec<_> = (0..n as i64)
            .map(|i| sample_row("alice", i + 1, &format!("file{i}.txt")))
            .collect();
        search.upsert_many(&rows).await.unwrap();
        let hits = search
            .query("alice", &crate::parse_query("file0"), 5, None)
            .await
            .unwrap();
        // At least one of the seeded rows is reachable; confirm the
        // batched flow committed.
        assert!(!hits.is_empty());
    }

    #[test]
    fn translate_path_replaces_subroot_prefix() {
        assert_eq!(
            translate_path("/docs", "/from-alice", "/docs/q1/report.docx"),
            "/from-alice/q1/report.docx",
        );
    }

    #[test]
    fn translate_path_handles_root_owner_subroot() {
        assert_eq!(
            translate_path("/", "/from-alice", "/q1/report.docx"),
            "/from-alice/q1/report.docx",
        );
    }

    #[test]
    fn translate_path_handles_trailing_slash_in_prefix() {
        assert_eq!(
            translate_path("/docs", "/from-alice/", "/docs/r.txt"),
            "/from-alice/r.txt",
        );
    }

    #[test]
    fn translate_path_handles_subroot_exact_match() {
        assert_eq!(
            translate_path("/docs", "/from-alice", "/docs"),
            "/from-alice",
        );
    }

    #[test]
    fn translate_path_tolerates_no_leading_slash_in_owner_path() {
        // StoragePath stores paths without leading slash.
        assert_eq!(
            translate_path("/docs", "/from-alice", "docs/r.txt"),
            "/from-alice/r.txt",
        );
    }
}
