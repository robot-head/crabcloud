# Platform Core — Phase 2: Cross-Cutting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the cross-cutting building blocks every later phase will share — cache, i18n, OCS envelope + capabilities aggregator, runtime app-config service, and a `rustcloud-core` facade defining `AppState`, the unified `Error` type, and the `BootstrapHook` extension point — all unit-tested and provably wired together by an `AppState::build()` integration test.

**Architecture:** Each concern lives in its own focused crate (`rustcloud-cache`, `rustcloud-i18n`, `rustcloud-ocs`), all flowing into `rustcloud-core` which composes them with the Phase 1 crates (`rustcloud-config`, `rustcloud-db`) into a clone-cheap `AppState`. No HTTP, no UI yet — the OCS envelope produces strings + content-type pairs that Phase 3 will wrap in axum `IntoResponse`; the `Error` type ships with status-code mapping (also a pure function) that Phase 3 turns into HTTP responses. The `BootstrapHook` registration vector lands so that future apps (Phase 3+) have an unchanging extension point.

**Tech Stack:** Rust 1.85, `tokio` (Mutex for cache state), `async-trait` (for the `Cache` and `CapabilityProvider` traits), `serde_json` / `quick-xml` (OCS envelope rendering), `polib` (gettext `.po` parsing), `serde` for typed cache wrapper, `chrono` for timestamps (already a sqlx feature), `bytes::Bytes` for cache payloads (or `Vec<u8>` — see §9.1 of spec; we use `Vec<u8>` for ergonomic ownership).

**Parent spec:** `docs/superpowers/specs/2026-05-10-platform-core-design.md` — Phase 2 implements sections §5.2 (runtime app-config), §7.6 (Error type, status-mapping only), §9 (Cache, i18n, OCS envelope + capabilities), and the `AppState` + `BootstrapHook` machinery from §4.1 / §10.1.

**Previous phase:** Phase 1 (Foundations) shipped `rustcloud-config`, `rustcloud-db`, `rustcloud-server`, `xtask`, CI, multi-dialect integration tests. End-state of Phase 1 is at commit `29706dc` on `master`.

---

## Conventions (carry-over from Phase 1)

- **Commits:** Conventional Commits (`feat:`, `chore:`, `test:`, `docs:`). Co-Authored-By trailer with `Claude Opus 4.7 <noreply@anthropic.com>`.
- **TDD:** Write failing test → verify it fails → implement → verify it passes → commit. For brand-new crates the first verification may be a build check; meaningful tests follow immediately.
- **rustfmt:** The plan's verbatim code may have lines that exceed rustfmt's default width. After writing files, run `cargo fmt --all` and commit the formatted version. Authorized at all task boundaries.
- **No mocks for the DB or cache.** Tests hit a real in-process `SqlitePool` and a real `MemoryCache`. Multi-dialect tests for DB-backed code run in CI via the Phase 1 testcontainers/service-container path.
- **Errors:** Library crates expose typed errors via `thiserror`. `rustcloud-core::Error` aggregates errors from sibling crates via `#[from]` and adds variants the HTTP layer (Phase 3) will need.
- **Async traits:** Use `async-trait` (already in `[workspace.dependencies]`). Native async-fn-in-trait is stable on rustc 1.75+ but `Send` bounds require boilerplate; `async-trait` is mature and cheap.
- **Plan-bug protocol:** If the verbatim code from this plan fails to compile or test, fix the minimal issue, report it as DONE_WITH_CONCERNS with the diff explained.

---

## File Structure (Phase 2 additions)

```
rustcloud/
├── Cargo.toml                                     # workspace.members + workspace.dependencies extended
├── crates/
│   ├── rustcloud-cache/                           # NEW
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs                             # re-exports
│   │       ├── trait_def.rs                       # Cache trait
│   │       ├── memory.rs                          # MemoryCache + Entry + lazy TTL expiry
│   │       └── typed.rs                           # TypedCache<T> serde wrapper
│   ├── rustcloud-i18n/                            # NEW
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── locale.rs                          # Locale type + Accept-Language parser
│   │       ├── catalog.rs                         # PO catalog loader (uses polib)
│   │       └── service.rs                         # I18n struct, t() / tn() with fallback
│   ├── rustcloud-ocs/                             # NEW
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── status.rs                          # OcsStatus enum + status codes
│   │       ├── envelope.rs                        # OcsResponse<T> + render (JSON+XML)
│   │       ├── format.rs                          # Format + content negotiation
│   │       ├── capabilities.rs                    # CapabilityProvider trait + aggregator + ETag
│   │       └── core_caps.rs                       # CoreCapabilities impl
│   ├── rustcloud-core/                            # NEW
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── error.rs                           # Error enum + status mapping
│   │       ├── appconfig.rs                       # AppConfigService (cache-backed)
│   │       ├── bootstrap.rs                       # BootstrapHook type + registry
│   │       └── state.rs                           # AppState struct + build()
│   │   └── tests/
│   │       └── app_state_build.rs                 # integration test for full assembly
│   └── rustcloud-server/                          # MODIFIED
│       └── src/main.rs                            # bootstrap path uses AppState::build()
├── l10n/                                          # NEW (translations root)
│   └── core/
│       ├── README.md
│       └── de.po                                  # seed German translations for tests
└── docs/superpowers/plans/2026-05-10-platform-core-phase-2-cross-cutting.changelog.md
```

Updates to `[workspace.dependencies]` (Task 1 of each crate-adding batch):
- `quick-xml = { version = "0.36", features = ["serialize"] }` — for OCS XML emission
- `polib = "0.4"` — gettext `.po` parser, pure Rust
- `parking_lot = "0.12"` — *if* we choose `parking_lot::RwLock` over `tokio::sync::Mutex` for the cache (decision: use `tokio::sync::Mutex` — keep it async, no extra dep)
- `accept-language = "3.1"` — header parser (kept simple; or we roll our own — see Task 5)

We'll add **only** `quick-xml` and `polib` and `accept-language` to workspace deps; cache uses `tokio::sync::Mutex` from the existing `tokio` dep.

---

## Task 1: rustcloud-cache — workspace setup + Cache trait

**Files:**
- Create: `crates/rustcloud-cache/Cargo.toml`
- Create: `crates/rustcloud-cache/src/lib.rs`
- Create: `crates/rustcloud-cache/src/trait_def.rs`
- Modify: `Cargo.toml` (add `crates/rustcloud-cache` to `members`; add `rustcloud-cache = { path = "crates/rustcloud-cache" }` under `[workspace.dependencies]`)

The `Cache` trait is the core abstraction; implementations live in their own modules. Phase 2 ships `MemoryCache`; Redis lands in its own micro-sub-project later.

- [ ] **Step 1: Add `crates/rustcloud-cache` to the workspace**

Modify `Cargo.toml` — append `crates/rustcloud-cache` to `members` (keep alphabetical order with the existing entries):

```toml
[workspace]
members = [
    "crates/rustcloud-cache",
    "crates/rustcloud-config",
    "crates/rustcloud-db",
    "crates/rustcloud-server",
    "xtask",
]
```

Under `[workspace.dependencies]`, add (alphabetical):

```toml
rustcloud-cache  = { path = "crates/rustcloud-cache" }
```

- [ ] **Step 2: Write `crates/rustcloud-cache/Cargo.toml`**

```toml
[package]
name = "rustcloud-cache"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
async-trait.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tokio.workspace = true

[dev-dependencies]
tokio = { workspace = true, features = ["macros", "rt-multi-thread", "time", "test-util"] }
```

- [ ] **Step 3: Write `crates/rustcloud-cache/src/trait_def.rs`**

```rust
//! Cache trait. See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §9.1.

use async_trait::async_trait;
use std::time::Duration;

/// Cache errors. Most are I/O-shaped; future backends (Redis) may surface
/// transport errors. The memory backend returns only `CasMismatch`.
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("compare-and-swap failed: value did not match")]
    CasMismatch,
    #[error("cache I/O error: {0}")]
    Io(String),
}

pub type CacheResult<T> = Result<T, CacheError>;

/// Bytes-in, bytes-out cache. Callers handle serialization; the `TypedCache<T>`
/// wrapper in this crate provides a typed serde-backed convenience layer.
#[async_trait]
pub trait Cache: Send + Sync {
    /// Returns `None` if the key is missing or expired.
    async fn get(&self, key: &str) -> CacheResult<Option<Vec<u8>>>;

    /// Sets a key. `ttl = None` means no expiry.
    async fn set(&self, key: &str, value: &[u8], ttl: Option<Duration>) -> CacheResult<()>;

    /// Deletes a key. No error if absent.
    async fn del(&self, key: &str) -> CacheResult<()>;

    /// Atomic numeric increment. Returns the new value. If the key is absent,
    /// treats it as `0` and writes `by` (or sets to `by` for negative `by`).
    async fn incr(&self, key: &str, by: i64) -> CacheResult<i64>;

    /// Compare-and-swap. Sets `new` only if the current value equals `old`.
    /// Returns `Ok(true)` on success, `Ok(false)` on mismatch (no error).
    async fn cas(&self, key: &str, old: &[u8], new: &[u8]) -> CacheResult<bool>;
}
```

- [ ] **Step 4: Write `crates/rustcloud-cache/src/lib.rs`**

```rust
//! Cache abstraction for Rustcloud.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §9.1.

mod memory;
mod trait_def;
mod typed;

pub use memory::MemoryCache;
pub use trait_def::{Cache, CacheError, CacheResult};
pub use typed::TypedCache;
```

(`memory` and `typed` are added in Tasks 2 and 3; create empty placeholder files now so this `mod` declaration compiles.)

Create `crates/rustcloud-cache/src/memory.rs`:
```rust
// Implemented in Task 2.
```

Create `crates/rustcloud-cache/src/typed.rs`:
```rust
// Implemented in Task 3.
```

- [ ] **Step 5: Verify the crate compiles**

Run:
```
cargo build -p rustcloud-cache
```

Expected: the crate compiles. (No tests yet; the trait is dead code but `pub` items are fine.)

- [ ] **Step 6: Commit**

```
git add Cargo.toml crates/rustcloud-cache
git commit -m "feat(cache): add Cache trait and crate scaffolding

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 2: rustcloud-cache — MemoryCache implementation

**Files:**
- Modify: `crates/rustcloud-cache/src/memory.rs` (replace placeholder)

The `MemoryCache` uses `tokio::sync::Mutex<HashMap<String, Entry>>`. TTL expiry is **lazy on read** — no background sweeper. The spec §9.1 mentions a sweeper but explicitly notes "Memory backend works for single-node dev"; lazy expiry is sufficient and simpler. Background sweeping is a Phase 3+ optimization.

- [ ] **Step 1: Write the failing tests**

Write `crates/rustcloud-cache/src/memory.rs`:

```rust
//! In-process `Cache` implementation. Single-node use only.

use crate::trait_def::{Cache, CacheError, CacheResult};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
struct Entry {
    value: Vec<u8>,
    expires_at: Option<Instant>,
}

impl Entry {
    fn is_expired(&self, now: Instant) -> bool {
        self.expires_at.is_some_and(|t| t <= now)
    }
}

#[derive(Debug, Clone, Default)]
pub struct MemoryCache {
    inner: Arc<Mutex<HashMap<String, Entry>>>,
}

impl MemoryCache {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl Cache for MemoryCache {
    async fn get(&self, key: &str) -> CacheResult<Option<Vec<u8>>> {
        let mut g = self.inner.lock().await;
        let now = Instant::now();
        if let Some(entry) = g.get(key) {
            if entry.is_expired(now) {
                g.remove(key);
                return Ok(None);
            }
            return Ok(Some(entry.value.clone()));
        }
        Ok(None)
    }

    async fn set(&self, key: &str, value: &[u8], ttl: Option<Duration>) -> CacheResult<()> {
        let expires_at = ttl.map(|d| Instant::now() + d);
        let entry = Entry { value: value.to_vec(), expires_at };
        self.inner.lock().await.insert(key.to_string(), entry);
        Ok(())
    }

    async fn del(&self, key: &str) -> CacheResult<()> {
        self.inner.lock().await.remove(key);
        Ok(())
    }

    async fn incr(&self, key: &str, by: i64) -> CacheResult<i64> {
        let mut g = self.inner.lock().await;
        let now = Instant::now();
        let current = match g.get(key) {
            Some(entry) if !entry.is_expired(now) => {
                std::str::from_utf8(&entry.value)
                    .map_err(|e| CacheError::Io(format!("incr: value not utf-8: {e}")))?
                    .parse::<i64>()
                    .map_err(|e| CacheError::Io(format!("incr: value not i64: {e}")))?
            }
            _ => 0,
        };
        let new = current.saturating_add(by);
        let entry = Entry { value: new.to_string().into_bytes(), expires_at: None };
        g.insert(key.to_string(), entry);
        Ok(new)
    }

    async fn cas(&self, key: &str, old: &[u8], new: &[u8]) -> CacheResult<bool> {
        let mut g = self.inner.lock().await;
        let now = Instant::now();
        let matches = match g.get(key) {
            Some(entry) if !entry.is_expired(now) => entry.value == old,
            _ => false,
        };
        if matches {
            let entry = Entry { value: new.to_vec(), expires_at: None };
            g.insert(key.to_string(), entry);
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{sleep, Duration as TokioDuration};

    #[tokio::test]
    async fn get_missing_returns_none() {
        let c = MemoryCache::new();
        assert!(c.get("absent").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn set_then_get_roundtrips() {
        let c = MemoryCache::new();
        c.set("k", b"v", None).await.unwrap();
        assert_eq!(c.get("k").await.unwrap(), Some(b"v".to_vec()));
    }

    #[tokio::test]
    async fn ttl_expires_on_read() {
        // Use a short real TTL because tokio::time::pause won't advance Instant::now
        // (which our Entry uses). Trade a tiny wall-clock wait for simplicity.
        let c = MemoryCache::new();
        c.set("k", b"v", Some(Duration::from_millis(20))).await.unwrap();
        assert_eq!(c.get("k").await.unwrap(), Some(b"v".to_vec()));
        sleep(TokioDuration::from_millis(40)).await;
        assert!(c.get("k").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn del_removes_key() {
        let c = MemoryCache::new();
        c.set("k", b"v", None).await.unwrap();
        c.del("k").await.unwrap();
        assert!(c.get("k").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn incr_from_absent() {
        let c = MemoryCache::new();
        assert_eq!(c.incr("n", 1).await.unwrap(), 1);
        assert_eq!(c.incr("n", 4).await.unwrap(), 5);
    }

    #[tokio::test]
    async fn incr_rejects_non_numeric_value() {
        let c = MemoryCache::new();
        c.set("n", b"hello", None).await.unwrap();
        let err = c.incr("n", 1).await.unwrap_err();
        assert!(matches!(err, CacheError::Io(_)));
    }

    #[tokio::test]
    async fn cas_succeeds_when_value_matches() {
        let c = MemoryCache::new();
        c.set("k", b"a", None).await.unwrap();
        assert!(c.cas("k", b"a", b"b").await.unwrap());
        assert_eq!(c.get("k").await.unwrap(), Some(b"b".to_vec()));
    }

    #[tokio::test]
    async fn cas_returns_false_when_value_mismatches() {
        let c = MemoryCache::new();
        c.set("k", b"a", None).await.unwrap();
        assert!(!c.cas("k", b"WRONG", b"b").await.unwrap());
        assert_eq!(c.get("k").await.unwrap(), Some(b"a".to_vec()));
    }

    #[tokio::test]
    async fn cas_returns_false_when_key_absent() {
        let c = MemoryCache::new();
        assert!(!c.cas("k", b"a", b"b").await.unwrap());
    }

    #[tokio::test]
    async fn clones_share_state() {
        let c1 = MemoryCache::new();
        let c2 = c1.clone();
        c1.set("k", b"v", None).await.unwrap();
        assert_eq!(c2.get("k").await.unwrap(), Some(b"v".to_vec()));
    }
}
```

- [ ] **Step 2: Run the tests**

```
cargo test -p rustcloud-cache --lib
```

Expected: 10 tests pass.

- [ ] **Step 3: Commit**

```
git add crates/rustcloud-cache/src/memory.rs
git commit -m "feat(cache): add MemoryCache with lazy TTL expiry

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 3: rustcloud-cache — TypedCache<T> serde wrapper

**Files:**
- Modify: `crates/rustcloud-cache/src/typed.rs` (replace placeholder)

A thin generic wrapper that handles serde + key prefixing. Callers wrap any `Cache` impl with a typed view:

```rust
let users_cache: TypedCache<User> = TypedCache::new(cache.clone(), "users:");
users_cache.set("alice", &User { ... }, ttl).await?;
let alice: Option<User> = users_cache.get("alice").await?;
```

- [ ] **Step 1: Write the failing tests and the impl**

Write `crates/rustcloud-cache/src/typed.rs`:

```rust
//! Typed `serde`-backed convenience wrapper around any `Cache` impl.

use crate::trait_def::{Cache, CacheError, CacheResult};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

/// Wraps a `Cache` with a key prefix and serde (de)serialization.
///
/// Keys passed to `get`/`set`/`del` are concatenated with `prefix` before hitting the
/// underlying cache. Values are JSON-encoded for portability across cache backends.
pub struct TypedCache<T> {
    inner: Arc<dyn Cache>,
    prefix: String,
    _marker: PhantomData<fn() -> T>,
}

impl<T> TypedCache<T> {
    pub fn new(inner: Arc<dyn Cache>, prefix: impl Into<String>) -> Self {
        Self { inner, prefix: prefix.into(), _marker: PhantomData }
    }

    fn full_key(&self, key: &str) -> String {
        format!("{}{}", self.prefix, key)
    }
}

impl<T: Serialize + DeserializeOwned + Send + Sync> TypedCache<T> {
    pub async fn get(&self, key: &str) -> CacheResult<Option<T>> {
        let raw = self.inner.get(&self.full_key(key)).await?;
        match raw {
            None => Ok(None),
            Some(bytes) => {
                let v = serde_json::from_slice::<T>(&bytes)
                    .map_err(|e| CacheError::Io(format!("typed get decode: {e}")))?;
                Ok(Some(v))
            }
        }
    }

    pub async fn set(&self, key: &str, value: &T, ttl: Option<Duration>) -> CacheResult<()> {
        let bytes = serde_json::to_vec(value)
            .map_err(|e| CacheError::Io(format!("typed set encode: {e}")))?;
        self.inner.set(&self.full_key(key), &bytes, ttl).await
    }

    pub async fn del(&self, key: &str) -> CacheResult<()> {
        self.inner.del(&self.full_key(key)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryCache;
    use serde::{Deserialize, Serialize};
    use std::sync::Arc;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct User {
        name: String,
        age: u32,
    }

    fn mk() -> TypedCache<User> {
        let cache: Arc<dyn Cache> = Arc::new(MemoryCache::new());
        TypedCache::new(cache, "users:")
    }

    #[tokio::test]
    async fn get_missing_returns_none() {
        let c = mk();
        assert!(c.get("absent").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn set_then_get_roundtrips() {
        let c = mk();
        let u = User { name: "alice".into(), age: 30 };
        c.set("alice", &u, None).await.unwrap();
        assert_eq!(c.get("alice").await.unwrap(), Some(u));
    }

    #[tokio::test]
    async fn keys_are_prefix_namespaced() {
        let cache: Arc<dyn Cache> = Arc::new(MemoryCache::new());
        let users = TypedCache::<User>::new(cache.clone(), "users:");
        let admins = TypedCache::<User>::new(cache.clone(), "admins:");
        let u_alice = User { name: "alice".into(), age: 30 };
        let a_alice = User { name: "alice".into(), age: 99 };
        users.set("alice", &u_alice, None).await.unwrap();
        admins.set("alice", &a_alice, None).await.unwrap();
        assert_eq!(users.get("alice").await.unwrap(), Some(u_alice));
        assert_eq!(admins.get("alice").await.unwrap(), Some(a_alice));
    }

    #[tokio::test]
    async fn del_removes_typed_value() {
        let c = mk();
        let u = User { name: "alice".into(), age: 30 };
        c.set("alice", &u, None).await.unwrap();
        c.del("alice").await.unwrap();
        assert!(c.get("alice").await.unwrap().is_none());
    }
}
```

- [ ] **Step 2: Run the tests**

```
cargo test -p rustcloud-cache --lib
```

Expected: 14 tests pass (10 from memory + 4 typed).

- [ ] **Step 3: Commit**

```
git add crates/rustcloud-cache/src/typed.rs
git commit -m "feat(cache): add TypedCache<T> serde wrapper with key prefixing

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 4: rustcloud-i18n — workspace setup + Locale + Accept-Language parser

**Files:**
- Create: `crates/rustcloud-i18n/Cargo.toml`
- Create: `crates/rustcloud-i18n/src/lib.rs`
- Create: `crates/rustcloud-i18n/src/locale.rs`
- Modify: `Cargo.toml` (add member + workspace dep entry; add `polib` and `accept-language` to `[workspace.dependencies]`)

The `Locale` type is a typed wrapper around a short language tag (`"en"`, `"de"`, `"fr_FR"`). The Accept-Language parser is small enough to roll inline (parse the header into `(locale, q-weight)` pairs, sort descending).

- [ ] **Step 1: Add workspace member, deps, and external crates**

Modify `Cargo.toml`:

```toml
[workspace]
members = [
    "crates/rustcloud-cache",
    "crates/rustcloud-config",
    "crates/rustcloud-db",
    "crates/rustcloud-i18n",
    "crates/rustcloud-server",
    "xtask",
]
```

Append to `[workspace.dependencies]`:

```toml
polib = "0.4"
accept-language = "3.1"
rustcloud-i18n = { path = "crates/rustcloud-i18n" }
```

- [ ] **Step 2: Write `crates/rustcloud-i18n/Cargo.toml`**

```toml
[package]
name = "rustcloud-i18n"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
accept-language.workspace = true
polib.workspace = true
thiserror.workspace = true
tracing.workspace = true

[dev-dependencies]
tempfile.workspace = true
```

- [ ] **Step 3: Write `crates/rustcloud-i18n/src/locale.rs`**

```rust
//! `Locale` type and Accept-Language resolution.

/// A short language tag, normalized to lowercase with underscores
/// (Nextcloud convention: `en`, `de`, `fr_FR`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Locale(String);

impl Locale {
    pub fn new(s: impl Into<String>) -> Self {
        let raw = s.into();
        // Normalize: lowercase, hyphen → underscore (Accept-Language uses hyphens;
        // Nextcloud filenames use underscores).
        let normalized = raw.to_lowercase().replace('-', "_");
        Locale(normalized)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// "en" or "fr" or similar — the base language without region.
    pub fn base(&self) -> &str {
        self.0.split_once('_').map(|(b, _)| b).unwrap_or(&self.0)
    }
}

impl std::fmt::Display for Locale {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Pick the best locale for a request, given:
/// - `accept_language`: the raw header value (may be empty).
/// - `available`: locales we actually have catalogs for.
/// - `fallback`: the `config.default_language` (lowercased).
///
/// Resolution order: header preferences (highest q-weight first) that match available
/// → header base-language match (e.g. header says `de-DE`, only `de` available) →
/// fallback → `en`.
pub fn resolve(accept_language: &str, available: &[Locale], fallback: &Locale) -> Locale {
    let prefs = accept_language::parse(accept_language); // Vec<String>, ordered by q desc

    for pref in &prefs {
        let want = Locale::new(pref.clone());
        if available.iter().any(|l| l == &want) {
            return want;
        }
    }
    for pref in &prefs {
        let want = Locale::new(pref.clone());
        if available.iter().any(|l| l.base() == want.base()) {
            return available.iter().find(|l| l.base() == want.base()).unwrap().clone();
        }
    }
    if available.iter().any(|l| l == fallback) {
        return fallback.clone();
    }
    Locale::new("en")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locales(tags: &[&str]) -> Vec<Locale> {
        tags.iter().map(|s| Locale::new(*s)).collect()
    }

    #[test]
    fn locale_normalizes_to_lowercase_underscore() {
        assert_eq!(Locale::new("EN-US").as_str(), "en_us");
        assert_eq!(Locale::new("de").as_str(), "de");
    }

    #[test]
    fn base_strips_region() {
        assert_eq!(Locale::new("fr_FR").base(), "fr");
        assert_eq!(Locale::new("de").base(), "de");
    }

    #[test]
    fn exact_match_wins() {
        let avail = locales(&["en", "de", "fr_fr"]);
        let fb = Locale::new("en");
        assert_eq!(resolve("de", &avail, &fb), Locale::new("de"));
    }

    #[test]
    fn higher_q_weight_wins_when_multiple_offered() {
        let avail = locales(&["en", "de"]);
        let fb = Locale::new("en");
        assert_eq!(resolve("de;q=0.9, en;q=0.8", &avail, &fb), Locale::new("de"));
        assert_eq!(resolve("de;q=0.1, en;q=0.9", &avail, &fb), Locale::new("en"));
    }

    #[test]
    fn base_language_falls_back_when_region_unavailable() {
        let avail = locales(&["en", "de"]);
        let fb = Locale::new("en");
        // Asked for de-DE; we only have plain "de" — should match by base.
        assert_eq!(resolve("de-DE", &avail, &fb), Locale::new("de"));
    }

    #[test]
    fn fallback_used_when_no_match() {
        let avail = locales(&["en", "de"]);
        let fb = Locale::new("de");
        assert_eq!(resolve("ja, ko", &avail, &fb), Locale::new("de"));
    }

    #[test]
    fn empty_header_uses_fallback() {
        let avail = locales(&["en", "de"]);
        let fb = Locale::new("de");
        assert_eq!(resolve("", &avail, &fb), Locale::new("de"));
    }

    #[test]
    fn final_en_when_fallback_also_unavailable() {
        let avail = locales(&["en"]);
        let fb = Locale::new("de"); // not available
        assert_eq!(resolve("", &avail, &fb), Locale::new("en"));
    }
}
```

- [ ] **Step 4: Write `crates/rustcloud-i18n/src/lib.rs`**

```rust
//! Internationalization for Rustcloud.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §9.2.

mod catalog;
mod locale;
mod service;

pub use catalog::{Catalog, CatalogError};
pub use locale::{resolve, Locale};
pub use service::I18n;
```

Create empty placeholders so the `mod` declarations parse:

`crates/rustcloud-i18n/src/catalog.rs`:
```rust
// Implemented in Task 5.

#[derive(Debug)]
pub struct Catalog;

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("placeholder")]
    Placeholder,
}
```

`crates/rustcloud-i18n/src/service.rs`:
```rust
// Implemented in Task 6.

#[derive(Debug, Default)]
pub struct I18n;
```

- [ ] **Step 5: Run tests**

```
cargo test -p rustcloud-i18n --lib
```

Expected: 8 tests pass (all from `locale::tests`).

- [ ] **Step 6: Commit**

```
git add Cargo.toml crates/rustcloud-i18n
git commit -m "feat(i18n): add Locale type and Accept-Language resolver

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 5: rustcloud-i18n — Catalog loader (polib)

**Files:**
- Modify: `crates/rustcloud-i18n/src/catalog.rs`

Scans `l10n/<app>/<locale>.po` files and produces in-memory catalogs. Uses `polib` for parsing. Returns a `HashMap<(app, locale), Catalog>` ready for the `I18n` service in Task 6.

- [ ] **Step 1: Write the failing tests**

Replace `crates/rustcloud-i18n/src/catalog.rs`:

```rust
//! Gettext `.po` catalog loader.

use crate::locale::Locale;
use polib::po_file;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("scan I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse error in {path}: {message}")]
    Parse { path: String, message: String },
}

/// One catalog: a flat msgid → msgstr lookup for a specific `(app, locale)`.
///
/// Plural forms are stored separately; `lookup_plural` returns by index n.
#[derive(Debug, Default)]
pub struct Catalog {
    singular: HashMap<String, String>,
    /// For pluralized entries: msgid_singular → Vec of plural forms.
    /// The index for `n` is computed by the caller (we use the simple
    /// "n != 1" English rule; full plural-form expressions can land later).
    plural: HashMap<String, Vec<String>>,
}

impl Catalog {
    /// Look up a singular message. Returns `None` if not translated; callers
    /// should fall back to the source string.
    pub fn lookup(&self, msgid: &str) -> Option<&str> {
        self.singular.get(msgid).map(String::as_str)
    }

    /// Look up a plural message. `n` is the count; we use the simple
    /// English rule (index 0 for n==1, index 1 otherwise). Returns the source
    /// string fallback expectation: `None` means the caller should fall back.
    pub fn lookup_plural(&self, msgid: &str, n: i64) -> Option<&str> {
        let forms = self.plural.get(msgid)?;
        let idx = if n == 1 { 0 } else { 1 };
        forms.get(idx).map(String::as_str)
    }
}

/// Load all `l10n/<app>/<locale>.po` catalogs under `root`. Each subdirectory
/// of `root` is treated as an `<app>` name; each `*.po` file inside is treated
/// as a locale.
///
/// Returns a map keyed by `(app, locale)` for fast lookup at request time.
/// Missing `root` directories are not an error — they just produce an empty map.
pub fn load_all(root: &Path) -> Result<HashMap<(String, Locale), Catalog>, CatalogError> {
    let mut out = HashMap::new();
    if !root.exists() {
        return Ok(out);
    }
    for app_entry in std::fs::read_dir(root)? {
        let app_entry = app_entry?;
        if !app_entry.file_type()?.is_dir() {
            continue;
        }
        let app = app_entry.file_name().to_string_lossy().to_string();
        for po_entry in std::fs::read_dir(app_entry.path())? {
            let po_entry = po_entry?;
            let path = po_entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("po") {
                continue;
            }
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let locale = Locale::new(stem);
            let catalog = parse_po(&path)?;
            out.insert((app.clone(), locale), catalog);
        }
    }
    Ok(out)
}

fn parse_po(path: &Path) -> Result<Catalog, CatalogError> {
    let file = po_file::parse(path).map_err(|e| CatalogError::Parse {
        path: path.display().to_string(),
        message: format!("{e:?}"),
    })?;
    let mut cat = Catalog::default();
    for msg in file.messages() {
        if msg.is_translated() {
            if let Some(plural_id) = msg.msgid_plural() {
                // polib's API returns plural forms via msgstr_plural()
                let forms: Vec<String> = msg
                    .msgstr_plural()
                    .map(|v| v.iter().cloned().collect())
                    .unwrap_or_default();
                if !forms.is_empty() {
                    cat.plural.insert(msg.msgid().to_string(), forms);
                }
                // Also index by the plural-id so callers can use either form.
                let _ = plural_id;
            } else if let Ok(msgstr) = msg.msgstr() {
                if !msgstr.is_empty() {
                    cat.singular.insert(msg.msgid().to_string(), msgstr.to_string());
                }
            }
        }
    }
    Ok(cat)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write_po(dir: &Path, app: &str, locale: &str, body: &str) {
        let app_dir = dir.join(app);
        fs::create_dir_all(&app_dir).unwrap();
        fs::write(app_dir.join(format!("{locale}.po")), body).unwrap();
    }

    const MIN_PO: &str = r#"msgid ""
msgstr "Content-Type: text/plain; charset=UTF-8\n"

msgid "Hello"
msgstr "Hallo"

msgid "Bye"
msgstr "Tschüss"
"#;

    #[test]
    fn missing_root_returns_empty() {
        let dir = tempdir().unwrap();
        let map = load_all(&dir.path().join("does-not-exist")).unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn loads_single_app_locale() {
        let dir = tempdir().unwrap();
        write_po(dir.path(), "core", "de", MIN_PO);
        let map = load_all(dir.path()).unwrap();
        assert_eq!(map.len(), 1);
        let key = ("core".to_string(), Locale::new("de"));
        let cat = map.get(&key).unwrap();
        assert_eq!(cat.lookup("Hello"), Some("Hallo"));
        assert_eq!(cat.lookup("Bye"), Some("Tschüss"));
        assert_eq!(cat.lookup("Untranslated"), None);
    }

    #[test]
    fn loads_multiple_apps_and_locales() {
        let dir = tempdir().unwrap();
        write_po(dir.path(), "core", "de", MIN_PO);
        write_po(dir.path(), "core", "fr", MIN_PO);
        write_po(dir.path(), "files", "de", MIN_PO);
        let map = load_all(dir.path()).unwrap();
        assert_eq!(map.len(), 3);
        assert!(map.contains_key(&("core".to_string(), Locale::new("de"))));
        assert!(map.contains_key(&("core".to_string(), Locale::new("fr"))));
        assert!(map.contains_key(&("files".to_string(), Locale::new("de"))));
    }

    #[test]
    fn ignores_non_po_files() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("core")).unwrap();
        fs::write(dir.path().join("core").join("readme.md"), "not a po file").unwrap();
        fs::write(dir.path().join("core").join("de.po"), MIN_PO).unwrap();
        let map = load_all(dir.path()).unwrap();
        assert_eq!(map.len(), 1);
    }
}
```

- [ ] **Step 2: Run the tests**

```
cargo test -p rustcloud-i18n --lib
```

Expected: 12 tests pass (8 locale + 4 catalog).

Note: if polib's API differs from what's shown above (`msg.msgstr()` returns `Result<&str, ...>` vs `&str`, `msgstr_plural()` shape varies between versions), adjust the parsing to match the installed version. Report any adjustments in your `DONE_WITH_CONCERNS` block.

- [ ] **Step 3: Commit**

```
git add crates/rustcloud-i18n/src/catalog.rs
git commit -m "feat(i18n): add gettext .po catalog loader via polib

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 6: rustcloud-i18n — I18n service with t() / tn()

**Files:**
- Modify: `crates/rustcloud-i18n/src/service.rs`

`I18n` is the public service. Construct once at startup with all catalogs loaded; clone-cheap (`Arc` inside). `t(app, msgid)` and `tn(app, singular, plural, n)` perform the lookup with source-string fallback. Format-argument substitution is a printf-style `%s` / `%d` replacement.

- [ ] **Step 1: Write the failing tests + impl**

Replace `crates/rustcloud-i18n/src/service.rs`:

```rust
//! Top-level i18n service.

use crate::catalog::Catalog;
use crate::locale::Locale;
use std::collections::HashMap;
use std::sync::Arc;

/// The runtime i18n service. Clone-cheap (`Arc` inside).
#[derive(Debug, Clone)]
pub struct I18n {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    catalogs: HashMap<(String, Locale), Catalog>,
    available: Vec<Locale>,
    fallback: Locale,
}

impl I18n {
    pub fn new(
        catalogs: HashMap<(String, Locale), Catalog>,
        fallback: Locale,
    ) -> Self {
        let mut available: Vec<Locale> = catalogs.keys().map(|(_, l)| l.clone()).collect();
        available.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        available.dedup();
        Self {
            inner: Arc::new(Inner { catalogs, available, fallback }),
        }
    }

    pub fn available_locales(&self) -> &[Locale] {
        &self.inner.available
    }

    pub fn fallback(&self) -> &Locale {
        &self.inner.fallback
    }

    /// Translate a singular message. If no translation is available for
    /// `(app, locale, msgid)`, returns the source `msgid` unchanged.
    /// `args` substitutes `%s` placeholders in order.
    pub fn t(&self, app: &str, locale: &Locale, msgid: &str, args: &[&str]) -> String {
        let translated = self
            .inner
            .catalogs
            .get(&(app.to_string(), locale.clone()))
            .and_then(|c| c.lookup(msgid))
            .unwrap_or(msgid);
        substitute(translated, args)
    }

    /// Translate a pluralized message. `n` selects the form (simple English
    /// rule: 0 == many, 1 == singular, anything else == many).
    /// Falls back to the appropriate English source string.
    pub fn tn(
        &self,
        app: &str,
        locale: &Locale,
        singular: &str,
        plural: &str,
        n: i64,
        args: &[&str],
    ) -> String {
        let translated = self
            .inner
            .catalogs
            .get(&(app.to_string(), locale.clone()))
            .and_then(|c| c.lookup_plural(singular, n));
        let chosen = translated.unwrap_or(if n == 1 { singular } else { plural });
        substitute(chosen, args)
    }
}

/// Simple `%s` and `%d` substitution; consumes args in order.
fn substitute(template: &str, args: &[&str]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    let mut iter = args.iter();
    while let Some(c) = chars.next() {
        if c == '%' {
            match chars.peek() {
                Some('s') | Some('d') => {
                    chars.next();
                    out.push_str(iter.next().copied().unwrap_or(""));
                }
                Some('%') => {
                    chars.next();
                    out.push('%');
                }
                _ => out.push(c),
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::load_all;
    use std::fs;
    use tempfile::tempdir;

    fn seed_catalogs() -> HashMap<(String, Locale), Catalog> {
        let dir = tempdir().unwrap();
        let app = dir.path().join("core");
        fs::create_dir_all(&app).unwrap();
        fs::write(
            app.join("de.po"),
            r#"msgid ""
msgstr "Content-Type: text/plain; charset=UTF-8\n"

msgid "Hello %s"
msgstr "Hallo %s"
"#,
        )
        .unwrap();
        load_all(dir.path()).unwrap()
    }

    #[test]
    fn translates_with_args() {
        let cats = seed_catalogs();
        let i18n = I18n::new(cats, Locale::new("en"));
        let s = i18n.t("core", &Locale::new("de"), "Hello %s", &["Alice"]);
        assert_eq!(s, "Hallo Alice");
    }

    #[test]
    fn falls_back_to_source_when_locale_missing() {
        let cats = seed_catalogs();
        let i18n = I18n::new(cats, Locale::new("en"));
        let s = i18n.t("core", &Locale::new("ja"), "Hello %s", &["Alice"]);
        assert_eq!(s, "Hello Alice");
    }

    #[test]
    fn falls_back_to_source_when_msgid_untranslated() {
        let cats = seed_catalogs();
        let i18n = I18n::new(cats, Locale::new("en"));
        let s = i18n.t("core", &Locale::new("de"), "Bye %s", &["Alice"]);
        assert_eq!(s, "Bye Alice");
    }

    #[test]
    fn substitute_handles_percent_d_and_percent_percent() {
        assert_eq!(substitute("%d apples are 100%%", &["5"]), "5 apples are 100%");
        assert_eq!(substitute("plain text", &[]), "plain text");
        assert_eq!(substitute("%s %s", &["a"]), "a "); // missing arg → empty
    }

    #[test]
    fn tn_uses_singular_for_one_else_plural() {
        let cats = HashMap::new();
        let i18n = I18n::new(cats, Locale::new("en"));
        let l = Locale::new("en");
        assert_eq!(i18n.tn("files", &l, "%d file", "%d files", 1, &["1"]), "1 file");
        assert_eq!(i18n.tn("files", &l, "%d file", "%d files", 5, &["5"]), "5 files");
        assert_eq!(i18n.tn("files", &l, "%d file", "%d files", 0, &["0"]), "0 files");
    }

    #[test]
    fn available_locales_is_sorted_and_unique() {
        let cats = seed_catalogs();
        let i18n = I18n::new(cats, Locale::new("en"));
        let avail = i18n.available_locales();
        assert_eq!(avail, &[Locale::new("de")]);
    }
}
```

- [ ] **Step 2: Run the tests**

```
cargo test -p rustcloud-i18n --lib
```

Expected: 18 tests pass (8 locale + 4 catalog + 6 service).

- [ ] **Step 3: Commit**

```
git add crates/rustcloud-i18n/src/service.rs
git commit -m "feat(i18n): add I18n service with t()/tn() and printf substitution

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 7: Seed the core translations directory

**Files:**
- Create: `l10n/core/README.md`
- Create: `l10n/core/de.po`

A real (small) `.po` file for the `core` app so later integration tests (and the development build) have something to load. We seed German only; other locales land as the project grows.

- [ ] **Step 1: Write the README**

Create `l10n/core/README.md`:

```markdown
# Core translations

Gettext `.po` files for the Rustcloud `core` namespace. Each file is named
`<locale>.po` (e.g. `de.po`, `fr_FR.po`). New locales: drop in a new file with the
same `msgid` keys.

Edit/refresh with any gettext-aware editor (Poedit, GTranslator, plain text).
```

- [ ] **Step 2: Write `l10n/core/de.po`**

Create `l10n/core/de.po`:

```po
msgid ""
msgstr ""
"Project-Id-Version: rustcloud-core 0.1.0\n"
"Content-Type: text/plain; charset=UTF-8\n"
"Content-Transfer-Encoding: 8bit\n"
"Language: de\n"
"Plural-Forms: nplurals=2; plural=(n != 1);\n"

msgid "Welcome to Rustcloud"
msgstr "Willkommen bei Rustcloud"

msgid "Logged in as %s"
msgstr "Angemeldet als %s"
```

- [ ] **Step 3: Sanity-check parseability**

Confirm the file loads by running:

```
cargo test -p rustcloud-i18n --lib
```

Expected: still 18 tests passing (no change). The file exists for future integration tests in Task 14.

- [ ] **Step 4: Commit**

```
git add l10n
git commit -m "feat(i18n): seed l10n/core/de.po with two messages

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 8: rustcloud-ocs — workspace + OcsStatus + envelope skeleton

**Files:**
- Create: `crates/rustcloud-ocs/Cargo.toml`
- Create: `crates/rustcloud-ocs/src/lib.rs`
- Create: `crates/rustcloud-ocs/src/status.rs`
- Create: `crates/rustcloud-ocs/src/envelope.rs`
- Create: `crates/rustcloud-ocs/src/format.rs`
- Modify: `Cargo.toml` (add member + dep + `quick-xml`)

`OcsStatus` codifies the Nextcloud OCS status codes; `OcsResponse<T>` is the envelope; `Format` selects JSON vs XML. Phase 2's `render()` returns `(body: String, content_type: &'static str)` — no axum yet.

- [ ] **Step 1: Add workspace member + dependency**

Modify `Cargo.toml`:

```toml
[workspace]
members = [
    "crates/rustcloud-cache",
    "crates/rustcloud-config",
    "crates/rustcloud-db",
    "crates/rustcloud-i18n",
    "crates/rustcloud-ocs",
    "crates/rustcloud-server",
    "xtask",
]
```

Append under `[workspace.dependencies]`:

```toml
quick-xml = { version = "0.36", features = ["serialize"] }
rustcloud-ocs = { path = "crates/rustcloud-ocs" }
```

- [ ] **Step 2: Write `crates/rustcloud-ocs/Cargo.toml`**

```toml
[package]
name = "rustcloud-ocs"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
async-trait.workspace = true
quick-xml.workspace = true
rustcloud-cache.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tokio.workspace = true
tracing.workspace = true
```

- [ ] **Step 3: Write `crates/rustcloud-ocs/src/status.rs`**

```rust
//! Nextcloud OCS status codes. Hand-mapped to match upstream behavior so
//! existing clients see the numbers they expect.
//!
//! See spec §9.3.

/// OCS-level status (the `<statuscode>` in the envelope). Distinct from the
/// HTTP status — both are emitted, the OCS one inside the envelope, the
/// HTTP one in the wire-level response status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcsStatus {
    Ok,            // 100 in v1, 200 in v2
    Created,       // 201 (v2 only — rare; map to Ok in v1)
    BadRequest,    // 400
    Unauthorized,  // 997 — yes, really
    Forbidden,     // 403
    NotFound,      // 998
    UnknownError,  // 999
    ServerError,   // 996
}

impl OcsStatus {
    pub fn v1_code(self) -> u16 {
        match self {
            OcsStatus::Ok => 100,
            OcsStatus::Created => 100,
            OcsStatus::BadRequest => 400,
            OcsStatus::Unauthorized => 997,
            OcsStatus::Forbidden => 403,
            OcsStatus::NotFound => 998,
            OcsStatus::UnknownError => 999,
            OcsStatus::ServerError => 996,
        }
    }

    pub fn v2_code(self) -> u16 {
        match self {
            OcsStatus::Ok => 200,
            OcsStatus::Created => 201,
            OcsStatus::BadRequest => 400,
            OcsStatus::Unauthorized => 997,
            OcsStatus::Forbidden => 403,
            OcsStatus::NotFound => 998,
            OcsStatus::UnknownError => 999,
            OcsStatus::ServerError => 996,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            OcsStatus::Ok => "ok",
            OcsStatus::Created => "ok",
            OcsStatus::BadRequest => "failure",
            OcsStatus::Unauthorized => "failure",
            OcsStatus::Forbidden => "failure",
            OcsStatus::NotFound => "failure",
            OcsStatus::UnknownError => "failure",
            OcsStatus::ServerError => "failure",
        }
    }

    pub fn http_code(self) -> u16 {
        match self {
            OcsStatus::Ok => 200,
            OcsStatus::Created => 201,
            OcsStatus::BadRequest => 400,
            OcsStatus::Unauthorized => 401,
            OcsStatus::Forbidden => 403,
            OcsStatus::NotFound => 404,
            OcsStatus::UnknownError => 500,
            OcsStatus::ServerError => 500,
        }
    }
}

/// Which OCS protocol version the response is wrapped in. Affects only
/// the `statuscode` mapping (100 vs 200 for OK).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcsVersion {
    V1,
    V2,
}

impl OcsStatus {
    pub fn code_for(self, version: OcsVersion) -> u16 {
        match version {
            OcsVersion::V1 => self.v1_code(),
            OcsVersion::V2 => self.v2_code(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_maps_to_100_in_v1_and_200_in_v2() {
        assert_eq!(OcsStatus::Ok.code_for(OcsVersion::V1), 100);
        assert_eq!(OcsStatus::Ok.code_for(OcsVersion::V2), 200);
    }

    #[test]
    fn nextcloud_specific_failure_codes_match_upstream() {
        assert_eq!(OcsStatus::Unauthorized.v2_code(), 997);
        assert_eq!(OcsStatus::NotFound.v2_code(), 998);
        assert_eq!(OcsStatus::UnknownError.v2_code(), 999);
        assert_eq!(OcsStatus::ServerError.v2_code(), 996);
    }

    #[test]
    fn http_codes_independent_from_ocs_codes() {
        assert_eq!(OcsStatus::Unauthorized.http_code(), 401);
        assert_eq!(OcsStatus::NotFound.http_code(), 404);
    }
}
```

- [ ] **Step 4: Write `crates/rustcloud-ocs/src/format.rs`**

```rust
//! Response format selection. Mirrors Nextcloud's content negotiation.

/// Which serialization the response should use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Xml,
    Json,
}

impl Format {
    pub fn content_type(self) -> &'static str {
        match self {
            Format::Xml => "application/xml; charset=utf-8",
            Format::Json => "application/json; charset=utf-8",
        }
    }
}

/// Pick the format from a request's `?format=` query value and `Accept` header.
/// Precedence: `?format=` query > `Accept: application/json` > XML default.
pub fn negotiate(format_query: Option<&str>, accept_header: Option<&str>) -> Format {
    if let Some(q) = format_query {
        let q = q.to_ascii_lowercase();
        if q == "json" {
            return Format::Json;
        }
        if q == "xml" {
            return Format::Xml;
        }
    }
    if let Some(accept) = accept_header {
        // Naive: substring search for "application/json" or "json".
        let a = accept.to_ascii_lowercase();
        if a.contains("application/json") || a.contains("text/json") {
            return Format::Json;
        }
    }
    Format::Xml
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_param_wins() {
        assert_eq!(negotiate(Some("json"), Some("application/xml")), Format::Json);
        assert_eq!(negotiate(Some("xml"), Some("application/json")), Format::Xml);
    }

    #[test]
    fn accept_header_falls_through_when_no_query() {
        assert_eq!(negotiate(None, Some("application/json")), Format::Json);
        assert_eq!(negotiate(None, Some("text/json")), Format::Json);
        assert_eq!(negotiate(None, Some("application/xml")), Format::Xml);
    }

    #[test]
    fn default_is_xml() {
        assert_eq!(negotiate(None, None), Format::Xml);
        assert_eq!(negotiate(Some("garbage"), None), Format::Xml);
    }

    #[test]
    fn content_type_matches_format() {
        assert!(Format::Json.content_type().starts_with("application/json"));
        assert!(Format::Xml.content_type().starts_with("application/xml"));
    }
}
```

- [ ] **Step 5: Write `crates/rustcloud-ocs/src/lib.rs`**

```rust
//! OCS envelope and capabilities aggregator.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §9.3.

mod capabilities;
mod core_caps;
mod envelope;
mod format;
mod status;

pub use capabilities::{
    aggregate, CapabilityContext, CapabilityError, CapabilityProvider, CapabilitiesPayload,
};
pub use core_caps::CoreCapabilities;
pub use envelope::{render, OcsResponse};
pub use format::{negotiate, Format};
pub use status::{OcsStatus, OcsVersion};
```

Create placeholder stubs (replaced in Tasks 9–11):

`crates/rustcloud-ocs/src/envelope.rs`:
```rust
// Implemented in Task 9.

use crate::format::Format;
use crate::status::OcsVersion;

#[derive(Debug)]
pub struct OcsResponse<T> {
    pub status: crate::status::OcsStatus,
    pub message: String,
    pub data: T,
    pub version: OcsVersion,
}

pub fn render<T>(_resp: &OcsResponse<T>, _format: Format) -> (String, &'static str) {
    todo!("implemented in Task 9")
}
```

`crates/rustcloud-ocs/src/capabilities.rs`:
```rust
// Implemented in Task 10.

use async_trait::async_trait;
use std::sync::Arc;

#[derive(Debug, Default)]
pub struct CapabilityContext<'a> {
    pub locale: Option<&'a str>,
    pub user_id: Option<&'a str>,
}

#[derive(Debug, thiserror::Error)]
pub enum CapabilityError {
    #[error("placeholder")]
    Placeholder,
}

#[async_trait]
pub trait CapabilityProvider: Send + Sync {
    fn namespace(&self) -> &'static str;
    fn contribute(&self, ctx: &CapabilityContext<'_>) -> serde_json::Value;
}

#[derive(Debug)]
pub struct CapabilitiesPayload {
    pub etag: String,
    pub body: serde_json::Value,
}

pub async fn aggregate(
    _providers: &[Arc<dyn CapabilityProvider>],
    _ctx: &CapabilityContext<'_>,
    _cache: Arc<dyn rustcloud_cache::Cache>,
    _version: &str,
    _instance_id: &str,
) -> Result<CapabilitiesPayload, CapabilityError> {
    todo!("implemented in Task 10")
}
```

`crates/rustcloud-ocs/src/core_caps.rs`:
```rust
// Implemented in Task 11.

use crate::capabilities::{CapabilityContext, CapabilityProvider};
use async_trait::async_trait;

pub struct CoreCapabilities {
    pub webdav_root: String,
    pub poll_interval: u32,
}

#[async_trait]
impl CapabilityProvider for CoreCapabilities {
    fn namespace(&self) -> &'static str {
        "core"
    }
    fn contribute(&self, _ctx: &CapabilityContext<'_>) -> serde_json::Value {
        serde_json::Value::Null
    }
}
```

- [ ] **Step 6: Run the tests**

```
cargo test -p rustcloud-ocs --lib
```

Expected: 7 tests pass (3 status + 4 format).

- [ ] **Step 7: Commit**

```
git add Cargo.toml crates/rustcloud-ocs
git commit -m "feat(ocs): add OcsStatus and Format with content negotiation

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 9: rustcloud-ocs — OcsResponse envelope rendering

**Files:**
- Modify: `crates/rustcloud-ocs/src/envelope.rs`

Renders `OcsResponse<T>` to either JSON or XML, matching Nextcloud's wire format:

```json
{"ocs":{"meta":{"status":"ok","statuscode":200,"message":"OK"},"data": ...}}
```

```xml
<?xml version="1.0"?>
<ocs>
  <meta>
    <status>ok</status>
    <statuscode>200</statuscode>
    <message>OK</message>
  </meta>
  <data>...</data>
</ocs>
```

- [ ] **Step 1: Write the impl + tests**

Replace `crates/rustcloud-ocs/src/envelope.rs`:

```rust
//! OCS envelope rendering. JSON via `serde_json`; XML hand-rolled via `quick-xml`'s
//! `Writer` because the spec wants a specific element shape that's painful to
//! coax out of the serializer.

use crate::format::Format;
use crate::status::{OcsStatus, OcsVersion};
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug)]
pub struct OcsResponse<T: Serialize> {
    pub status: OcsStatus,
    pub message: String,
    pub data: T,
    pub version: OcsVersion,
}

impl<T: Serialize> OcsResponse<T> {
    pub fn ok(data: T, version: OcsVersion) -> Self {
        Self {
            status: OcsStatus::Ok,
            message: "OK".into(),
            data,
            version,
        }
    }

    pub fn failure(status: OcsStatus, message: impl Into<String>, data: T, version: OcsVersion) -> Self {
        Self { status, message: message.into(), data, version }
    }
}

/// Render to `(body, content_type)`. Errors are infallible at this layer
/// because serde_json::to_string only fails on user-supplied types we don't
/// pass through; we wrap the JSON case in a panic-free pattern anyway.
pub fn render<T: Serialize>(resp: &OcsResponse<T>, format: Format) -> (String, &'static str) {
    match format {
        Format::Json => (render_json(resp), Format::Json.content_type()),
        Format::Xml => (render_xml(resp), Format::Xml.content_type()),
    }
}

fn render_json<T: Serialize>(resp: &OcsResponse<T>) -> String {
    let meta = json!({
        "status": resp.status.label(),
        "statuscode": resp.status.code_for(resp.version),
        "message": resp.message,
    });
    let data: Value = serde_json::to_value(&resp.data).unwrap_or(Value::Null);
    let envelope = json!({
        "ocs": {
            "meta": meta,
            "data": data,
        }
    });
    serde_json::to_string(&envelope).unwrap_or_else(|_| "{}".into())
}

fn render_xml<T: Serialize>(resp: &OcsResponse<T>) -> String {
    // Build a JSON value first, then walk it as a tree to emit XML by hand.
    // Nextcloud's XML format is "JSON-as-XML": numbers, strings, arrays, objects
    // all map mechanically.
    let data: Value = serde_json::to_value(&resp.data).unwrap_or(Value::Null);
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\"?>\n<ocs>\n");
    out.push_str("  <meta>\n");
    out.push_str(&format!("    <status>{}</status>\n", xml_escape(resp.status.label())));
    out.push_str(&format!(
        "    <statuscode>{}</statuscode>\n",
        resp.status.code_for(resp.version)
    ));
    out.push_str(&format!(
        "    <message>{}</message>\n",
        xml_escape(&resp.message)
    ));
    out.push_str("  </meta>\n");
    out.push_str("  <data>");
    write_value(&mut out, &data);
    out.push_str("</data>\n");
    out.push_str("</ocs>\n");
    out
}

fn write_value(out: &mut String, v: &Value) {
    match v {
        Value::Null => {}
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => out.push_str(&xml_escape(s)),
        Value::Array(arr) => {
            for item in arr {
                out.push_str("<element>");
                write_value(out, item);
                out.push_str("</element>");
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                let tag = xml_escape(k);
                out.push_str(&format!("<{tag}>"));
                write_value(out, v);
                out.push_str(&format!("</{tag}>"));
            }
        }
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    #[derive(Serialize)]
    struct Empty {}

    #[derive(Serialize)]
    struct VersionPayload {
        major: u32,
        minor: u32,
        edition: String,
    }

    #[test]
    fn ok_json_envelope_v2() {
        let r = OcsResponse::ok(Empty {}, OcsVersion::V2);
        let (body, ct) = render(&r, Format::Json);
        assert!(ct.starts_with("application/json"));
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["ocs"]["meta"]["status"], "ok");
        assert_eq!(parsed["ocs"]["meta"]["statuscode"], 200);
        assert_eq!(parsed["ocs"]["meta"]["message"], "OK");
    }

    #[test]
    fn ok_xml_envelope_v1() {
        let r = OcsResponse::ok(Empty {}, OcsVersion::V1);
        let (body, _) = render(&r, Format::Xml);
        assert!(body.contains("<status>ok</status>"));
        assert!(body.contains("<statuscode>100</statuscode>"));
        assert!(body.contains("<message>OK</message>"));
    }

    #[test]
    fn failure_carries_message_and_code() {
        let r = OcsResponse::failure(OcsStatus::NotFound, "no such user", Empty {}, OcsVersion::V2);
        let (body_json, _) = render(&r, Format::Json);
        let parsed: serde_json::Value = serde_json::from_str(&body_json).unwrap();
        assert_eq!(parsed["ocs"]["meta"]["status"], "failure");
        assert_eq!(parsed["ocs"]["meta"]["statuscode"], 998);
        assert_eq!(parsed["ocs"]["meta"]["message"], "no such user");
    }

    #[test]
    fn json_payload_round_trip() {
        let payload = VersionPayload { major: 31, minor: 0, edition: "Rustcloud".into() };
        let r = OcsResponse::ok(payload, OcsVersion::V2);
        let (body, _) = render(&r, Format::Json);
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["ocs"]["data"]["major"], 31);
        assert_eq!(parsed["ocs"]["data"]["minor"], 0);
        assert_eq!(parsed["ocs"]["data"]["edition"], "Rustcloud");
    }

    #[test]
    fn xml_payload_emits_nested_tags() {
        let payload = VersionPayload { major: 31, minor: 0, edition: "Rustcloud".into() };
        let r = OcsResponse::ok(payload, OcsVersion::V2);
        let (body, _) = render(&r, Format::Xml);
        assert!(body.contains("<major>31</major>"));
        assert!(body.contains("<minor>0</minor>"));
        assert!(body.contains("<edition>Rustcloud</edition>"));
    }

    #[test]
    fn xml_escapes_special_chars() {
        let r = OcsResponse::failure(OcsStatus::BadRequest, "5 < 6 & true", Empty {}, OcsVersion::V2);
        let (body, _) = render(&r, Format::Xml);
        assert!(body.contains("<message>5 &lt; 6 &amp; true</message>"));
    }
}
```

- [ ] **Step 2: Run the tests**

```
cargo test -p rustcloud-ocs --lib
```

Expected: 13 tests pass (3 status + 4 format + 6 envelope).

- [ ] **Step 3: Commit**

```
git add crates/rustcloud-ocs/src/envelope.rs
git commit -m "feat(ocs): render OcsResponse to JSON and XML envelopes

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 10: rustcloud-ocs — CapabilityProvider trait + aggregator with ETag

**Files:**
- Modify: `crates/rustcloud-ocs/src/capabilities.rs`

The aggregator iterates registered providers, merges their contributions under their namespaces, and caches the assembled payload with a stable ETag (hash of provider list + version + instance_id + locale + user_id). Result lives in `rustcloud-cache` for 60s.

- [ ] **Step 1: Write the impl + tests**

Replace `crates/rustcloud-ocs/src/capabilities.rs`:

```rust
//! Capabilities aggregator. Iterates registered providers, merges JSON, caches.
//!
//! See spec §9.3.

use async_trait::async_trait;
use rustcloud_cache::{Cache, CacheError};
use serde_json::{json, Map, Value};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

/// Per-request context passed to `CapabilityProvider::contribute`.
/// Lightweight (no `AppState` reference) so providers don't accidentally couple
/// to the wider state machinery.
#[derive(Debug, Default, Clone)]
pub struct CapabilityContext<'a> {
    pub locale: Option<&'a str>,
    pub user_id: Option<&'a str>,
}

#[derive(Debug, thiserror::Error)]
pub enum CapabilityError {
    #[error("cache error: {0}")]
    Cache(#[from] CacheError),
    #[error("JSON serialization failed: {0}")]
    Json(#[from] serde_json::Error),
}

#[async_trait]
pub trait CapabilityProvider: Send + Sync {
    /// The top-level key under `ocs.data.capabilities` this provider contributes to.
    fn namespace(&self) -> &'static str;

    /// Return a JSON value (usually an object) to merge under `namespace()`.
    fn contribute(&self, ctx: &CapabilityContext<'_>) -> Value;
}

/// The aggregated payload returned to clients.
#[derive(Debug, Clone)]
pub struct CapabilitiesPayload {
    pub etag: String,
    pub body: Value,
}

/// Run the aggregator. Cache key includes locale + user_id so personalized
/// responses don't bleed across users.
pub async fn aggregate(
    providers: &[Arc<dyn CapabilityProvider>],
    ctx: &CapabilityContext<'_>,
    cache: Arc<dyn Cache>,
    version: &str,
    instance_id: &str,
) -> Result<CapabilitiesPayload, CapabilityError> {
    let cache_key = format!(
        "{instance_id}:caps:{}:{}",
        ctx.locale.unwrap_or(""),
        ctx.user_id.unwrap_or("")
    );

    if let Some(raw) = cache.get(&cache_key).await? {
        if let Ok(payload) = serde_json::from_slice::<CachedPayload>(&raw) {
            return Ok(CapabilitiesPayload { etag: payload.etag, body: payload.body });
        }
    }

    let mut caps = Map::new();
    for p in providers {
        caps.insert(p.namespace().to_string(), p.contribute(ctx));
    }

    let body = json!({
        "version": {
            "major": 31,
            "minor": 0,
            "micro": 0,
            "string": version,
            "edition": ""
        },
        "capabilities": Value::Object(caps),
    });

    let etag = compute_etag(version, instance_id, providers, ctx);
    let cached = CachedPayload { etag: etag.clone(), body: body.clone() };
    let serialized = serde_json::to_vec(&cached)?;
    let _ = cache.set(&cache_key, &serialized, Some(Duration::from_secs(60))).await;

    Ok(CapabilitiesPayload { etag, body })
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CachedPayload {
    etag: String,
    body: Value,
}

fn compute_etag(
    version: &str,
    instance_id: &str,
    providers: &[Arc<dyn CapabilityProvider>],
    ctx: &CapabilityContext<'_>,
) -> String {
    let mut hasher = DefaultHasher::new();
    version.hash(&mut hasher);
    instance_id.hash(&mut hasher);
    for p in providers {
        p.namespace().hash(&mut hasher);
    }
    ctx.locale.unwrap_or("").hash(&mut hasher);
    ctx.user_id.unwrap_or("").hash(&mut hasher);
    format!("W/\"{:x}\"", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustcloud_cache::MemoryCache;
    use serde_json::json;

    struct FakeProvider {
        ns: &'static str,
        body: Value,
    }

    #[async_trait]
    impl CapabilityProvider for FakeProvider {
        fn namespace(&self) -> &'static str {
            self.ns
        }
        fn contribute(&self, _ctx: &CapabilityContext<'_>) -> Value {
            self.body.clone()
        }
    }

    fn cache() -> Arc<dyn Cache> {
        Arc::new(MemoryCache::new())
    }

    #[tokio::test]
    async fn merges_providers_under_their_namespaces() {
        let providers: Vec<Arc<dyn CapabilityProvider>> = vec![
            Arc::new(FakeProvider { ns: "core", body: json!({"pollinterval": 60}) }),
            Arc::new(FakeProvider { ns: "files", body: json!({"versioning": true}) }),
        ];
        let ctx = CapabilityContext::default();
        let payload = aggregate(&providers, &ctx, cache(), "31.0.0", "inst1").await.unwrap();
        assert_eq!(payload.body["capabilities"]["core"]["pollinterval"], 60);
        assert_eq!(payload.body["capabilities"]["files"]["versioning"], true);
        assert_eq!(payload.body["version"]["string"], "31.0.0");
    }

    #[tokio::test]
    async fn etag_changes_with_version() {
        let providers: Vec<Arc<dyn CapabilityProvider>> = vec![Arc::new(FakeProvider {
            ns: "core",
            body: json!({}),
        })];
        let ctx = CapabilityContext::default();
        let c = cache();
        let a = aggregate(&providers, &ctx, c.clone(), "31.0.0", "inst1").await.unwrap();
        // Clear cache so we compute fresh.
        c.del("inst1:caps::").await.unwrap();
        let b = aggregate(&providers, &ctx, c.clone(), "31.0.1", "inst1").await.unwrap();
        assert_ne!(a.etag, b.etag);
    }

    #[tokio::test]
    async fn etag_separates_users() {
        let providers: Vec<Arc<dyn CapabilityProvider>> = vec![Arc::new(FakeProvider {
            ns: "core",
            body: json!({}),
        })];
        let c = cache();
        let alice_ctx = CapabilityContext { locale: Some("en"), user_id: Some("alice") };
        let bob_ctx = CapabilityContext { locale: Some("en"), user_id: Some("bob") };
        let a = aggregate(&providers, &alice_ctx, c.clone(), "31", "inst1").await.unwrap();
        let b = aggregate(&providers, &bob_ctx, c.clone(), "31", "inst1").await.unwrap();
        assert_ne!(a.etag, b.etag);
    }

    #[tokio::test]
    async fn second_call_hits_cache() {
        // Verify by checking that a cache key was written after the first call.
        let providers: Vec<Arc<dyn CapabilityProvider>> = vec![Arc::new(FakeProvider {
            ns: "core",
            body: json!({}),
        })];
        let ctx = CapabilityContext::default();
        let c = cache();
        aggregate(&providers, &ctx, c.clone(), "31", "inst1").await.unwrap();
        let key = "inst1:caps::";
        assert!(c.get(key).await.unwrap().is_some(), "cache should contain aggregated payload");

        // Second call should produce identical etag (cache hit).
        let p2 = aggregate(&providers, &ctx, c.clone(), "31", "inst1").await.unwrap();
        let first_etag = {
            let raw = c.get(key).await.unwrap().unwrap();
            let cached: CachedPayload = serde_json::from_slice(&raw).unwrap();
            cached.etag
        };
        assert_eq!(p2.etag, first_etag);
    }
}
```

- [ ] **Step 2: Run the tests**

```
cargo test -p rustcloud-ocs --lib
```

Expected: 17 tests pass (3 status + 4 format + 6 envelope + 4 capabilities).

- [ ] **Step 3: Commit**

```
git add crates/rustcloud-ocs/src/capabilities.rs
git commit -m "feat(ocs): aggregate capability providers with cache-backed ETag

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 11: rustcloud-ocs — CoreCapabilities implementation

**Files:**
- Modify: `crates/rustcloud-ocs/src/core_caps.rs`

The `core` namespace contribution: the keys Nextcloud clients expect to find — `pollinterval`, `webdav-root`, `mod-rewrite-working`. Returned values are configurable so theming / overrides can vary.

- [ ] **Step 1: Write the impl + tests**

Replace `crates/rustcloud-ocs/src/core_caps.rs`:

```rust
//! Built-in `core` namespace capabilities. Matches Nextcloud's shape.
//!
//! Spec §9.3.

use crate::capabilities::{CapabilityContext, CapabilityProvider};
use async_trait::async_trait;
use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct CoreCapabilities {
    /// In seconds. Nextcloud default is 60.
    pub poll_interval: u32,
    /// Sub-path under `/remote.php` where DAV lives. Default: `"remote.php/dav"`.
    pub webdav_root: String,
    /// Whether mod_rewrite (or equivalent) is configured. True for axum-direct.
    pub mod_rewrite_working: bool,
    /// Reference time bucket size in ms.
    pub reference_time_offset_ms: i64,
}

impl Default for CoreCapabilities {
    fn default() -> Self {
        Self {
            poll_interval: 60,
            webdav_root: "remote.php/dav".into(),
            mod_rewrite_working: true,
            reference_time_offset_ms: 0,
        }
    }
}

#[async_trait]
impl CapabilityProvider for CoreCapabilities {
    fn namespace(&self) -> &'static str {
        "core"
    }

    fn contribute(&self, _ctx: &CapabilityContext<'_>) -> Value {
        json!({
            "pollinterval": self.poll_interval,
            "webdav-root": self.webdav_root,
            "mod-rewrite-working": self.mod_rewrite_working,
            "reference-time": self.reference_time_offset_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::{aggregate, CapabilityProvider};
    use rustcloud_cache::{Cache, MemoryCache};
    use std::sync::Arc;

    #[test]
    fn default_values_match_nextcloud_shape() {
        let core = CoreCapabilities::default();
        let v = core.contribute(&CapabilityContext::default());
        assert_eq!(v["pollinterval"], 60);
        assert_eq!(v["webdav-root"], "remote.php/dav");
        assert_eq!(v["mod-rewrite-working"], true);
    }

    #[test]
    fn custom_values_flow_through() {
        let core = CoreCapabilities {
            poll_interval: 30,
            webdav_root: "ocs/v2.php/dav".into(),
            mod_rewrite_working: false,
            reference_time_offset_ms: 1000,
        };
        let v = core.contribute(&CapabilityContext::default());
        assert_eq!(v["pollinterval"], 30);
        assert_eq!(v["webdav-root"], "ocs/v2.php/dav");
        assert_eq!(v["mod-rewrite-working"], false);
        assert_eq!(v["reference-time"], 1000);
    }

    #[tokio::test]
    async fn aggregator_includes_core_namespace() {
        let providers: Vec<Arc<dyn CapabilityProvider>> =
            vec![Arc::new(CoreCapabilities::default())];
        let ctx = CapabilityContext::default();
        let cache: Arc<dyn Cache> = Arc::new(MemoryCache::new());
        let p = aggregate(&providers, &ctx, cache, "31.0.0", "inst1").await.unwrap();
        assert_eq!(p.body["capabilities"]["core"]["pollinterval"], 60);
    }
}
```

- [ ] **Step 2: Run the tests**

```
cargo test -p rustcloud-ocs --lib
```

Expected: 20 tests pass (3 status + 4 format + 6 envelope + 4 capabilities + 3 core_caps).

- [ ] **Step 3: Commit**

```
git add crates/rustcloud-ocs/src/core_caps.rs
git commit -m "feat(ocs): add CoreCapabilities provider with Nextcloud-shaped defaults

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 12: rustcloud-core — workspace + Error + AppConfigService

**Files:**
- Create: `crates/rustcloud-core/Cargo.toml`
- Create: `crates/rustcloud-core/src/lib.rs`
- Create: `crates/rustcloud-core/src/error.rs`
- Create: `crates/rustcloud-core/src/appconfig.rs`
- Create: `crates/rustcloud-core/src/bootstrap.rs` (stub for Task 13)
- Create: `crates/rustcloud-core/src/state.rs` (stub for Task 13)
- Modify: `Cargo.toml` (add member + workspace dep)

`rustcloud-core` is the facade. Phase 2 lays down the `Error` enum (with status-code mapping for Phase 3 to use), the runtime `AppConfigService` backed by the `oc_appconfig` table + cache, and stubs for `AppState` + `BootstrapHook` (filled in by Task 13).

- [ ] **Step 1: Add workspace member + dep**

Modify `Cargo.toml`:

```toml
[workspace]
members = [
    "crates/rustcloud-cache",
    "crates/rustcloud-config",
    "crates/rustcloud-core",
    "crates/rustcloud-db",
    "crates/rustcloud-i18n",
    "crates/rustcloud-ocs",
    "crates/rustcloud-server",
    "xtask",
]
```

Append under `[workspace.dependencies]`:

```toml
rustcloud-core = { path = "crates/rustcloud-core" }
```

- [ ] **Step 2: Write `crates/rustcloud-core/Cargo.toml`**

```toml
[package]
name = "rustcloud-core"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
anyhow.workspace = true
async-trait.workspace = true
rustcloud-cache.workspace = true
rustcloud-config.workspace = true
rustcloud-db.workspace = true
rustcloud-i18n.workspace = true
rustcloud-ocs.workspace = true
serde.workspace = true
serde_json.workspace = true
sqlx.workspace = true
thiserror.workspace = true
tokio.workspace = true
tracing.workspace = true

[dev-dependencies]
tempfile.workspace = true
```

- [ ] **Step 3: Write `crates/rustcloud-core/src/error.rs`**

```rust
//! Unified `Error` type for the core surface.
//!
//! Each kind has a HTTP status mapping (used by Phase 3's HTTP layer) that lives
//! here as a pure function — no axum types are pulled in.

use rustcloud_cache::CacheError;
use rustcloud_config::{FileConfigError, LoadError};
use rustcloud_db::DbError;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not found")]
    NotFound,
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("locked")]
    Locked,
    #[error("OCS error {code}: {message}")]
    Ocs { code: u16, message: String },
    #[error(transparent)]
    Config(#[from] LoadError),
    #[error(transparent)]
    ConfigValidation(#[from] FileConfigError),
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Cache(#[from] CacheError),
    #[error("internal error: {0:#}")]
    Internal(anyhow::Error),
}

impl Error {
    /// HTTP status code Phase 3's response layer will use. Internal/Db errors
    /// map to 500; auth issues to 401/403; validation to 400.
    pub fn http_status(&self) -> u16 {
        match self {
            Error::NotFound => 404,
            Error::Unauthorized => 401,
            Error::Forbidden => 403,
            Error::BadRequest(_) => 400,
            Error::Conflict(_) => 409,
            Error::Locked => 423,
            Error::Ocs { code, .. } => *code,
            Error::Config(_) | Error::ConfigValidation(_) => 500,
            Error::Db(_) => 500,
            Error::Cache(_) => 500,
            Error::Internal(_) => 500,
        }
    }

    /// A short, safe message that is OK to expose to clients. Internal errors
    /// produce a generic message; specific errors expose their reason.
    pub fn client_message(&self) -> String {
        match self {
            Error::NotFound => "Not Found".into(),
            Error::Unauthorized => "Unauthorized".into(),
            Error::Forbidden => "Forbidden".into(),
            Error::BadRequest(m) => m.clone(),
            Error::Conflict(m) => m.clone(),
            Error::Locked => "Locked".into(),
            Error::Ocs { message, .. } => message.clone(),
            Error::Config(_) | Error::ConfigValidation(_) | Error::Db(_) | Error::Cache(_) | Error::Internal(_) => {
                "Internal Server Error".into()
            }
        }
    }
}

pub type CoreResult<T> = Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_status_mapping() {
        assert_eq!(Error::NotFound.http_status(), 404);
        assert_eq!(Error::Unauthorized.http_status(), 401);
        assert_eq!(Error::Forbidden.http_status(), 403);
        assert_eq!(Error::BadRequest("x".into()).http_status(), 400);
        assert_eq!(Error::Conflict("x".into()).http_status(), 409);
        assert_eq!(Error::Locked.http_status(), 423);
        assert_eq!(Error::Ocs { code: 418, message: "teapot".into() }.http_status(), 418);
    }

    #[test]
    fn internal_errors_hide_details_in_client_message() {
        let e = Error::Internal(anyhow::anyhow!("postgres exploded: rows=42, table=oc_users"));
        assert_eq!(e.client_message(), "Internal Server Error");
        // Display still shows the chain (for logging).
        assert!(format!("{e:#}").contains("postgres exploded"));
    }

    #[test]
    fn bad_request_message_passes_through() {
        let e = Error::BadRequest("missing field 'email'".into());
        assert_eq!(e.client_message(), "missing field 'email'");
    }

    #[test]
    fn from_db_error_works() {
        let dberr = DbError::InvalidUrl("nope".into());
        let e: Error = dberr.into();
        assert!(matches!(e, Error::Db(_)));
        assert_eq!(e.http_status(), 500);
    }
}
```

- [ ] **Step 4: Write `crates/rustcloud-core/src/appconfig.rs`**

```rust
//! Runtime app-config service backed by `oc_appconfig` + a write-through cache.
//!
//! Schema-compatible with Nextcloud (spec §5.2). Reads check cache first;
//! misses fall through to DB and prime the cache. Writes go to DB then
//! invalidate the cache key.

use crate::error::{CoreResult, Error};
use rustcloud_cache::Cache;
use rustcloud_db::DbPool;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct AppConfigService {
    pool: DbPool,
    cache: Arc<dyn Cache>,
    table: String,
    instance_id: String,
}

impl std::fmt::Debug for AppConfigService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppConfigService")
            .field("table", &self.table)
            .field("instance_id", &self.instance_id)
            .finish()
    }
}

impl AppConfigService {
    pub fn new(pool: DbPool, cache: Arc<dyn Cache>, prefix: &str, instance_id: &str) -> Self {
        Self {
            pool,
            cache,
            table: format!("{prefix}appconfig"),
            instance_id: instance_id.to_string(),
        }
    }

    fn cache_key(&self, appid: &str, key: &str) -> String {
        format!("{}:appconfig:{appid}:{key}", self.instance_id)
    }

    pub async fn get(&self, appid: &str, key: &str) -> CoreResult<Option<String>> {
        let ck = self.cache_key(appid, key);
        if let Some(bytes) = self.cache.get(&ck).await? {
            // Empty bytes = sentinel for "known missing"
            if bytes.is_empty() {
                return Ok(None);
            }
            return Ok(Some(String::from_utf8_lossy(&bytes).into_owned()));
        }
        let v = self.fetch_db(appid, key).await?;
        let sentinel: &[u8] = match &v {
            Some(s) => s.as_bytes(),
            None => &[],
        };
        let _ = self.cache.set(&ck, sentinel, Some(Duration::from_secs(60))).await;
        Ok(v)
    }

    pub async fn set(&self, appid: &str, key: &str, value: &str) -> CoreResult<()> {
        self.write_db(appid, key, value).await?;
        // Invalidate; next read will repopulate.
        let _ = self.cache.del(&self.cache_key(appid, key)).await;
        Ok(())
    }

    async fn fetch_db(&self, appid: &str, key: &str) -> CoreResult<Option<String>> {
        let select_q = match &self.pool {
            DbPool::Postgres(_) => format!(
                "SELECT configvalue FROM {} WHERE appid = $1 AND configkey = $2",
                self.table
            ),
            _ => format!(
                "SELECT configvalue FROM {} WHERE appid = ? AND configkey = ?",
                self.table
            ),
        };
        let row: Option<(String,)> = match &self.pool {
            DbPool::Sqlite(p) => sqlx::query_as(&select_q)
                .bind(appid)
                .bind(key)
                .fetch_optional(p)
                .await
                .map_err(|e| Error::Db(rustcloud_db::DbError::Sqlx(e)))?,
            DbPool::MySql(p) => sqlx::query_as(&select_q)
                .bind(appid)
                .bind(key)
                .fetch_optional(p)
                .await
                .map_err(|e| Error::Db(rustcloud_db::DbError::Sqlx(e)))?,
            DbPool::Postgres(p) => sqlx::query_as(&select_q)
                .bind(appid)
                .bind(key)
                .fetch_optional(p)
                .await
                .map_err(|e| Error::Db(rustcloud_db::DbError::Sqlx(e)))?,
        };
        Ok(row.map(|(v,)| v))
    }

    async fn write_db(&self, appid: &str, key: &str, value: &str) -> CoreResult<()> {
        // UPSERT — dialect-specific.
        match &self.pool {
            DbPool::Sqlite(p) => {
                let q = format!(
                    "INSERT INTO {table} (appid, configkey, configvalue) VALUES (?, ?, ?) \
                     ON CONFLICT(appid, configkey) DO UPDATE SET configvalue = excluded.configvalue",
                    table = self.table
                );
                sqlx::query(&q)
                    .bind(appid)
                    .bind(key)
                    .bind(value)
                    .execute(p)
                    .await
                    .map_err(|e| Error::Db(rustcloud_db::DbError::Sqlx(e)))?;
            }
            DbPool::MySql(p) => {
                let q = format!(
                    "INSERT INTO {table} (appid, configkey, configvalue) VALUES (?, ?, ?) \
                     ON DUPLICATE KEY UPDATE configvalue = VALUES(configvalue)",
                    table = self.table
                );
                sqlx::query(&q)
                    .bind(appid)
                    .bind(key)
                    .bind(value)
                    .execute(p)
                    .await
                    .map_err(|e| Error::Db(rustcloud_db::DbError::Sqlx(e)))?;
            }
            DbPool::Postgres(p) => {
                let q = format!(
                    "INSERT INTO {table} (appid, configkey, configvalue) VALUES ($1, $2, $3) \
                     ON CONFLICT (appid, configkey) DO UPDATE SET configvalue = EXCLUDED.configvalue",
                    table = self.table
                );
                sqlx::query(&q)
                    .bind(appid)
                    .bind(key)
                    .bind(value)
                    .execute(p)
                    .await
                    .map_err(|e| Error::Db(rustcloud_db::DbError::Sqlx(e)))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustcloud_cache::MemoryCache;
    use rustcloud_config::{CacheConfig, DbType, FileConfig};
    use rustcloud_db::{core_set, MigrationRunner};
    use secrecy::SecretString;
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn cfg_sqlite(path: PathBuf) -> FileConfig {
        FileConfig {
            instanceid: "test".into(),
            secret: SecretString::new("s".into()),
            passwordsalt: SecretString::new("ps".into()),
            installed: true,
            version: "31.0.0.0".into(),
            versionstring: "31.0.0".into(),
            dbtype: DbType::Sqlite,
            dbhost: None,
            dbport: None,
            dbname: path.to_string_lossy().into(),
            dbuser: None,
            dbpassword: None,
            dbtableprefix: "oc_".into(),
            db_pool_max: 4,
            datadirectory: "/tmp".into(),
            trusted_domains: vec!["localhost".into()],
            trusted_proxies: vec![],
            overwrite_cli_url: None,
            overwrite_protocol: None,
            overwrite_host: None,
            loglevel: "info".into(),
            logfile: None,
            default_language: "en".into(),
            bind_address: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
            cache: CacheConfig::default(),
        }
    }

    async fn fresh() -> AppConfigService {
        let dir = tempdir().unwrap();
        let cfg = cfg_sqlite(dir.path().join("ac.db"));
        let pool = DbPool::connect(&cfg).await.unwrap();
        let mut runner = MigrationRunner::new(&pool, &cfg.dbtableprefix);
        runner.register(core_set());
        runner.run().await.unwrap();
        let cache: Arc<dyn Cache> = Arc::new(MemoryCache::new());
        // Keep dir alive in test by leaking — small leak in tests is fine.
        std::mem::forget(dir);
        AppConfigService::new(pool, cache, &cfg.dbtableprefix, &cfg.instanceid)
    }

    #[tokio::test]
    async fn get_missing_returns_none() {
        let ac = fresh().await;
        assert_eq!(ac.get("files", "no-such-key").await.unwrap(), None);
    }

    #[tokio::test]
    async fn set_then_get_roundtrips() {
        let ac = fresh().await;
        ac.set("files", "max_upload", "1024").await.unwrap();
        assert_eq!(ac.get("files", "max_upload").await.unwrap(), Some("1024".to_string()));
    }

    #[tokio::test]
    async fn set_upserts_existing_key() {
        let ac = fresh().await;
        ac.set("files", "max_upload", "1024").await.unwrap();
        ac.set("files", "max_upload", "2048").await.unwrap();
        assert_eq!(ac.get("files", "max_upload").await.unwrap(), Some("2048".to_string()));
    }

    #[tokio::test]
    async fn cache_is_used_on_second_read() {
        let ac = fresh().await;
        ac.set("files", "k", "v").await.unwrap();
        let _ = ac.get("files", "k").await.unwrap(); // populates cache
        // Mutate DB directly to verify next read hits cache, not DB.
        let direct_q = "UPDATE oc_appconfig SET configvalue = 'BYPASSED' WHERE appid = 'files' AND configkey = 'k'";
        if let DbPool::Sqlite(p) = &ac.pool {
            sqlx::query(direct_q).execute(p).await.unwrap();
        }
        // Cache still returns the original.
        assert_eq!(ac.get("files", "k").await.unwrap(), Some("v".to_string()));
    }

    #[tokio::test]
    async fn missing_key_is_cached_as_sentinel() {
        let ac = fresh().await;
        // First miss: hits DB.
        assert_eq!(ac.get("files", "absent").await.unwrap(), None);
        // Insert directly into DB; the cached miss-sentinel should still hide it.
        let direct_q = "INSERT INTO oc_appconfig (appid, configkey, configvalue) VALUES ('files', 'absent', 'sneaky')";
        if let DbPool::Sqlite(p) = &ac.pool {
            sqlx::query(direct_q).execute(p).await.unwrap();
        }
        assert_eq!(ac.get("files", "absent").await.unwrap(), None);
    }
}
```

- [ ] **Step 5: Write `crates/rustcloud-core/src/lib.rs`**

```rust
//! Composition crate for the Rustcloud substrate. Holds `AppState`, the
//! unified `Error` type, the runtime `AppConfigService`, and the
//! `BootstrapHook` extension point.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §4.1.

mod appconfig;
mod bootstrap;
mod error;
mod state;

pub use appconfig::AppConfigService;
pub use bootstrap::{boxed_hook, BootstrapHook, BootstrapRegistry};
pub use error::{CoreResult, Error};
pub use state::{AppState, AppStateBuilder};
```

Create placeholder stubs for Task 13:

`crates/rustcloud-core/src/bootstrap.rs`:
```rust
// Implemented in Task 13.

#[derive(Default)]
pub struct BootstrapRegistry;
pub type BootstrapHook = Box<dyn Send>;
```

`crates/rustcloud-core/src/state.rs`:
```rust
// Implemented in Task 13.

#[derive(Clone, Default)]
pub struct AppState;

#[derive(Default)]
pub struct AppStateBuilder;
```

- [ ] **Step 6: Run the tests**

```
cargo test -p rustcloud-core --lib
```

Expected: 9 tests pass (4 error + 5 appconfig).

- [ ] **Step 7: Commit**

```
git add Cargo.toml crates/rustcloud-core
git commit -m "feat(core): add Error type and cache-backed AppConfigService

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 13: rustcloud-core — BootstrapHook + AppState + AppStateBuilder

**Files:**
- Modify: `crates/rustcloud-i18n/src/lib.rs` (re-export `load_all`)
- Modify: `crates/rustcloud-core/src/bootstrap.rs`
- Modify: `crates/rustcloud-core/src/state.rs`

`AppState` is the clone-cheap handle every later phase passes around. `BootstrapHook` is a future-producing closure registered at startup; the registry drains and runs hooks before traffic is served. **`BootstrapHook` takes the `AppState` by value** — `AppState` is clone-cheap (`Arc`-backed internally), and owning the state in the hook removes a thorny HRTB lifetime that closures struggle with.

- [ ] **Step 1: Re-export `load_all` from `rustcloud-i18n`**

`state.rs` (next step) calls `rustcloud_i18n::load_all(...)`. Add it to the crate's public surface.

Replace `crates/rustcloud-i18n/src/lib.rs`:
```rust
//! Internationalization for Rustcloud.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §9.2.

mod catalog;
mod locale;
mod service;

pub use catalog::{load_all, Catalog, CatalogError};
pub use locale::{resolve, Locale};
pub use service::I18n;
```

Quick sanity check:
```
cargo build -p rustcloud-i18n
```
Expected: clean.

- [ ] **Step 2: Write `crates/rustcloud-core/src/bootstrap.rs`**

Replace the stub:

```rust
//! Bootstrap-hook registry. Apps register a future-producing closure here;
//! `AppStateBuilder::build()` drains the registry and runs each hook in order.

use crate::error::CoreResult;
use crate::state::AppState;
use std::future::Future;
use std::pin::Pin;

/// A registered bootstrap action. Receives an owned `AppState` clone (cheap —
/// `AppState` is `Arc`-backed) for setup (registering capability providers,
/// running migrations, seeding config). Returns a future that resolves when
/// setup is complete.
///
/// Use the `boxed_hook` helper below to wrap an ergonomic async closure
/// instead of authoring the `Pin<Box<...>>` coercion by hand.
pub type BootstrapHook = Box<
    dyn FnOnce(AppState) -> Pin<Box<dyn Future<Output = CoreResult<()>> + Send>> + Send,
>;

/// Wrap an `async` closure into a `BootstrapHook`. The closure takes the
/// `AppState` by value (clone-cheap) and returns any future of `CoreResult<()>`.
///
/// ```ignore
/// let hook = boxed_hook(|state| async move {
///     state.appconfig.set("core", "ready", "1").await?;
///     Ok(())
/// });
/// ```
pub fn boxed_hook<F, Fut>(f: F) -> BootstrapHook
where
    F: FnOnce(AppState) -> Fut + Send + 'static,
    Fut: Future<Output = CoreResult<()>> + Send + 'static,
{
    Box::new(move |state| Box::pin(f(state)))
}

/// Holds pending hooks. Cleared as hooks run during `AppStateBuilder::build`.
#[derive(Default)]
pub struct BootstrapRegistry {
    hooks: Vec<BootstrapHook>,
}

impl BootstrapRegistry {
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    pub fn register(&mut self, hook: BootstrapHook) {
        self.hooks.push(hook);
    }

    pub fn len(&self) -> usize {
        self.hooks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    /// Drains and runs all registered hooks in registration order.
    /// Each hook receives a fresh `AppState` clone.
    pub async fn run(&mut self, state: &AppState) -> CoreResult<()> {
        let hooks = std::mem::take(&mut self.hooks);
        for hook in hooks {
            hook(state.clone()).await?;
        }
        Ok(())
    }
}

impl std::fmt::Debug for BootstrapRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BootstrapRegistry")
            .field("hooks", &self.hooks.len())
            .finish()
    }
}
```

- [ ] **Step 3: Write `crates/rustcloud-core/src/state.rs`**

Replace the stub with the final, clean version:

```rust
//! `AppState` — the clone-cheap composition handle.

use crate::appconfig::AppConfigService;
use crate::bootstrap::BootstrapRegistry;
use crate::error::{CoreResult, Error};
use rustcloud_cache::{Cache, MemoryCache};
use rustcloud_config::FileConfig;
use rustcloud_db::{core_set, DbPool, MigrationRunner};
use rustcloud_i18n::I18n;
use rustcloud_ocs::CapabilityProvider;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Application-wide composition handle. All fields are `Arc`- or `Clone`-backed
/// so cloning is cheap.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<FileConfig>,
    pub pool: DbPool,
    pub cache: Arc<dyn Cache>,
    pub i18n: Arc<I18n>,
    pub appconfig: AppConfigService,
    pub capability_providers: Arc<Mutex<Vec<Arc<dyn CapabilityProvider>>>>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("instance_id", &self.config.instanceid)
            .field("dbtype", &self.config.dbtype.as_str())
            .finish()
    }
}

impl AppState {
    /// Convenience: register a capability provider at runtime.
    pub async fn register_capability_provider(&self, p: Arc<dyn CapabilityProvider>) {
        self.capability_providers.lock().await.push(p);
    }
}

/// Builder that loads / connects everything and produces an `AppState`.
pub struct AppStateBuilder {
    config: Arc<FileConfig>,
    catalog_root: Option<std::path::PathBuf>,
    cache: Option<Arc<dyn Cache>>,
    registry: BootstrapRegistry,
}

impl AppStateBuilder {
    pub fn new(config: FileConfig) -> Self {
        Self {
            config: Arc::new(config),
            catalog_root: None,
            cache: None,
            registry: BootstrapRegistry::new(),
        }
    }

    pub fn with_catalog_root(mut self, p: impl Into<std::path::PathBuf>) -> Self {
        self.catalog_root = Some(p.into());
        self
    }

    pub fn with_cache(mut self, c: Arc<dyn Cache>) -> Self {
        self.cache = Some(c);
        self
    }

    pub fn with_hook(mut self, hook: crate::bootstrap::BootstrapHook) -> Self {
        self.registry.register(hook);
        self
    }

    /// Build the `AppState`:
    /// 1. Connect the DB pool.
    /// 2. Run core migrations.
    /// 3. Load i18n catalogs (no-op if `catalog_root` unset or missing).
    /// 4. Construct cache (default: `MemoryCache`).
    /// 5. Construct `AppConfigService`.
    /// 6. Run registered hooks (each gets a cheap `AppState` clone).
    pub async fn build(mut self) -> CoreResult<AppState> {
        let pool = DbPool::connect(&self.config).await?;

        let mut runner = MigrationRunner::new(&pool, &self.config.dbtableprefix);
        runner.register(core_set());
        runner.run().await?;

        let i18n = match &self.catalog_root {
            Some(root) => {
                let catalogs = rustcloud_i18n::load_all(root)
                    .map_err(|e| Error::Internal(anyhow::anyhow!("i18n load: {e}")))?;
                Arc::new(I18n::new(
                    catalogs,
                    rustcloud_i18n::Locale::new(&self.config.default_language),
                ))
            }
            None => Arc::new(I18n::new(
                std::collections::HashMap::new(),
                rustcloud_i18n::Locale::new(&self.config.default_language),
            )),
        };

        let cache = self.cache.unwrap_or_else(|| Arc::new(MemoryCache::new()));

        let appconfig = AppConfigService::new(
            pool.clone(),
            cache.clone(),
            &self.config.dbtableprefix,
            &self.config.instanceid,
        );

        let state = AppState {
            config: self.config.clone(),
            pool,
            cache,
            i18n,
            appconfig,
            capability_providers: Arc::new(Mutex::new(Vec::new())),
        };

        self.registry.run(&state).await?;
        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::BootstrapHook;
    use rustcloud_config::{CacheConfig, DbType};
    use secrecy::SecretString;
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn cfg_sqlite(path: PathBuf) -> FileConfig {
        FileConfig {
            instanceid: "test".into(),
            secret: SecretString::new("s".into()),
            passwordsalt: SecretString::new("ps".into()),
            installed: true,
            version: "31.0.0.0".into(),
            versionstring: "31.0.0".into(),
            dbtype: DbType::Sqlite,
            dbhost: None,
            dbport: None,
            dbname: path.to_string_lossy().into(),
            dbuser: None,
            dbpassword: None,
            dbtableprefix: "oc_".into(),
            db_pool_max: 4,
            datadirectory: "/tmp".into(),
            trusted_domains: vec!["localhost".into()],
            trusted_proxies: vec![],
            overwrite_cli_url: None,
            overwrite_protocol: None,
            overwrite_host: None,
            loglevel: "info".into(),
            logfile: None,
            default_language: "en".into(),
            bind_address: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
            cache: CacheConfig::default(),
        }
    }

    #[tokio::test]
    async fn build_assembles_state_from_minimal_config() {
        let dir = tempdir().unwrap();
        let cfg = cfg_sqlite(dir.path().join("state.db"));
        let state = AppStateBuilder::new(cfg).build().await.unwrap();
        assert_eq!(state.config.instanceid, "test");
        assert_eq!(state.pool.dialect(), "sqlite");
        assert!(state.i18n.available_locales().is_empty());
        // appconfig should be usable.
        state.appconfig.set("test", "k", "v").await.unwrap();
        assert_eq!(state.appconfig.get("test", "k").await.unwrap(), Some("v".into()));
    }

    #[tokio::test]
    async fn build_runs_registered_hooks() {
        use crate::boxed_hook;
        let dir = tempdir().unwrap();
        let cfg = cfg_sqlite(dir.path().join("state.db"));
        // Hook receives an owned AppState clone and writes a sentinel.
        let hook = boxed_hook(|state: AppState| async move {
            state.appconfig.set("core", "bootstrapped", "yes").await?;
            Ok(())
        });
        let state = AppStateBuilder::new(cfg).with_hook(hook).build().await.unwrap();
        assert_eq!(
            state.appconfig.get("core", "bootstrapped").await.unwrap(),
            Some("yes".to_string())
        );
    }

    #[tokio::test]
    async fn register_capability_provider_appends() {
        use rustcloud_ocs::CoreCapabilities;
        let dir = tempdir().unwrap();
        let cfg = cfg_sqlite(dir.path().join("state.db"));
        let state = AppStateBuilder::new(cfg).build().await.unwrap();
        state
            .register_capability_provider(Arc::new(CoreCapabilities::default()))
            .await;
        let guard = state.capability_providers.lock().await;
        assert_eq!(guard.len(), 1);
        assert_eq!(guard[0].namespace(), "core");
    }
}
```

- [ ] **Step 4: Run the tests**

```
cargo test -p rustcloud-core --lib
```

Expected: 12 tests pass (4 error + 5 appconfig + 3 state).

- [ ] **Step 5: Commit**

```
git add crates/rustcloud-core/src crates/rustcloud-i18n/src/lib.rs
git commit -m "feat(core): add AppState, AppStateBuilder, and BootstrapRegistry

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 14: rustcloud-core — integration test for full assembly

**Files:**
- Create: `crates/rustcloud-core/tests/app_state_build.rs`

End-to-end test that:
1. Writes a minimal config.toml and an l10n directory in a tempdir.
2. Calls `AppStateBuilder::new(config).with_catalog_root(l10n_dir).with_hook(...).build()`.
3. Asserts the pool dialect, i18n lookup, appconfig persistence, capability registration, and bootstrap hook execution.

- [ ] **Step 1: Write the integration test**

Create `crates/rustcloud-core/tests/app_state_build.rs`:

```rust
//! End-to-end assembly proof for `AppStateBuilder`.

use rustcloud_cache::Cache;
use rustcloud_config::{CacheConfig, DbType, FileConfig};
use rustcloud_core::{AppState, AppStateBuilder};
use rustcloud_i18n::Locale;
use rustcloud_ocs::{aggregate, CapabilityContext, CapabilityProvider, CoreCapabilities};
use secrecy::SecretString;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::tempdir;

fn cfg_sqlite(path: PathBuf) -> FileConfig {
    FileConfig {
        instanceid: "it".into(),
        secret: SecretString::new("s".into()),
        passwordsalt: SecretString::new("ps".into()),
        installed: true,
        version: "31.0.0.0".into(),
        versionstring: "31.0.0".into(),
        dbtype: DbType::Sqlite,
        dbhost: None,
        dbport: None,
        dbname: path.to_string_lossy().into(),
        dbuser: None,
        dbpassword: None,
        dbtableprefix: "oc_".into(),
        db_pool_max: 4,
        datadirectory: "/tmp".into(),
        trusted_domains: vec!["localhost".into()],
        trusted_proxies: vec![],
        overwrite_cli_url: None,
        overwrite_protocol: None,
        overwrite_host: None,
        loglevel: "info".into(),
        logfile: None,
        default_language: "en".into(),
        bind_address: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        cache: CacheConfig::default(),
    }
}

fn seed_de_po(root: &std::path::Path) {
    let app = root.join("core");
    fs::create_dir_all(&app).unwrap();
    fs::write(
        app.join("de.po"),
        r#"msgid ""
msgstr "Content-Type: text/plain; charset=UTF-8\n"

msgid "Welcome"
msgstr "Willkommen"
"#,
    )
    .unwrap();
}

#[tokio::test]
async fn full_assembly_works_end_to_end() {
    let dir = tempdir().unwrap();
    let l10n_dir = dir.path().join("l10n");
    seed_de_po(&l10n_dir);

    let cfg = cfg_sqlite(dir.path().join("it.db"));

    // Hook writes a sentinel that future tests can rely on. `boxed_hook`
    // wraps an async closure into the `BootstrapHook` shape.
    let hook = rustcloud_core::boxed_hook(|state: AppState| async move {
        state.appconfig.set("core", "phase2_built", "1").await?;
        Ok(())
    });

    let state = AppStateBuilder::new(cfg)
        .with_catalog_root(&l10n_dir)
        .with_hook(hook)
        .build()
        .await
        .unwrap();

    // DbPool is connected and migrations applied — appconfig works.
    assert_eq!(state.appconfig.get("core", "phase2_built").await.unwrap(), Some("1".into()));

    // i18n catalogs loaded; lookup hits German translation.
    let de = Locale::new("de");
    let s = state.i18n.t("core", &de, "Welcome", &[]);
    assert_eq!(s, "Willkommen");
    let s_fallback = state.i18n.t("core", &de, "Bye", &[]);
    assert_eq!(s_fallback, "Bye"); // fallback to source

    // Capability provider registration + aggregator end-to-end.
    state
        .register_capability_provider(Arc::new(CoreCapabilities::default()))
        .await;
    let providers = state.capability_providers.lock().await.clone();
    let payload = aggregate(
        &providers,
        &CapabilityContext::default(),
        state.cache.clone(),
        &state.config.versionstring,
        &state.config.instanceid,
    )
    .await
    .unwrap();
    assert_eq!(payload.body["capabilities"]["core"]["pollinterval"], 60);

    // Cache is shared and writable.
    state.cache.set("smoke", b"ok", None).await.unwrap();
    assert_eq!(state.cache.get("smoke").await.unwrap(), Some(b"ok".to_vec()));
}
```

- [ ] **Step 2: Run the integration test**

```
cargo test -p rustcloud-core --test app_state_build
```

Expected: 1 test passes (`full_assembly_works_end_to_end`).

- [ ] **Step 3: Run the full workspace tests**

```
cargo xtask check-all
```

Expected: all tests across all crates pass (workspace total around 50+).

- [ ] **Step 4: Commit**

```
git add crates/rustcloud-core/tests/app_state_build.rs
git commit -m "test(core): end-to-end AppStateBuilder integration test

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 15: rustcloud-server — wire AppState into bootstrap

**Files:**
- Modify: `crates/rustcloud-server/Cargo.toml`
- Modify: `crates/rustcloud-server/src/main.rs`

Replace the ad-hoc DbPool wiring in `migrate` with the unified `AppStateBuilder`. The migrate path still runs migrations (the builder does this internally), then exits — the AppState assembly is the real verifier that everything composes.

- [ ] **Step 1: Add `rustcloud-core` to the server's deps**

Modify `crates/rustcloud-server/Cargo.toml` — append to `[dependencies]`:

```toml
rustcloud-core.workspace = true
```

(`rustcloud-db`, `rustcloud-config` are already declared from Phase 1 — keep them; `rustcloud-core` will re-export some of those types but the binary still uses both directly for now.)

- [ ] **Step 2: Replace the migrate-arm with AppStateBuilder**

Modify `crates/rustcloud-server/src/main.rs` — replace the `Cmd::Migrate` arm:

Find:
```rust
        Cmd::Migrate => {
            let config = rustcloud_config::load(&cli.config, &[])?;
            info!(dbtype = %config.dbtype.as_str(), "connecting to database");

            let pool = rustcloud_db::DbPool::connect(&config).await?;
            info!(dialect = pool.dialect(), "connected");

            let mut runner = rustcloud_db::MigrationRunner::new(&pool, &config.dbtableprefix);
            runner.register(rustcloud_db::core_set());
            let applied = runner.run().await?;
            info!(applied, "migrations complete");

            pool.close().await;
            Ok(())
        }
```

Replace with:
```rust
        Cmd::Migrate => {
            let config = rustcloud_config::load(&cli.config, &[])?;
            info!(
                dbtype = %config.dbtype.as_str(),
                "assembling AppState (this runs migrations)"
            );

            // The builder runs migrations internally; we don't need to call the
            // MigrationRunner separately. Build, then close the pool and exit.
            let state = rustcloud_core::AppStateBuilder::new(config).build().await?;
            info!(dialect = state.pool.dialect(), "AppState ready; closing pool");
            state.pool.close().await;
            info!("migrate complete");
            Ok(())
        }
```

The `Cmd::Serve` arm remains stubbed (`bail!`). It will become the primary entry point in Phase 3.

- [ ] **Step 3: Build + smoke-test against a SQLite fixture**

Run:
```
cargo build
```

Then create a fixture TOML and run migrate:

PowerShell:
```powershell
@'
instanceid = "phase2smoke"
secret = "s"
passwordsalt = "ps"
installed = true
version = "31.0.0.0"
versionstring = "31.0.0"
dbtype = "sqlite"
dbname = "phase2-smoke.db"
datadirectory = "./data"
trusted_domains = ["localhost"]
'@ | Out-File -Encoding utf8 fixture.toml

cargo run -p rustcloud-server -- --config fixture.toml migrate
```

Expected: log lines `assembling AppState ... dbtype=sqlite`, `AppState ready; closing pool`, `migrate complete`. The `phase2-smoke.db` file is created with `oc_appconfig` and `oc_migrations` tables.

Clean up:
```powershell
Remove-Item fixture.toml
Remove-Item phase2-smoke.db
```

- [ ] **Step 4: Run the full check**

```
cargo xtask check-all
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```
git add crates/rustcloud-server
git commit -m "feat(server): use AppStateBuilder in migrate path

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 16: Phase 2 acceptance + README + changelog

**Files:**
- Modify: `README.md`
- Create: `docs/superpowers/plans/2026-05-10-platform-core-phase-2-cross-cutting.changelog.md`

- [ ] **Step 1: Update the README**

Replace `README.md`:

```markdown
# Rustcloud

A Rust port of [Nextcloud server](https://github.com/nextcloud/server), with a Dioxus frontend.

**Status:** early — Phases 1 (Foundations) and 2 (Cross-cutting) complete. The binary can boot, load layered config, connect to SQLite/MySQL/Postgres, assemble a full `AppState` (DbPool + Cache + I18n + AppConfig + capability providers), run core migrations, and exit. No HTTP surface yet; Phase 3 adds it.

See `docs/superpowers/specs/` for design specs and `docs/superpowers/plans/` for implementation plans.

## Quick start

```bash
# 1. Copy the example config and edit it.
cp config/config.toml.example config/config.toml
# (Set `installed = true` and pick a `dbtype`.)

# 2a. SQLite: nothing else needed. Skip to step 3.

# 2b. MySQL or Postgres: start the dev DBs.
cargo xtask up

# 3. Run migrations.
cargo run -p rustcloud-server -- migrate
```

## Development

```bash
cargo xtask check-all     # fmt + clippy + tests (SQLite only)
cargo xtask up            # start MySQL + Postgres for multi-dialect tests
cargo test -p rustcloud-db --test migrate_end_to_end -- --include-ignored
cargo xtask down          # stop dev DBs
```

## Workspace layout

- `crates/rustcloud-config` — layered TOML config loader (figment-based).
- `crates/rustcloud-cache` — `Cache` trait + `MemoryCache` + `TypedCache<T>`.
- `crates/rustcloud-db` — `DbPool` enum over Sqlite/MySql/Postgres, `MigrationRunner`.
- `crates/rustcloud-i18n` — gettext `.po` loader, `Locale`, `I18n` service.
- `crates/rustcloud-ocs` — OCS envelope (JSON/XML), `CapabilityProvider` aggregator with cache-backed ETag.
- `crates/rustcloud-core` — `AppState`, `AppConfigService`, `Error`, `BootstrapHook`.
- `crates/rustcloud-server` — the binary; CLI, tracing, lifecycle.
- `xtask/` — project automation (`check-all`, `up`, `down`).
- `migrations/core/` — core SQL migrations, per-dialect.
- `l10n/<app>/<locale>.po` — translation catalogs.

Future phases add `rustcloud-http` (Phase 3) and `rustcloud-ui` (Phase 4).

## License

AGPL-3.0-or-later.
```

- [ ] **Step 2: Write the Phase 2 changelog**

Create `docs/superpowers/plans/2026-05-10-platform-core-phase-2-cross-cutting.changelog.md`:

```markdown
# Phase 2 (Cross-cutting) — Changelog

Completed: 2026-05-10

## What works

- **`rustcloud-cache`**: `Cache` trait, `MemoryCache` impl with lazy TTL expiry, `TypedCache<T>` serde wrapper.
- **`rustcloud-i18n`**: `Locale` type with Accept-Language resolution, `Catalog` loader for gettext `.po` files (polib), `I18n` service with `t()`/`tn()` and source-string fallback. Seed `l10n/core/de.po` provided.
- **`rustcloud-ocs`**: `OcsResponse<T>` envelope rendering to JSON or XML; `Format` content negotiation; `CapabilityProvider` trait + cache-backed aggregator with stable ETag; `CoreCapabilities` provider matching Nextcloud's `core` namespace shape.
- **`rustcloud-core`**: `Error` enum with HTTP status mapping + client-safe message extraction; `AppConfigService` (cache-write-through against `oc_appconfig`); `BootstrapRegistry` + `BootstrapHook` — the extension point future apps will use; `AppState` + `AppStateBuilder` that assembles everything end-to-end.
- **`rustcloud-server`**: `migrate` subcommand now uses `AppStateBuilder::build()`, proving the assembly path.

## What's deferred

- HTTP surface (axum router + middleware + session + CSRF + `status.php` + login + OCS routes): Phase 3.
- Dioxus Fullstack UI: Phase 4.
- App/plugin framework (lifecycle hooks beyond `BootstrapHook`, dependency resolution, settings page registration): later sub-project.
- Redis cache backend: micro-sub-project before multi-node deploy.
- Background job runner / cron.

## Known limitations

- `MemoryCache` TTL expiry is lazy on read (no background sweeper). Acceptable for single-node; multi-node deploys will use Redis.
- The OCS XML serializer is hand-rolled tree-walk rather than `quick-xml::Serializer` — easier to control the exact element shape clients expect.
- `I18n` uses the simple English plural rule (`n != 1`). Full plural-form expression support is deferred.
- `Locale` normalizes `en-US` → `en_us`; some external systems use Nextcloud's pre-normalized form (`en_US`) — flag if clients complain.

## Known follow-ups (carried from Phase 1 + new from Phase 2)

- Centralize lint policy (`[workspace.lints]`). Carried.
- Sparse rustdoc on public type-level APIs. Carried; partly addressed for new types.
- `version` subcommand should print git SHA + dialect support (spec §10.2 / §10.5). Carried.
- Test config-builder duplication (`cfg_sqlite`/`base_config`) — now in 5 places. Consolidate before Phase 3.
- `quick-xml` is declared as a workspace dep but the current XML rendering is hand-rolled; either keep the dep for future XML parsing needs or drop it.
- `compute_etag` in `rustcloud-ocs::capabilities` uses `DefaultHasher`, which is documented as not stable across Rust versions. Acceptable for an ETag (clients re-fetch on mismatch) but worth swapping for `blake3` or `xxhash-rust` if a stable cross-version hash matters.
- `AppConfigService::fetch_db` repeats the same `query_as` body three times for the three pool variants. Phase 3 introduces the `db_dispatch!` macro mentioned in the spec; this is its first natural use site.

## Acceptance criteria

| # | Criterion | Status |
|---|---|---|
| Phase 1 #1 | `cargo xtask check-all` against all three backends | ✅ (carry-over) |
| Phase 1 #3 | Binary boots + migrates against all three DBs | ✅ (carry-over; now via `AppStateBuilder`) |
| Phase 1 #9 | Single + multi-dialect tests green | ✅ (carry-over) |
| Phase 2 (a) | Cache trait + memory impl unit-tested | ✅ |
| Phase 2 (b) | I18n catalog loader + service unit-tested | ✅ |
| Phase 2 (c) | OCS envelope renders JSON and XML correctly | ✅ |
| Phase 2 (d) | Capabilities aggregator works in isolation (cached, ETag-keyed) | ✅ |
| Phase 2 (e) | `AppStateBuilder::build()` integration test proves end-to-end assembly | ✅ |
| Spec ac §13 #4 | `/status.php` | Deferred to Phase 3 |
| Spec ac §13 #5 | `/ocs/v2.php/cloud/capabilities` | Endpoint wiring deferred to Phase 3; aggregator works in isolation today |
| Spec ac §13 #6, 7, 8 | Browser/login/middleware | Deferred to Phases 3-4 |
```

- [ ] **Step 3: Commit**

```
git add README.md docs/superpowers/plans/2026-05-10-platform-core-phase-2-cross-cutting.changelog.md
git commit -m "docs: phase 2 acceptance — README + changelog

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Phase 2 Self-Review (executor applies before declaring complete)

After all 16 tasks land, verify against the spec sections this phase implements:

| Spec section | Phase 2 status |
|---|---|
| §5.2 Runtime config (`oc_appconfig`) | ✅ `AppConfigService` cache-backed |
| §7.6 Error → response | ✅ `Error` enum + status mapping; `IntoResponse` deferred to Phase 3 |
| §9.1 Cache trait + MemoryCache | ✅ |
| §9.2 i18n loader + resolver | ✅ |
| §9.3 OCS envelope + capabilities aggregator | ✅ |
| §4.1 AppState | ✅ `AppState` + `AppStateBuilder` |
| §10.1 Bootstrap | Partial — `BootstrapHook` exists; full sequence (HTTP serving, signal handling) is Phase 3 |

If `cargo xtask check-all` is green, Phase 2 is complete.

---
