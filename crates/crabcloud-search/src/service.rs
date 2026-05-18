//! `Search` — query / upsert / delete / fan-out.
//!
//! Spec sections §4 (schema), §5 (surface contracts), §6 (edge cases).
//! Per-dialect dispatch via `match self.pool.as_ref()`. sqlite uses
//! FTS5; mysql uses FULLTEXT NATURAL LANGUAGE MODE; postgres uses
//! tsvector + plainto_tsquery + ts_rank_cd.

use crate::error::SearchError;
use crate::sql;
use crate::types::{SearchHit, SearchQuery};
use async_trait::async_trait;
use crabcloud_db::DbPool;
use crabcloud_filecache::FileCache;
use crabcloud_users::UserId;
use sqlx::Row as _;
use std::sync::Arc;

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
            match_expr.push_str(&q.text);
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
                self.upsert_for_file(
                    r.as_str(),
                    row.fileid,
                    &row.storage_id,
                    &basename,
                    &viewer_path,
                    row.mimetype.as_str(),
                    row.mtime as i64,
                    row.size as i64,
                )
                .await?;
            }
        }
        Ok(())
    }

    /// Inverse: walk the same subroot and DELETE per-(recipient,
    /// fileid). `owner_uid` carries the OWNER's storage id (same
    /// convention as `fan_out_for_share`).
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
        for row in rows {
            for r in &former_recipients {
                self.delete_for_viewer_file(r.as_str(), row.fileid).await?;
            }
        }
        Ok(())
    }
}

/// Translate an owner-relative path to a viewer-relative path. Given
/// owner_subroot=`/docs` and recipient_prefix=`/from-alice`,
/// owner_path=`/docs/q1/r.docx` becomes `/from-alice/q1/r.docx`.
///
/// Strips leading slashes from both inputs so it's tolerant of the
/// "no leading slash" StoragePath representation as well as the
/// "leading-slash" web-facing form.
fn translate_path(owner_subroot: &str, recipient_prefix: &str, owner_path: &str) -> String {
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
