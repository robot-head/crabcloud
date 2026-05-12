//! `SessionStore` — typed wrapper over `Arc<dyn Cache>` for ephemeral
//! per-session blob state (CSRF token, two_factor_passed). Keyed by the
//! authoritative `oc_authtoken` row id.

use crate::session::data::Session;
use crabcloud_cache::{Cache, CacheError};
use std::sync::Arc;
use std::time::Duration;

/// Idle TTL for the ephemeral session blob. Sliding refresh on save.
pub const SESSION_IDLE_TTL: Duration = Duration::from_secs(30 * 60);

/// Typed wrapper over the shared `Cache` for ephemeral session blobs. Keys are
/// scoped by `instance_id` and the authoritative `oc_authtoken` row id.
#[derive(Clone)]
pub struct SessionStore {
    cache: Arc<dyn Cache>,
    instance_id: String,
}

impl SessionStore {
    /// Build a store backed by `cache`, scoping keys with `instance_id`.
    pub fn new(cache: Arc<dyn Cache>, instance_id: impl Into<String>) -> Self {
        Self {
            cache,
            instance_id: instance_id.into(),
        }
    }

    fn key_for_token(&self, token_id: i64) -> String {
        format!("{}:session_blob:{}", self.instance_id, token_id)
    }

    /// Load the ephemeral blob for `token_id`. Returns `Ok(None)` if no entry.
    pub async fn load_for_token(&self, token_id: i64) -> Result<Option<Session>, CacheError> {
        let raw = self.cache.get(&self.key_for_token(token_id)).await?;
        match raw {
            Some(bytes) => {
                let s: Session = serde_json::from_slice(&bytes)
                    .map_err(|e| CacheError::Io(format!("session decode: {e}")))?;
                Ok(Some(s))
            }
            None => Ok(None),
        }
    }

    /// Persist `session` for `token_id` with the idle TTL refreshed.
    pub async fn save_for_token(&self, token_id: i64, session: &Session) -> Result<(), CacheError> {
        let bytes = serde_json::to_vec(session)
            .map_err(|e| CacheError::Io(format!("session encode: {e}")))?;
        self.cache
            .set(
                &self.key_for_token(token_id),
                &bytes,
                Some(SESSION_IDLE_TTL),
            )
            .await
    }

    /// Remove the ephemeral blob for `token_id`. No-op if it doesn't exist.
    pub async fn destroy_for_token(&self, token_id: i64) -> Result<(), CacheError> {
        self.cache.del(&self.key_for_token(token_id)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crabcloud_cache::MemoryCache;

    #[tokio::test]
    async fn save_then_load_roundtrips() {
        let store = SessionStore::new(Arc::new(MemoryCache::new()), "inst1");
        let mut s = Session::new();
        s.csrf_token = "abc".into();
        store.save_for_token(42, &s).await.unwrap();
        let loaded = store.load_for_token(42).await.unwrap().unwrap();
        assert_eq!(loaded.csrf_token, "abc");
    }

    #[tokio::test]
    async fn load_missing_returns_none() {
        let store = SessionStore::new(Arc::new(MemoryCache::new()), "inst1");
        assert!(store.load_for_token(99).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn destroy_removes_blob() {
        let store = SessionStore::new(Arc::new(MemoryCache::new()), "inst1");
        store.save_for_token(7, &Session::new()).await.unwrap();
        store.destroy_for_token(7).await.unwrap();
        assert!(store.load_for_token(7).await.unwrap().is_none());
    }
}
