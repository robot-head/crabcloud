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
        Self {
            inner,
            prefix: prefix.into(),
            _marker: PhantomData,
        }
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
        let u = User {
            name: "alice".into(),
            age: 30,
        };
        c.set("alice", &u, None).await.unwrap();
        assert_eq!(c.get("alice").await.unwrap(), Some(u));
    }

    #[tokio::test]
    async fn keys_are_prefix_namespaced() {
        let cache: Arc<dyn Cache> = Arc::new(MemoryCache::new());
        let users = TypedCache::<User>::new(cache.clone(), "users:");
        let admins = TypedCache::<User>::new(cache.clone(), "admins:");
        let u_alice = User {
            name: "alice".into(),
            age: 30,
        };
        let a_alice = User {
            name: "alice".into(),
            age: 99,
        };
        users.set("alice", &u_alice, None).await.unwrap();
        admins.set("alice", &a_alice, None).await.unwrap();
        assert_eq!(users.get("alice").await.unwrap(), Some(u_alice));
        assert_eq!(admins.get("alice").await.unwrap(), Some(a_alice));
    }

    #[tokio::test]
    async fn del_removes_typed_value() {
        let c = mk();
        let u = User {
            name: "alice".into(),
            age: 30,
        };
        c.set("alice", &u, None).await.unwrap();
        c.del("alice").await.unwrap();
        assert!(c.get("alice").await.unwrap().is_none());
    }
}
