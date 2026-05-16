//! Adapter glue from `crabcloud-sharing` to `crabcloud-publiclinks`.
//!
//! `crabcloud-publiclinks` cannot depend on `crabcloud-sharing` (sharing
//! depends on publiclinks for `Tokens`/`Passwords`), so the token-lookup
//! plumbing lives here in core where both deps are in scope.
//!
//! `SharesTokenLookup` is the only implementation of `TokenLookup` we ship
//! in production; tests in `crabcloud-publiclinks/tests/auth_layer_e2e.rs`
//! use their own stubs.

use async_trait::async_trait;
use crabcloud_publiclinks::{LinkRow, TokenLookup};
use crabcloud_sharing::Shares;
use std::sync::Arc;

/// Thin adapter that exposes `Shares::resolve_by_token` as a `TokenLookup`.
/// Translates the rich `ShareRow` into the minimal `LinkRow` the auth layer
/// consumes, and flattens `ShareError` into `std::io::Error` so the trait
/// stays free of any sharing-specific error type.
pub struct SharesTokenLookup {
    pub shares: Arc<Shares>,
}

#[async_trait]
impl TokenLookup for SharesTokenLookup {
    async fn lookup(&self, token: &str) -> Result<Option<LinkRow>, std::io::Error> {
        let row = self
            .shares
            .resolve_by_token(token)
            .await
            .map_err(|e| std::io::Error::other(format!("{e}")))?;
        Ok(row.map(|r| LinkRow {
            share_id: r.id,
            owner_uid: r.uid_owner,
            // Link rows store the FULL owner path in `file_target` per Batch
            // B; the auth layer strips the leading `/` before constructing
            // a `StoragePath`.
            owner_path: r.file_target,
            permissions: r.permissions.as_u32(),
            password_hash: r.password_hash,
            expiration: r.expiration,
        }))
    }
}
