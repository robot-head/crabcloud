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

/// In-process [`Cache`] implementation backed by a tokio-mutex-guarded
/// `HashMap`. Single-node only; cloning shares the same backing store.
#[derive(Debug, Clone, Default)]
pub struct MemoryCache {
    inner: Arc<Mutex<HashMap<String, Entry>>>,
}

impl MemoryCache {
    /// Construct an empty in-process cache.
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
        let entry = Entry {
            value: value.to_vec(),
            expires_at,
        };
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
            Some(entry) if !entry.is_expired(now) => std::str::from_utf8(&entry.value)
                .map_err(|e| CacheError::Io(format!("incr: value not utf-8: {e}")))?
                .parse::<i64>()
                .map_err(|e| CacheError::Io(format!("incr: value not i64: {e}")))?,
            _ => 0,
        };
        let new = current.saturating_add(by);
        let entry = Entry {
            value: new.to_string().into_bytes(),
            expires_at: None,
        };
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
            let entry = Entry {
                value: new.to_vec(),
                expires_at: None,
            };
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
        c.set("k", b"v", Some(Duration::from_millis(20)))
            .await
            .unwrap();
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
