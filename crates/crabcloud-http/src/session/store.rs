//! `SessionStore` — typed wrapper over `Arc<dyn Cache>` for session payloads.

use crate::session::data::{Session, SessionId};
use crabcloud_cache::{Cache, CacheError};
use std::sync::Arc;
use std::time::Duration;

/// Idle TTL for sessions. Spec §7.3 says 30 min idle, 24 h absolute. Phase 3
/// ships the idle-TTL only; absolute-TTL enforcement is a Phase 4 concern.
pub const SESSION_IDLE_TTL: Duration = Duration::from_secs(30 * 60);

/// Typed wrapper over the shared `Cache` for session payloads. Keys are
/// scoped by `instance_id`.
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

    fn key(&self, id: &SessionId) -> String {
        format!("{}:session:{}", self.instance_id, id.as_str())
    }

    /// Load the session at `id`. Returns `Ok(None)` if the cache has no entry.
    pub async fn load(&self, id: &SessionId) -> Result<Option<Session>, CacheError> {
        let raw = self.cache.get(&self.key(id)).await?;
        match raw {
            Some(bytes) => {
                let s: Session = serde_json::from_slice(&bytes)
                    .map_err(|e| CacheError::Io(format!("session decode: {e}")))?;
                Ok(Some(s))
            }
            None => Ok(None),
        }
    }

    /// Persist `session` at `id` with the idle TTL refreshed.
    pub async fn save(&self, id: &SessionId, session: &Session) -> Result<(), CacheError> {
        let bytes = serde_json::to_vec(session)
            .map_err(|e| CacheError::Io(format!("session encode: {e}")))?;
        self.cache
            .set(&self.key(id), &bytes, Some(SESSION_IDLE_TTL))
            .await
    }

    /// Remove the session at `id`. No-op if it doesn't exist.
    pub async fn destroy(&self, id: &SessionId) -> Result<(), CacheError> {
        self.cache.del(&self.key(id)).await
    }

    fn user_index_key(&self, uid: &str) -> String {
        format!("{}:sessions_by_user:{}", self.instance_id, uid)
    }

    /// Record `id` in the per-user session index so it can later be revoked
    /// via [`destroy_all_for`] / [`destroy_all_for_except`]. Idempotent.
    pub async fn record_for_user(&self, uid: &str, id: &SessionId) -> Result<(), CacheError> {
        // Read-modify-write: concurrent logins for the same user can race and
        // lose an index entry (last writer wins). Accepted per plan §8 — the
        // impact is that `destroy_all_for` may miss one session on a tight
        // race; the sliding SESSION_IDLE_TTL refresh recovers the index once
        // either session re-saves.
        let key = self.user_index_key(uid);
        let current: Vec<String> = match self.cache.get(&key).await? {
            Some(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            None => Vec::new(),
        };
        let mut set: Vec<String> = current;
        if !set.iter().any(|s| s == id.as_str()) {
            set.push(id.as_str().to_string());
        }
        let bytes = serde_json::to_vec(&set)
            .map_err(|e| CacheError::Io(format!("session index encode: {e}")))?;
        self.cache.set(&key, &bytes, Some(SESSION_IDLE_TTL)).await
    }

    /// Destroy every session for `uid` except the optional `except` id. Best
    /// effort: individual destroy failures are swallowed so a single bad row
    /// doesn't block the revoke.
    pub async fn destroy_all_for_except(
        &self,
        uid: &str,
        except: Option<&SessionId>,
    ) -> Result<(), CacheError> {
        let key = self.user_index_key(uid);
        let current: Vec<String> = match self.cache.get(&key).await? {
            Some(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            None => return Ok(()),
        };
        let mut survivors: Vec<String> = Vec::new();
        for id_str in current {
            let id = SessionId(id_str.clone());
            if except.map(|e| e.as_str()) == Some(id.as_str()) {
                survivors.push(id_str);
            } else {
                let _ = self.destroy(&id).await;
            }
        }
        if survivors.is_empty() {
            let _ = self.cache.del(&key).await;
        } else {
            let bytes = serde_json::to_vec(&survivors)
                .map_err(|e| CacheError::Io(format!("session index encode: {e}")))?;
            let _ = self.cache.set(&key, &bytes, Some(SESSION_IDLE_TTL)).await;
        }
        Ok(())
    }

    /// Destroy every session for `uid`. Convenience wrapper over
    /// [`destroy_all_for_except`] with no exception.
    pub async fn destroy_all_for(&self, uid: &str) -> Result<(), CacheError> {
        self.destroy_all_for_except(uid, None).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crabcloud_cache::MemoryCache;

    #[tokio::test]
    async fn save_then_load_roundtrips() {
        let store = SessionStore::new(Arc::new(MemoryCache::new()), "inst1");
        let id = SessionId::new_random();
        let mut s = Session::new();
        s.user_id = Some("alice".into());
        store.save(&id, &s).await.unwrap();
        let loaded = store.load(&id).await.unwrap().unwrap();
        assert_eq!(loaded.user_id.as_deref(), Some("alice"));
        assert_eq!(loaded.csrf_token, s.csrf_token);
    }

    #[tokio::test]
    async fn load_missing_returns_none() {
        let store = SessionStore::new(Arc::new(MemoryCache::new()), "inst1");
        let id = SessionId::new_random();
        assert!(store.load(&id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn destroy_removes_session() {
        let store = SessionStore::new(Arc::new(MemoryCache::new()), "inst1");
        let id = SessionId::new_random();
        store.save(&id, &Session::new()).await.unwrap();
        store.destroy(&id).await.unwrap();
        assert!(store.load(&id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn destroy_all_for_except_kills_others_keeps_current() {
        let store = SessionStore::new(Arc::new(MemoryCache::new()), "inst1");
        let id_a = SessionId::new_random();
        let id_b = SessionId::new_random();
        let mut sa = Session::new();
        sa.user_id = Some("alice".into());
        let mut sb = Session::new();
        sb.user_id = Some("alice".into());
        store.save(&id_a, &sa).await.unwrap();
        store.save(&id_b, &sb).await.unwrap();
        store.record_for_user("alice", &id_a).await.unwrap();
        store.record_for_user("alice", &id_b).await.unwrap();

        store
            .destroy_all_for_except("alice", Some(&id_b))
            .await
            .unwrap();
        assert!(store.load(&id_a).await.unwrap().is_none());
        assert!(store.load(&id_b).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn destroy_all_for_kills_everything_for_user() {
        let store = SessionStore::new(Arc::new(MemoryCache::new()), "inst1");
        let id = SessionId::new_random();
        let mut s = Session::new();
        s.user_id = Some("bob".into());
        store.save(&id, &s).await.unwrap();
        store.record_for_user("bob", &id).await.unwrap();
        store.destroy_all_for("bob").await.unwrap();
        assert!(store.load(&id).await.unwrap().is_none());
    }
}
