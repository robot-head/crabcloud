# File Cache + Async Scanner (Sub-project 4b)

**Date:** 2026-05-12
**Status:** Brainstormed; awaiting user approval before plan-writing.

## 1. Goal

Mirror 4a's storage state in `oc_filecache` so subsequent sub-projects (5: WebDAV) can serve `stat`/`list` in O(1) and so desktop sync clients see Nextcloud-compatible ETag propagation. Add an async event consumer (`ChannelEventSink` + `Scanner`) that subscribes to 4a's storage events and writes through the cache.

## 2. Why now / who asked

Sub-project 4a shipped `Storage` + two backends + `EventSink` trait + `NoopEventSink`. 4a's `stat`/`list` go straight to the backend on every call — fine for tests, unworkable for a real WebDAV server. 4b makes those calls cheap by routing through a DB cache. Together with 4a, 4b makes WebDAV in sub-project 5 architecturally possible.

## 3. Scope

**In scope:**

- New `crabcloud-filecache` workspace crate.
- New `ChannelEventSink` in `crabcloud-storage` (existing crate).
- Migration `0003_filecache` (sqlite/mysql/postgres) creating `oc_filecache`, `oc_storages`, `oc_mimetypes`.
- `FileCache` façade: `stat`/`list`/`apply`/`lookup`/`lookup_by_id`/`stamp_last_checked`.
- Cache-miss populate path with per-path lock serialization.
- Ancestor size + ETag propagation in a single DB transaction on every mutation.
- `Scanner`: continuous consumer of `ChannelEventSink` + on-demand full-scan + lag-recovery.
- `files:scan <storage_id>` CLI subcommand wired into `crabcloud-server`'s clap.
- `[filecache]` config block in `crabcloud-config`.
- `AppState` extended with `storage_sink`, `filecache`, `scanner`.
- Multi-dialect integration tests (sqlite/mysql/postgres).

**Out of scope (deferred):**

- S3 backend — sub-project **4b-S3** (own brainstorming).
- Mount composition / `View` layer — sub-project **4c**.
- Chunked-upload protocol translation — sub-project **4c**.
- WebDAV / HTTP routes — sub-project **5**.
- Mount-aware permissions composition — 4c + sharing sub-project.
- Trash, versions, WebDAV LOCK/UNLOCK — separate later sub-projects.
- Encryption hooks — separate later sub-project.
- `checksum` column — populated by a future checksum sub-project; 4b ships the column NULL.

## 4. Load-bearing decisions

- **Match Nextcloud upstream cache strategy:** write-through everything. Every storage event in one DB tx updates the leaf row AND bumps ancestor `size` (by delta) AND replaces ancestor `etag`. Cache-miss on stat populates via a real-backend stat under a per-path lock. Matches what desktop/iOS/Android clients expect.
- **Lazy interning** for `oc_mimetypes` and `oc_storages`: insert on first sight + in-process intern cache for repeat lookups.
- **Per-path mutex map** (`DashMap<(String, StoragePath), Arc<Mutex<()>>>`) prevents thundering-herd populate. Opportunistic cleanup on drop; bounded eviction if monitoring shows growth.
- **Single-process scanner** runs as a tokio task spawned by `AppStateBuilder` when `[filecache] enabled = true` (default). Subscribes to ONE `ChannelEventSink` shared by all storages.
- **Broadcast channel** for events (`tokio::sync::broadcast`, capacity 1024). On `RecvError::Lagged`, scanner falls back to a full-scan of every registered storage.
- **Ancestor missing is an error**, not a silent insert: `apply` returns `FileCacheError::AncestorMissing` and the scanner logs + re-scans the affected subtree. Reason: the populate path always materializes parents top-down, so a missing ancestor implies corruption or a bug.
- **No negative caching** in 4b. Cache miss + backend NotFound returns NotFound; subsequent calls hit the backend again. 4c can add negative-row support if it proves load-bearing.
- **CLI subcommand only**, no auto-startup-scan in 4b. Operators trigger `files:scan` manually. Reasoning: 4b's storages aren't registered automatically (4c's mount/View introduces that); a startup auto-scan with zero storages registered is a no-op.

## 5. Crate + module layout

```
crates/
├── crabcloud-storage/                              MODIFIED
│   └── src/lib.rs                                  + ChannelEventSink struct + impl EventSink
├── crabcloud-filecache/                            NEW
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                                  FileCache facade + public types + re-exports
│       ├── error.rs                                FileCacheError + FileCacheResult
│       ├── schema.rs                               FilecacheRow + sqlx FromRow per dialect
│       ├── mimetypes.rs                            intern_mimetype + type-half helper
│       ├── storages.rs                             intern_storage helper
│       ├── populate.rs                             cache-miss populate (per-path lock)
│       ├── propagate.rs                            ancestor walk + tx-wrapped size/etag bump
│       └── scanner/
│           ├── mod.rs                              Scanner struct + spawn()/register_storage()
│           ├── apply.rs                            StorageEvent -> mutation dispatch
│           ├── full_scan.rs                        BFS walk from root, populate top-down
│           └── cli.rs                              `files:scan` subcommand
├── crabcloud-core/                                 MODIFIED
│   └── src/state.rs                                + storage_sink, filecache, scanner fields on AppState
├── crabcloud-config/                               MODIFIED
│   └── src/lib.rs                                  + FilecacheConfig { enabled, event_channel_capacity }
├── crabcloud-server/                               MODIFIED
│   └── src/cli.rs                                  + files:scan subcommand
└── migrations/core/0003_filecache/                 NEW
    ├── sqlite.sql
    ├── mysql.sql
    └── postgres.sql
```

**Cargo dependencies** for `crabcloud-filecache`:

- `crabcloud-storage` (path)
- `crabcloud-db` (path)
- `crabcloud-cache` (path) — for the path-lock map type re-use, if applicable
- `crabcloud-config` (path) — for `FilecacheConfig`
- `async-trait`, `tokio` (fs/io-util/sync/macros), `sqlx`, `tracing`, `thiserror`, `dashmap`, `hex`, `md-5` (for path_hash).
- Dev: `tempfile`, `testcontainers-modules` (already workspace).

New workspace deps to add: `dashmap = "6"`, `md-5 = "0.11"`.

## 6. Public surface

### 6.1 `ChannelEventSink` (in `crabcloud-storage`)

```rust
pub struct ChannelEventSink {
    tx: tokio::sync::broadcast::Sender<StorageEvent>,
}

impl ChannelEventSink {
    pub fn new(capacity: usize) -> Self;
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<StorageEvent>;
}

#[async_trait::async_trait]
impl EventSink for ChannelEventSink {
    async fn emit(&self, event: StorageEvent);  // Best-effort; failed send (no receivers) silently dropped.
}
```

### 6.2 `FileCache` façade

```rust
pub struct FileCache {
    pool: DbPool,
    populate_locks: DashMap<(String, StoragePath), Arc<Mutex<()>>>,
    storage_ids: DashMap<String, i64>,
    mimetypes: DashMap<String, i64>,
}

impl FileCache {
    pub fn new(pool: DbPool) -> Self;

    pub async fn stat(
        &self,
        storage: &Arc<dyn Storage>,
        path: &StoragePath,
    ) -> FileCacheResult<FileMetadata>;

    pub async fn list(
        &self,
        storage: &Arc<dyn Storage>,
        path: &StoragePath,
    ) -> FileCacheResult<Vec<DirEntry>>;

    pub async fn apply(&self, event: &StorageEvent) -> FileCacheResult<()>;

    pub async fn lookup(
        &self,
        storage_id: &str,
        path: &StoragePath,
    ) -> FileCacheResult<Option<FilecacheRow>>;

    pub async fn lookup_by_id(
        &self,
        fileid: i64,
    ) -> FileCacheResult<Option<FilecacheRow>>;

    pub async fn stamp_last_checked(&self, storage_id: &str) -> FileCacheResult<()>;
}
```

### 6.3 `FilecacheRow`

```rust
pub struct FilecacheRow {
    pub fileid: i64,
    pub storage_id: String,
    pub path: StoragePath,
    pub parent: Option<i64>,
    pub name: String,
    pub kind: FileKind,
    pub mimetype: Mimetype,
    pub size: u64,
    pub mtime: u64,
    pub storage_mtime: u64,
    pub etag: ETag,
    pub permissions: Permissions,
}
```

### 6.4 `Scanner`

```rust
pub struct Scanner {
    cache: Arc<FileCache>,
    storages: DashMap<String, Arc<dyn Storage>>,
    sink: Arc<ChannelEventSink>,
}

impl Scanner {
    pub fn new(cache: Arc<FileCache>, sink: Arc<ChannelEventSink>) -> Self;

    pub fn register_storage(&self, storage: Arc<dyn Storage>);

    pub fn spawn(self: Arc<Self>) -> tokio::task::JoinHandle<()>;

    pub async fn full_scan(&self, storage: &Arc<dyn Storage>) -> FileCacheResult<u64>;
}
```

### 6.5 `FileCacheError`

```rust
#[derive(Debug, thiserror::Error)]
pub enum FileCacheError {
    #[error("not found")]
    NotFound,
    #[error("ancestor missing: {0}")]
    AncestorMissing(StoragePath),
    #[error("storage error: {0}")]
    Storage(#[from] crabcloud_storage::StorageError),
    #[error("db error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("invalid state: {0}")]
    Invalid(String),
}

pub type FileCacheResult<T> = Result<T, FileCacheError>;
```

## 7. Schema (`migrations/core/0003_filecache`)

### 7.1 `oc_storages`

| Column | SQLite | MySQL | Postgres | Notes |
|---|---|---|---|---|
| numeric_id | INTEGER PK AUTOINCREMENT | INT UNSIGNED PK AUTO_INCREMENT | SERIAL PK | FK target |
| id | TEXT UNIQUE NOT NULL | VARCHAR(64) UNIQUE NOT NULL | VARCHAR(64) UNIQUE NOT NULL | `Storage::id()` value |
| available | INTEGER NOT NULL DEFAULT 1 | TINYINT NOT NULL DEFAULT 1 | SMALLINT NOT NULL DEFAULT 1 | future use |
| last_checked | INTEGER NULL | INT NULL | INTEGER NULL | unix ts of last scan |

### 7.2 `oc_mimetypes`

| Column | SQLite | MySQL | Postgres | Notes |
|---|---|---|---|---|
| id | INTEGER PK AUTOINCREMENT | INT UNSIGNED PK AUTO_INCREMENT | SERIAL PK | FK target |
| mimetype | TEXT UNIQUE NOT NULL | VARCHAR(255) UNIQUE NOT NULL | VARCHAR(255) UNIQUE NOT NULL | full `"image/png"` and type-half `"image"` both stored |

### 7.3 `oc_filecache`

| Column | SQLite | MySQL | Postgres | Notes |
|---|---|---|---|---|
| fileid | INTEGER PK AUTOINCREMENT | BIGINT UNSIGNED PK AUTO_INCREMENT | BIGSERIAL PK | system-wide unique |
| storage | INTEGER NOT NULL | INT UNSIGNED NOT NULL | INT NOT NULL | FK -> `oc_storages.numeric_id`, ON DELETE CASCADE |
| path | TEXT NOT NULL | VARCHAR(4000) NOT NULL | VARCHAR(4000) NOT NULL | `StoragePath::as_str()` |
| path_hash | TEXT NOT NULL | CHAR(32) NOT NULL | CHAR(32) NOT NULL | hex(md5(path)); unique-indexed with storage |
| parent | INTEGER NULL | BIGINT UNSIGNED NULL | BIGINT NULL | self-FK; NULL for root; ON DELETE CASCADE |
| name | TEXT NOT NULL | VARCHAR(250) NOT NULL | VARCHAR(250) NOT NULL | basename only |
| mimetype | INTEGER NOT NULL | INT UNSIGNED NOT NULL | INT NOT NULL | FK -> `oc_mimetypes.id`, ON DELETE RESTRICT |
| mimepart | INTEGER NOT NULL | INT UNSIGNED NOT NULL | INT NOT NULL | FK -> `oc_mimetypes.id`, ON DELETE RESTRICT |
| size | INTEGER NOT NULL DEFAULT 0 | BIGINT NOT NULL DEFAULT 0 | BIGINT NOT NULL DEFAULT 0 | bytes; aggregated for directories |
| mtime | INTEGER NOT NULL DEFAULT 0 | INT UNSIGNED NOT NULL DEFAULT 0 | INTEGER NOT NULL DEFAULT 0 | server-observed unix sec |
| storage_mtime | INTEGER NOT NULL DEFAULT 0 | INT UNSIGNED NOT NULL DEFAULT 0 | INTEGER NOT NULL DEFAULT 0 | backend-observed unix sec |
| encrypted | INTEGER NOT NULL DEFAULT 0 | TINYINT NOT NULL DEFAULT 0 | SMALLINT NOT NULL DEFAULT 0 | always 0 in 4b |
| etag | TEXT NOT NULL | VARCHAR(40) NOT NULL | VARCHAR(40) NOT NULL | 40-char hex |
| permissions | INTEGER NOT NULL DEFAULT 0 | INT UNSIGNED NOT NULL DEFAULT 0 | INTEGER NOT NULL DEFAULT 0 | `Permissions::bits()` |
| checksum | TEXT NULL | VARCHAR(255) NULL | VARCHAR(255) NULL | always NULL in 4b |

### 7.4 Indexes

```sql
CREATE UNIQUE INDEX fs_storage_path ON oc_filecache (storage, path_hash);
CREATE INDEX fs_parent ON oc_filecache (parent);
CREATE INDEX fs_mimepart ON oc_filecache (mimepart);
CREATE INDEX fs_mimetype ON oc_filecache (mimetype);
CREATE INDEX fs_storage_size ON oc_filecache (storage, size);
```

All `IF NOT EXISTS` so migration is idempotent.

## 8. Cache-miss populate algorithm

For `FileCache::stat(storage, path)`:

1. **Cache hit?** SELECT row by `(storage_id, path_hash)`. If present, return.
2. **Acquire path lock.** `populate_locks.entry((storage_id, path)).or_insert_with(|| Arc::new(Mutex::new(())))` → clone Arc → lock. Reuse existing locks under contention.
3. **Re-check cache under lock.** Another task may have populated while we waited.
4. **Backend stat.** `storage.stat(path).await?`. NotFound → drop lock + return NotFound. (No negative caching.)
5. **Recurse parent.** If `path.parent().is_some_and(|p| !p.is_root())`, `self.stat(storage, &parent).await?` to ensure parent is cached. Returns NotFound if parent doesn't exist → propagate.
6. **Intern.** `intern_storage(storage.id())` → `numeric_id`; `intern_mimetype(meta.mimetype.as_str())` + `intern_mimetype(type_half)` → both ids.
7. **INSERT.** Single `INSERT INTO oc_filecache (...) VALUES (...) RETURNING fileid`.
8. **Cleanup.** `if Arc::strong_count(&lock) == 1 { populate_locks.remove(...); }`. Opportunistic.

For `FileCache::list(storage, path)`: populate directory if missing, then populate every child (one level), then return.

## 9. Ancestor propagation algorithm

For `FileCache::apply(event)` on a mutating event, in one transaction:

1. Compute `delta_size` (new − old) and resolve target row's existing fileid (if any).
2. UPSERT the leaf row.
3. Walk ancestors from `path.parent()` up to storage root. For each:
   - `SELECT fileid FROM oc_filecache WHERE storage = ? AND path_hash = ?`. NotFound → `FileCacheError::AncestorMissing(path)`.
   - `UPDATE oc_filecache SET size = size + ?, etag = ?, mtime = ? WHERE fileid = ?` with `delta_size`, `ETag::new()`, `event_mtime`.
4. Commit.

Per-dialect SQL via the existing `db_dispatch!` macro pattern in `crabcloud-db`.

## 10. Event handlers

| Event | Cache mutation |
|---|---|
| `Written { storage_id, path, metadata }` | Upsert leaf; propagate size delta + fresh etag on ancestors |
| `DirCreated { storage_id, path, metadata }` | Insert leaf (kind=Directory, size=0); propagate fresh etag on ancestors (no size change) |
| `Deleted { storage_id, path }` | DELETE leaf (cascade kicks in for directories via parent FK); propagate `-old_size` on ancestors |
| `Moved { storage_id, from, to }` | UPDATE leaf's `parent`/`path`/`name`/`path_hash`. **If leaf is a directory**, also rewrite every descendant's `path` and `path_hash` (a single `UPDATE ... WHERE storage = ? AND path LIKE 'old_prefix/%'` per dialect). If `from.parent() != to.parent()`, propagate `-old_size` on source chain AND `+old_size` on dest chain; else single-chain etag bump |
| `Copied { storage_id, from, to }` | INSERT new leaf at `to` with fresh etag (copying from `from`'s row); propagate `+new_size` on dest chain |

## 11. Scanner

### 11.1 Continuous consumer

Loop: `rx.recv()` → match.
- `Ok(event)` → `cache.apply(&event).await`; on error log + `full_scan` the affected storage.
- `Err(Lagged(n))` → log + full-scan every registered storage.
- `Err(Closed)` → exit (clean shutdown).

### 11.2 Full scan

BFS from `StoragePath::root()`. For each path:
- `storage.stat(&path)` — backend.
- `cache.stat(storage, &path)` — populates the row.
- If directory, `storage.list(&path)` — backend; enqueue each child.

At the end, `cache.stamp_last_checked(storage.id())`.

### 11.3 CLI

`crabcloud files:scan <storage_id>`. Resolves the storage via `scanner.storages.get(storage_id)`; errors if unknown.

## 12. AppState + config

### 12.1 Config block

```toml
[filecache]
enabled = true
event_channel_capacity = 1024
```

`FilecacheConfig { enabled: bool, event_channel_capacity: usize }` added to `crabcloud-config`. Defaults: `{ true, 1024 }`.

### 12.2 AppState

New `AppState` fields:
- `storage_sink: Arc<ChannelEventSink>`
- `filecache: Arc<FileCache>`
- `scanner: Arc<Scanner>`

`AppStateBuilder::build`:

```rust
let storage_sink = Arc::new(ChannelEventSink::new(config.filecache.event_channel_capacity));
let filecache = Arc::new(FileCache::new(pool.clone()));
let scanner = Arc::new(Scanner::new(filecache.clone(), storage_sink.clone()));
if config.filecache.enabled {
    scanner.clone().spawn();
}
```

Existing tests + integration callers that don't care about storage continue to work — `storage_sink` accepts emits from zero senders (no-op when not wired) and the consumer loop just sits idle.

## 13. Test strategy

### 13.1 Integration tests (`crates/crabcloud-filecache/tests/`)

13 tests covering the full event surface + concurrency + scanner:

1. `apply_written_event` — basic insert.
2. `apply_propagates_size_and_etag` — write at depth 3, all ancestors bump.
3. `apply_dir_created` — directory leaf insert.
4. `apply_deleted_cascades` — descendant rows removed.
5. `apply_moved_within_storage` — path/parent update + both chains bumped.
6. `apply_copied` — destination chain bump only.
7. `stat_cache_miss_populates` — first call hits backend; second call doesn't.
8. `stat_cache_miss_concurrent_populates_once` — 100 concurrent stats → 1 backend hit.
9. `stat_cache_miss_distinct_paths_parallel` — 100 distinct paths → 100 backend hits.
10. `lookup_by_id` — read row by fileid.
11. `scanner_consumes_events` — wire ChannelEventSink + Scanner + LocalStorage; put_file; assert cache.
12. `scanner_full_scan_reconciles_drift` — write files directly; full_scan; assert cache.
13. `scanner_lagged_triggers_full_scan` — overflow channel; assert RecvError::Lagged triggers full-scan.

### 13.2 Multi-dialect

Existing `cargo xtask check-all` runs against SQLite + MySQL + Postgres via testcontainers. Migration `0003_filecache` registered in `core_set()` so all three dialects build the schema.

### 13.3 Unit tests

Per-module:
- `mimetypes::tests` — type-half splitting, intern reuse.
- `storages::tests` — intern reuse.
- `propagate::tests` — delta math.
- `populate::tests` — path-lock map eviction.

## 14. Acceptance criteria

| # | Criterion | Verified by |
|---|---|---|
| 1 | `cargo xtask check-all` clean on sqlite/mysql/postgres | CI |
| 2 | Migration `0003_filecache` creates 3 tables + 5 indexes + FKs on all three dialects | migration test |
| 3 | `ChannelEventSink` is `EventSink`; capacity 1024 default | unit |
| 4 | Written event inserts leaf with correct mimetype/size/etag/permissions | integration |
| 5 | Ancestor size + etag propagation is atomic (one transaction) | integration |
| 6 | Cache-miss populate serializes per-path (100 concurrent stats → 1 backend hit) | integration |
| 7 | Cache-miss populate parallelizes across paths (100 distinct → 100 hits) | integration |
| 8 | Scanner consumes broadcast events and applies them | integration |
| 9 | Full-scan reconciles external drift | integration |
| 10 | `RecvError::Lagged` triggers full-scan recovery | integration |
| 11 | `files:scan <storage_id>` CLI command runs full-scan | integration |
| 12 | Deleted directory cascades all descendant rows via FK | integration |
| 13 | Moved row updates path + parent + name + propagates ETag on BOTH chains when parent changes | integration |
| 14 | Workspace `-D warnings` clean | CI |
| 15 | `git grep -i rustcloud` empty | CI |

## 15. Risks + mitigations

- **Path-lock map growth.** `populate_locks` is monotonically inserting; opportunistic cleanup may leak entries under contention. Mitigation: cap at 10K entries with oldest-eviction if Prometheus shows growth; current monitoring isn't in place yet so ship without cap and revisit.
- **Broadcast lag.** A slow scanner can cause `RecvError::Lagged(n)`. Mitigation: full-scan recovery path. Latency cost: one full-scan per lag event.
- **Ancestor missing.** Cache invariant requires every ancestor exists before children. Mitigation: populate always materializes parents first; apply errors loudly + scanner re-scans on error. Diagnostic: structured log line points to the corrupted subtree.
- **`oc_filecache.path` length.** Spec caps at 4000 chars (MySQL/Postgres VARCHAR limit before index width concerns). `StoragePath::new` caps at 4096; there's a 96-char gap. Acceptable for 4b — operators with deeper paths can wait for VARCHAR-extension work or switch to TEXT-typed columns; not load-bearing for the WebDAV path.
- **Cross-storage moves.** Storage trait only supports within-storage rename. 4c will add View-level cross-storage move (copy + delete). 4b's cache layer handles `Moved` only within one storage.
- **External edits between scans.** If an operator edits the FS directly without `files:scan`, cache is stale until next scan. Mitigation: documented in the operator guide (added in Batch F changelog).

## 16. Open questions (deferred)

- **Negative caching.** 4b returns NotFound without remembering it. If WebDAV's PROPFIND on a recursive 404 tree becomes a hot path, add a negative-cache row with TTL.
- **`storage_mtime` propagation up directories.** Spec says no (only leaf's storage_mtime is the backend mtime). Confirm with WebDAV needs in sub-project 5.
- **Scanner concurrency.** Single consumer; events apply in order. If apply latency becomes a bottleneck, parallelize per-storage with a dispatcher.
- **`oc_filecache.parent` rebuild after move.** When a directory moves to a new parent, every descendant row's `parent` is unaffected (it still points to the directory's `fileid`, which itself moved). The descendants' `path` strings DO need updating. Confirm: do we walk the descendant subtree and rewrite each `path` field? Yes — `apply_moved_within_storage` test asserts this. Cost is O(subtree); acceptable since moves of large subtrees are rare.

## 17. Dependencies on other sub-projects

- **Upstream:** 4a (storage trait + LocalStorage + MemoryStorage + EventSink).
- **Downstream:** 4c (mount/View consumes `FileCache::lookup`/`lookup_by_id`); 4b-S3 (registers S3 storages with `Scanner`); sub-project 5 (WebDAV reads through `FileCache`).
