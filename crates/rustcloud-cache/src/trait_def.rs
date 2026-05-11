//! Cache trait. See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §9.1.

use async_trait::async_trait;
use std::time::Duration;

/// Cache errors. Future backends (Redis, Memcached) may surface transport
/// errors; the memory backend only produces `Io` for I/O-shaped failures
/// such as a value that can't be parsed during `incr`.
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    /// Generic I/O or serialization error reported by a backend.
    #[error("cache I/O error: {0}")]
    Io(String),
}

/// Convenience alias for `Result<T, CacheError>`.
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
    ///
    /// Saturates at `i64::MAX` / `i64::MIN`; do not rely on `incr` for exact
    /// counters near those bounds. Backends are not required to surface
    /// overflow; the memory backend silently caps.
    async fn incr(&self, key: &str, by: i64) -> CacheResult<i64>;

    /// Compare-and-swap. Sets `new` only if the current value equals `old`.
    /// Returns `Ok(true)` on success, `Ok(false)` on mismatch (no error).
    async fn cas(&self, key: &str, old: &[u8], new: &[u8]) -> CacheResult<bool>;
}
