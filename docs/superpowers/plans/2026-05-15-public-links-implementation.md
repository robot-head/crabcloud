# Public Links Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship Nextcloud-compatible public links: owners create read-only or upload-only ("file-drop") shareable URLs at `/s/{token}` for anonymous recipients. Recipients see a dedicated viewer page and can drive the link via desktop/mobile clients over `/public.php/dav/files/{token}`. Optional password protection and expiration. Full surface in one sub-project.

**Architecture:** A new `crabcloud-publiclinks` crate owns tokens, Argon2id password hashing, HMAC unlock cookies, in-memory rate limiting, and the `PublicLinkAuthLayer` axum middleware. `crabcloud-sharing` extends `Shares::create`/`update` to handle `share_type=3` by delegating token+password to publiclinks. A new `PublicLinkMountResolver` in `crabcloud-fs` returns exactly one mount per request — a `SharedSubrootStorage` rooted at the linked subtree with the link's permission bits. `SharedSubrootStorage` learns one new rule: create-only links (bit 4 without bit 1) refuse to list or stat children. A dx 0.7 SSR route at `/s/:token` renders the viewer (folder browse, file download, file-drop upload widget, password gate); a parallel public WebDAV router at `/public.php/dav/files/{token}` reuses the existing dav adapter with the new auth context.

**Tech Stack:** Rust 1.95, sqlx 0.8 (sqlite/mysql/postgres), axum 0.8, Dioxus 0.7 fullstack, tower middleware, `argon2` crate, `hmac` + `sha2` crates, `dashmap`. Builds on SP7's `oc_share` schema (no migration), `SharedSubrootStorage`, `Storage::inner_storage` accessor, and the existing dav adapter, View, Filecache, and OCS subrouter.

**Spec:** `docs/superpowers/specs/2026-05-15-public-links-design.md`

---

## Conventions for every batch

- **Branching:** Each batch is a separate PR off `origin/master`. At the start of each batch:
  ```bash
  cd "C:/Users/Matt Stone/git/crabcloud"
  git fetch origin master
  git switch -c sp8/<batch-letter>-<slug> origin/master
  ```
  Slugs: `a-publiclinks-crate`, `b-sharing-link-create`, `c-mount-and-create-only`, `d-auth-layer`, `e-ocs-and-viewer`, `f-public-webdav`, `g-smoke-and-polish`.
- **Commit cadence:** Commit at every "Commit" step. Frequent, focused commits.
- **Pre-PR check:** Before opening the PR, run:
  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```
  All three must pass locally.
- **Open the PR:**
  ```bash
  git push -u origin sp8/<batch-letter>-<slug>
  gh pr create --title "sp8: batch <X> — <topic>" --body "$(cat <<'EOF'
  ## Summary
  - <one-line bullets>

  ## Test plan
  - [ ] CI green (fmt-and-clippy, build-wasm, test-sqlite, test-multidialect, e2e)
  - [ ] <batch-specific manual checks>

  🤖 Generated with [Claude Code](https://claude.com/claude-code)
  EOF
  )"
  ```
- **Merge:** After all checks pass: `gh pr merge --squash --delete-branch`.
- **Established workaround:** Tests that build `AppState` must set `cfg.filecache.enabled = false` before `AppStateBuilder::new(cfg).build()`. See `crates/crabcloud-http/tests/dav_basic.rs:16-37`.
- **Pre-existing patterns to mirror:**
  - **Service crate shape:** `crates/crabcloud-sharing` and `crates/crabcloud-users` (split `lib.rs`, `error.rs`, `types.rs`, `service.rs`, `sql.rs`).
  - **Argon2id usage:** `crates/crabcloud-users/src/app_passwords.rs` already uses `argon2 = "0.5"`. Mirror the params + helpers.
  - **OCS handler shape:** `crates/crabcloud-http/src/routes/ocs/files_sharing.rs`.
  - **dx SSR route:** `crates/crabcloud-app/src/pages/files/mod.rs` (route table) + `crates/crabcloud-app/src/pages/files/ssr.rs` (server-side data loading).
  - **dav adapter:** `crates/crabcloud-http/src/routes/dav.rs` (PROPFIND / GET / PUT delegation through View).
  - **Per-dialect integration test harness:** `crates/crabcloud-sharing/tests/sharing_e2e.rs` (uses `testcontainers` + `crabcloud-config::test_support`).

---

## File-by-file map

### New crate: `crabcloud-publiclinks`

```
crates/crabcloud-publiclinks/
├── Cargo.toml
├── src/
│   ├── lib.rs              — re-exports
│   ├── error.rs            — PublicLinkError
│   ├── tokens.rs           — Tokens (generate + lookup) — delegates to Shares for DB
│   ├── passwords.rs        — Passwords::hash / verify (Argon2id)
│   ├── cookie.rs           — UnlockCookie::sign / verify (HMAC-SHA256)
│   ├── ratelimit.rs        — RateLimiter (per-token, per-IP windowed counters)
│   ├── context.rs          — PublicLinkAuthContext (request extension)
│   └── auth_layer.rs       — PublicLinkAuthLayer axum middleware
└── tests/
    └── publiclinks_e2e.rs   — unit + small integration (no testcontainers)
```

### Modified

- `crates/crabcloud-sharing/src/types.rs` — extend `CreateShareRequest` with optional `password`, `expire_date` fields for link rows; extend `UpdateShareFields` to allow `password: Option<Option<String>>` (clear vs set).
- `crates/crabcloud-sharing/src/service.rs` — `Shares::create` and `Shares::update` learn `share_type=3`; new `Shares::resolve_by_token(&str) -> Result<Option<ShareRow>>` lookup.
- `crates/crabcloud-sharing/src/sql.rs` — new `SELECT_BY_TOKEN_QM` / `SELECT_BY_TOKEN_PG`, `UPDATE_PASSWORD_QM` / `_PG`.
- `crates/crabcloud-sharing/tests/sharing_e2e.rs` — link-create / update / resolve tests (multidialect).
- `crates/crabcloud-fs/src/storage/share_subroot.rs` — create-only links forbid `list` / `stat` on non-root children.
- `crates/crabcloud-fs/src/mount.rs` (or new `public_link_mount.rs` if cleaner) — `PublicLinkMountResolver`.
- `crates/crabcloud-config/src/lib.rs` — `public_link_secret: Option<String>` config field; auto-generate on first start.
- `crates/crabcloud-core/src/state.rs` — `AppState` carries `publiclinks: Arc<PublicLinks>` (the new crate's facade).
- `crates/crabcloud-http/src/routes/ocs/files_sharing.rs` — create / update handlers accept `shareType=3` request shape; serialize link fields (token, url, password set) in `share_to_json`.
- `crates/crabcloud-http/src/routes/mod.rs` — register `/s/{token}` and `/public.php/dav/files/{token}` subrouters.
- `crates/crabcloud-http/src/routes/public_link/mod.rs` (new) — `/s/{token}` HTTP handlers (viewer, unlock, download, upload, zip).
- `crates/crabcloud-http/src/routes/public_dav.rs` (new) — `/public.php/dav/files/{token}/*path` handlers.
- `crates/crabcloud-app/src/pages/public_link.rs` (new) — dx SSR viewer page.
- `crates/crabcloud-app/src/app.rs` — register `/s/:token` and `/s/:token/*path` routes in the dx Router.
- `crates/crabcloud-app/tests/server_fns_public_link.rs` (new) — viewer SSR tests.
- `crates/crabcloud-http/tests/public_link_e2e.rs` (new) — full surface e2e tests.
- `crates/crabcloud-http/tests/public_dav_e2e.rs` (new) — public WebDAV e2e tests.
- `crates/crabcloud-app/src/bin/smoke_public_link.rs` (new) — dx smoke binary.

---

# Batch A — `crabcloud-publiclinks` foundation crate

**Branch:** `sp8/a-publiclinks-crate`

**Goal:** Stand up the new crate with token generation, Argon2id password hashing, HMAC unlock cookies, and an in-memory rate limiter. No external surface; everything lives behind unit tests.

### Task A1: Create the crate skeleton

**Files:**
- Create: `crates/crabcloud-publiclinks/Cargo.toml`
- Create: `crates/crabcloud-publiclinks/src/lib.rs`
- Create: `crates/crabcloud-publiclinks/src/error.rs`
- Modify: `Cargo.toml` (workspace) — add to members list

- [ ] **Step 1: Add the crate to the workspace**

Edit the workspace `Cargo.toml` at the repo root. Add `"crates/crabcloud-publiclinks"` to the `members` array (keep the alphabetical order if the file is sorted; if not, add at the end of the list).

- [ ] **Step 2: Write `crates/crabcloud-publiclinks/Cargo.toml`**

```toml
[package]
name = "crabcloud-publiclinks"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
argon2 = { workspace = true }
base64 = { workspace = true }
chrono = { workspace = true, features = ["serde"] }
dashmap = { workspace = true }
hmac = { workspace = true }
rand = { workspace = true }
sha2 = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true, features = ["sync"] }
tracing = { workspace = true }
zeroize = { workspace = true }

[dev-dependencies]
tokio = { workspace = true, features = ["macros", "rt-multi-thread", "test-util"] }
```

If any of these aren't already in the workspace's `[workspace.dependencies]`, add them there with the version `app_passwords.rs` and existing crates use. `argon2 = "0.5"`, `base64 = "0.22"`, `dashmap = "6"`, `hmac = "0.12"`, `sha2 = "0.10"`, `zeroize = "1"`. Check `Cargo.lock` to find existing pins if unsure.

- [ ] **Step 3: Write `src/lib.rs`**

```rust
//! Public link infrastructure: tokens, passwords, unlock cookies, rate limiting.
//!
//! Spec: `docs/superpowers/specs/2026-05-15-public-links-design.md`.
//!
//! This crate is intentionally db-agnostic and storage-agnostic. The DB lookup
//! for tokens is delegated back to `crabcloud-sharing::Shares::resolve_by_token`
//! (passed in via a small trait), keeping the dependency arrows clean.

mod cookie;
mod error;
mod passwords;
mod ratelimit;
mod tokens;

pub use cookie::UnlockCookie;
pub use error::PublicLinkError;
pub use passwords::{HashedPassword, Passwords};
pub use ratelimit::{RateLimitDecision, RateLimiter};
pub use tokens::{Token, Tokens};
```

- [ ] **Step 4: Write `src/error.rs`**

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PublicLinkError {
    #[error("invalid argon2 hash format")]
    InvalidHash,
    #[error("invalid cookie value")]
    InvalidCookie,
    #[error("token generation failed after retries")]
    TokenGenerationFailed,
    #[error(transparent)]
    Argon2(#[from] argon2::password_hash::Error),
}
```

- [ ] **Step 5: Verify crate builds**

```bash
cargo build -p crabcloud-publiclinks
```

Expected: clean build, possibly warnings about unused modules — those clear up as later tasks land.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/crabcloud-publiclinks/
git commit -m "publiclinks: crate skeleton with error type"
```

### Task A2: Token generator

**Files:**
- Create: `crates/crabcloud-publiclinks/src/tokens.rs`

- [ ] **Step 1: Write the failing tests**

Create `src/tokens.rs` with the test module at the bottom:

```rust
//! Public-link tokens: 15-char `[A-Za-z0-9]` strings (~89 bits entropy).
//!
//! Matches Nextcloud's token format byte-for-byte so existing desktop/mobile
//! clients accept the URLs without modification.

use rand::{rngs::OsRng, RngCore};
use std::fmt;

const ALPHABET: &[u8; 62] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
const TOKEN_LEN: usize = 15;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Token(String);

impl Token {
    pub fn generate() -> Self {
        let mut buf = [0u8; TOKEN_LEN];
        let mut entropy = [0u8; TOKEN_LEN];
        OsRng.fill_bytes(&mut entropy);
        for (i, b) in entropy.iter().enumerate() {
            buf[i] = ALPHABET[(*b as usize) % ALPHABET.len()];
        }
        // SAFETY: every byte chosen from `ALPHABET`, which is ASCII.
        Token(String::from_utf8(buf.to_vec()).expect("alphabet is ASCII"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Validate the *shape* of an incoming token string. Returns `None` if the
    /// string isn't a plausible token; this short-circuits DB lookups for
    /// random garbage path segments.
    pub fn parse(s: &str) -> Option<Self> {
        if s.len() != TOKEN_LEN {
            return None;
        }
        if !s.bytes().all(|b| ALPHABET.contains(&b)) {
            return None;
        }
        Some(Token(s.to_string()))
    }
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Facade used by callers. `Shares` owns the actual DB lookup; this trait
/// keeps `crabcloud-publiclinks` independent of sharing internals.
pub struct Tokens;

impl Tokens {
    pub fn new() -> Self {
        Self
    }
    pub fn generate(&self) -> Token {
        Token::generate()
    }
}

impl Default for Tokens {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn generated_token_has_correct_length_and_charset() {
        let t = Token::generate();
        assert_eq!(t.as_str().len(), TOKEN_LEN);
        for b in t.as_str().bytes() {
            assert!(ALPHABET.contains(&b), "byte {b} not in alphabet");
        }
    }

    #[test]
    fn ten_thousand_tokens_are_unique() {
        let mut seen = HashSet::new();
        for _ in 0..10_000 {
            assert!(seen.insert(Token::generate().0));
        }
    }

    #[test]
    fn parse_accepts_well_formed() {
        let t = Token::generate();
        assert_eq!(Token::parse(t.as_str()).unwrap(), t);
    }

    #[test]
    fn parse_rejects_wrong_length() {
        assert!(Token::parse("short").is_none());
        assert!(Token::parse("waytoolongforthistokentokentokentoken").is_none());
    }

    #[test]
    fn parse_rejects_invalid_chars() {
        // 15 chars but contains `_`
        assert!(Token::parse("ABC_DEFGHIJKLMN").is_none());
        // 15 chars but contains `+`
        assert!(Token::parse("ABC+DEFGHIJKLMN").is_none());
    }
}
```

- [ ] **Step 2: Run tests, watch them pass**

```bash
cargo test -p crabcloud-publiclinks tokens::tests --no-fail-fast
```

Expected: 5 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/crabcloud-publiclinks/src/tokens.rs
git commit -m "publiclinks: 15-char URL-safe token generator"
```

### Task A3: Password hashing

**Files:**
- Create: `crates/crabcloud-publiclinks/src/passwords.rs`

- [ ] **Step 1: Mirror app_passwords' Argon2 usage**

Read `crates/crabcloud-users/src/app_passwords.rs` first to find the existing Argon2id helper. We mirror the same parameters (memory cost, parallelism, salt format) so operational characteristics match.

- [ ] **Step 2: Write `src/passwords.rs`**

```rust
//! Argon2id hashing for public-link passwords. Mirrors the parameters used
//! by `crabcloud-users::app_passwords` so operational cost is consistent.

use crate::error::PublicLinkError;
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use zeroize::Zeroizing;

/// A stored Argon2id hash. The `String` is the PHC-format hash (`$argon2id$...`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashedPassword(String);

impl HashedPassword {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn from_stored(s: String) -> Self {
        Self(s)
    }
}

pub struct Passwords;

impl Passwords {
    pub fn new() -> Self {
        Self
    }

    pub fn hash(&self, plaintext: &str) -> Result<HashedPassword, PublicLinkError> {
        let salt = SaltString::generate(&mut OsRng);
        let argon = Argon2::default();
        let hash = argon.hash_password(plaintext.as_bytes(), &salt)?;
        Ok(HashedPassword(hash.to_string()))
    }

    /// Constant-time verification. Returns `false` for invalid hash format
    /// rather than erroring, so a malformed stored hash doesn't leak via the
    /// shape of the error response on the wire.
    pub fn verify(&self, plaintext: &str, hashed: &HashedPassword) -> bool {
        let parsed = match PasswordHash::new(hashed.as_str()) {
            Ok(p) => p,
            Err(_) => return false,
        };
        // Zeroize the plaintext after verification — we keep a copy on the
        // stack only briefly, but argon2's verify takes &[u8] so we don't
        // hand it our owned copy directly.
        let _z = Zeroizing::new(plaintext.to_string());
        Argon2::default()
            .verify_password(plaintext.as_bytes(), &parsed)
            .is_ok()
    }
}

impl Default for Passwords {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_matches() {
        let p = Passwords::new();
        let h = p.hash("hunter2").unwrap();
        assert!(p.verify("hunter2", &h));
    }

    #[test]
    fn wrong_password_rejected() {
        let p = Passwords::new();
        let h = p.hash("hunter2").unwrap();
        assert!(!p.verify("hunter3", &h));
    }

    #[test]
    fn empty_password_round_trips() {
        let p = Passwords::new();
        let h = p.hash("").unwrap();
        assert!(p.verify("", &h));
        assert!(!p.verify("anything", &h));
    }

    #[test]
    fn malformed_hash_yields_false() {
        let p = Passwords::new();
        let bad = HashedPassword::from_stored("not-a-real-hash".into());
        assert!(!p.verify("anything", &bad));
    }

    #[test]
    fn distinct_hashes_for_same_password() {
        // Salts differ, so the stored strings should not collide.
        let p = Passwords::new();
        let h1 = p.hash("same").unwrap();
        let h2 = p.hash("same").unwrap();
        assert_ne!(h1, h2);
        assert!(p.verify("same", &h1));
        assert!(p.verify("same", &h2));
    }
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p crabcloud-publiclinks passwords::tests --no-fail-fast
```

Expected: 5 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/crabcloud-publiclinks/src/passwords.rs
git commit -m "publiclinks: Argon2id password hash + verify"
```

### Task A4: Unlock cookie HMAC

**Files:**
- Create: `crates/crabcloud-publiclinks/src/cookie.rs`

- [ ] **Step 1: Write `src/cookie.rs`**

```rust
//! Unlock cookie format: base64url(`exp_unix_le_8 || hmac_sha256(secret, token || exp_unix_le_8)`).
//!
//! Cookie name is `pl_<token>`. Scope: `Path=/`, `HttpOnly`, `Secure` (prod),
//! `SameSite=Lax`, `Max-Age=3600`. The MAC binds the cookie to *both* the
//! token (so cookies don't cross-link) and the expiry (so a captured cookie
//! can't be replayed forever).

use crate::error::PublicLinkError;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub struct UnlockCookie;

impl UnlockCookie {
    pub const COOKIE_NAME_PREFIX: &'static str = "pl_";

    pub fn cookie_name_for(token: &str) -> String {
        format!("{}{}", Self::COOKIE_NAME_PREFIX, token)
    }

    /// Build the cookie value for `token` valid until `exp_unix`.
    pub fn sign(secret: &[u8], token: &str, exp_unix: i64) -> String {
        let mut payload = Vec::with_capacity(8 + token.len());
        payload.extend_from_slice(&exp_unix.to_le_bytes());
        let mac = {
            let mut mac =
                HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
            mac.update(token.as_bytes());
            mac.update(&exp_unix.to_le_bytes());
            mac.finalize().into_bytes()
        };
        payload.extend_from_slice(&mac);
        URL_SAFE_NO_PAD.encode(payload)
    }

    /// Returns `Ok(exp_unix)` on a valid, unexpired-at-now cookie.
    pub fn verify(
        secret: &[u8],
        token: &str,
        cookie_value: &str,
        now_unix: i64,
    ) -> Result<i64, PublicLinkError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(cookie_value)
            .map_err(|_| PublicLinkError::InvalidCookie)?;
        if bytes.len() != 8 + 32 {
            return Err(PublicLinkError::InvalidCookie);
        }
        let mut exp_bytes = [0u8; 8];
        exp_bytes.copy_from_slice(&bytes[..8]);
        let exp_unix = i64::from_le_bytes(exp_bytes);
        if exp_unix < now_unix {
            return Err(PublicLinkError::InvalidCookie);
        }
        let mut mac =
            HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
        mac.update(token.as_bytes());
        mac.update(&exp_bytes);
        mac.verify_slice(&bytes[8..])
            .map_err(|_| PublicLinkError::InvalidCookie)?;
        Ok(exp_unix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"32-byte-secret-for-tests--------";

    #[test]
    fn round_trip_verifies() {
        let v = UnlockCookie::sign(SECRET, "ABCDEFGHIJKLMNO", 2_000_000_000);
        assert_eq!(
            UnlockCookie::verify(SECRET, "ABCDEFGHIJKLMNO", &v, 1_000_000_000).unwrap(),
            2_000_000_000
        );
    }

    #[test]
    fn expired_cookie_rejected() {
        let v = UnlockCookie::sign(SECRET, "ABCDEFGHIJKLMNO", 1_000_000_000);
        let err = UnlockCookie::verify(SECRET, "ABCDEFGHIJKLMNO", &v, 1_000_000_001);
        assert!(matches!(err, Err(PublicLinkError::InvalidCookie)));
    }

    #[test]
    fn tampered_mac_rejected() {
        let mut v = UnlockCookie::sign(SECRET, "ABCDEFGHIJKLMNO", 2_000_000_000);
        // Flip a bit in the MAC region.
        let last = v.pop().unwrap();
        let flipped = if last == 'A' { 'B' } else { 'A' };
        v.push(flipped);
        let err = UnlockCookie::verify(SECRET, "ABCDEFGHIJKLMNO", &v, 1_000_000_000);
        assert!(matches!(err, Err(PublicLinkError::InvalidCookie)));
    }

    #[test]
    fn wrong_token_rejected() {
        let v = UnlockCookie::sign(SECRET, "ABCDEFGHIJKLMNO", 2_000_000_000);
        let err = UnlockCookie::verify(SECRET, "ZZZZZZZZZZZZZZZ", &v, 1_000_000_000);
        assert!(matches!(err, Err(PublicLinkError::InvalidCookie)));
    }

    #[test]
    fn wrong_secret_rejected() {
        let v = UnlockCookie::sign(SECRET, "ABCDEFGHIJKLMNO", 2_000_000_000);
        let err = UnlockCookie::verify(
            b"different-secret-for-tests------",
            "ABCDEFGHIJKLMNO",
            &v,
            1_000_000_000,
        );
        assert!(matches!(err, Err(PublicLinkError::InvalidCookie)));
    }

    #[test]
    fn garbage_input_rejected() {
        assert!(matches!(
            UnlockCookie::verify(SECRET, "ABCDEFGHIJKLMNO", "not-base64@@", 1_000_000_000),
            Err(PublicLinkError::InvalidCookie)
        ));
        assert!(matches!(
            UnlockCookie::verify(SECRET, "ABCDEFGHIJKLMNO", "AAAA", 1_000_000_000),
            Err(PublicLinkError::InvalidCookie)
        ));
    }

    #[test]
    fn cookie_name_includes_token() {
        assert_eq!(UnlockCookie::cookie_name_for("ABC"), "pl_ABC");
    }
}
```

- [ ] **Step 2: Run tests**

```bash
cargo test -p crabcloud-publiclinks cookie::tests --no-fail-fast
```

Expected: 7 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/crabcloud-publiclinks/src/cookie.rs
git commit -m "publiclinks: HMAC-SHA256 unlock cookie sign+verify"
```

### Task A5: Rate limiter

**Files:**
- Create: `crates/crabcloud-publiclinks/src/ratelimit.rs`

- [ ] **Step 1: Write `src/ratelimit.rs`**

```rust
//! In-memory windowed rate limiting for public links. Two flavors:
//! - Per-token password-unlock attempts: 10 per hour per token.
//! - Per-IP file-drop uploads: 60 per hour per IP.
//!
//! MVP single-node scope; state vanishes on process restart. Documented
//! limitation — SP-later can swap for durable counters if multi-node.

use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const PASSWORD_ATTEMPTS_PER_HOUR: u32 = 10;
pub const UPLOAD_ATTEMPTS_PER_HOUR: u32 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitDecision {
    Allowed,
    Throttled { retry_after_secs: u64 },
}

#[derive(Debug, Clone, Copy)]
struct AttemptLog {
    window_start: Instant,
    count: u32,
}

#[derive(Clone)]
pub struct RateLimiter {
    inner: Arc<RateLimiterInner>,
}

struct RateLimiterInner {
    password_attempts: DashMap<String, AttemptLog>,
    upload_attempts: DashMap<String, AttemptLog>,
    window: Duration,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::with_window(Duration::from_secs(3600))
    }

    pub fn with_window(window: Duration) -> Self {
        Self {
            inner: Arc::new(RateLimiterInner {
                password_attempts: DashMap::new(),
                upload_attempts: DashMap::new(),
                window,
            }),
        }
    }

    pub fn check_password_attempt(&self, token: &str) -> RateLimitDecision {
        self.check(&self.inner.password_attempts, token, PASSWORD_ATTEMPTS_PER_HOUR)
    }

    pub fn check_upload(&self, ip: &str) -> RateLimitDecision {
        self.check(&self.inner.upload_attempts, ip, UPLOAD_ATTEMPTS_PER_HOUR)
    }

    fn check(
        &self,
        bucket: &DashMap<String, AttemptLog>,
        key: &str,
        cap: u32,
    ) -> RateLimitDecision {
        let now = Instant::now();
        let window = self.inner.window;
        let mut entry = bucket
            .entry(key.to_string())
            .or_insert(AttemptLog { window_start: now, count: 0 });
        if now.duration_since(entry.window_start) >= window {
            entry.window_start = now;
            entry.count = 0;
        }
        if entry.count >= cap {
            let elapsed = now.duration_since(entry.window_start);
            let retry_after_secs = window.saturating_sub(elapsed).as_secs().max(1);
            return RateLimitDecision::Throttled { retry_after_secs };
        }
        entry.count += 1;
        RateLimitDecision::Allowed
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_ten_password_attempts_allowed_eleventh_throttled() {
        let rl = RateLimiter::new();
        for _ in 0..PASSWORD_ATTEMPTS_PER_HOUR {
            assert!(matches!(rl.check_password_attempt("tok"), RateLimitDecision::Allowed));
        }
        assert!(matches!(
            rl.check_password_attempt("tok"),
            RateLimitDecision::Throttled { .. }
        ));
    }

    #[test]
    fn upload_cap_higher_than_password_cap() {
        let rl = RateLimiter::new();
        for _ in 0..UPLOAD_ATTEMPTS_PER_HOUR {
            assert!(matches!(rl.check_upload("1.2.3.4"), RateLimitDecision::Allowed));
        }
        assert!(matches!(
            rl.check_upload("1.2.3.4"),
            RateLimitDecision::Throttled { .. }
        ));
    }

    #[test]
    fn distinct_keys_have_independent_counters() {
        let rl = RateLimiter::new();
        for _ in 0..PASSWORD_ATTEMPTS_PER_HOUR {
            assert!(matches!(rl.check_password_attempt("a"), RateLimitDecision::Allowed));
        }
        assert!(matches!(
            rl.check_password_attempt("b"),
            RateLimitDecision::Allowed
        ));
    }

    #[test]
    fn window_resets_with_short_window() {
        let rl = RateLimiter::with_window(Duration::from_millis(50));
        for _ in 0..PASSWORD_ATTEMPTS_PER_HOUR {
            rl.check_password_attempt("t");
        }
        assert!(matches!(
            rl.check_password_attempt("t"),
            RateLimitDecision::Throttled { .. }
        ));
        std::thread::sleep(Duration::from_millis(80));
        assert!(matches!(rl.check_password_attempt("t"), RateLimitDecision::Allowed));
    }
}
```

- [ ] **Step 2: Run tests**

```bash
cargo test -p crabcloud-publiclinks ratelimit::tests --no-fail-fast
```

Expected: 4 tests pass. The `window_resets_with_short_window` test sleeps 80ms; allow up to 1s for CI variance.

- [ ] **Step 3: Commit**

```bash
git add crates/crabcloud-publiclinks/src/ratelimit.rs
git commit -m "publiclinks: in-memory windowed rate limiter"
```

### Task A6: Pre-PR sweep + PR

- [ ] **Step 1: Sweep**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Fix any warnings. The new crate has no consumers yet — `unused_crate_dependencies` should stay quiet because every dep is referenced.

- [ ] **Step 2: Push + open PR**

```bash
git push -u origin sp8/a-publiclinks-crate
gh pr create --title "sp8(a): publiclinks crate skeleton + tokens, passwords, cookies, rate-limit" --body "$(cat <<'EOF'
## Summary
- New `crabcloud-publiclinks` crate scaffolding.
- Token generator (15-char `[A-Za-z0-9]`, ~89-bit entropy).
- Argon2id password hash + verify mirroring `app_passwords` parameters.
- HMAC-SHA256 unlock cookie sign+verify with token binding.
- In-memory windowed rate limiter (per-token password, per-IP upload).

## Test plan
- [ ] CI green (fmt-and-clippy, build-wasm, test-sqlite, test-multidialect, e2e)
- [ ] `cargo test -p crabcloud-publiclinks` passes locally

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 3: Merge after green** (gh pr merge --squash --delete-branch).

---

# Batch B — Sharing service link support

**Branch:** `sp8/b-sharing-link-create`

**Goal:** Lift the `NotImplemented` gate in `Shares::create` for `share_type=3`. Generate token, hash password, persist link rows. Extend `Shares::update` for `password` and `expiration` fields. Add `Shares::resolve_by_token` lookup.

### Task B1: Extend request types for link fields

**Files:**
- Modify: `crates/crabcloud-sharing/src/types.rs`
- Modify: `crates/crabcloud-sharing/Cargo.toml` — add `crabcloud-publiclinks` dep

- [ ] **Step 1: Add publiclinks dep**

In `crates/crabcloud-sharing/Cargo.toml`, add to `[dependencies]`:

```toml
crabcloud-publiclinks = { path = "../crabcloud-publiclinks" }
```

- [ ] **Step 2: Extend `CreateShareRequest`**

In `crates/crabcloud-sharing/src/types.rs`, add fields to the struct:

```rust
#[derive(Debug, Clone)]
pub struct CreateShareRequest {
    pub requester: String,
    pub path: String,
    pub share_type: ShareType,
    /// For `Link` shares this is empty / unused. Service ignores it for link rows.
    pub share_with: String,
    pub permissions: u32,
    pub home_storage_id: String,
    /// Link-only. `None` = no password.
    pub password: Option<String>,
    /// Link-only. `None` = no expiration.
    pub expire_date: Option<chrono::NaiveDate>,
}
```

Update the existing tests / call sites to add `password: None, expire_date: None,` to all existing struct literals (search the workspace for `CreateShareRequest {` and add the new fields).

- [ ] **Step 3: Extend `UpdateShareFields`**

Already has `password: Option<Option<String>>` per current code (see service.rs line ~390, currently rejected with `NotImplemented`). Leave the field shape as-is, but the type is already present.

Verify by looking at types.rs `UpdateShareFields` definition. The field is `password: Option<Option<String>>` (outer = "did the caller mention it?", inner = "what value? None means clear").

- [ ] **Step 4: Build to confirm types align**

```bash
cargo build -p crabcloud-sharing -p crabcloud-http
```

Fix any build errors in callers that construct `CreateShareRequest`.

- [ ] **Step 5: Commit**

```bash
git add crates/crabcloud-sharing/
git commit -m "sharing: extend CreateShareRequest with link-only password+expire_date"
```

### Task B2: Add SELECT_BY_TOKEN, UPDATE_PASSWORD SQL constants

**Files:**
- Modify: `crates/crabcloud-sharing/src/sql.rs`

- [ ] **Step 1: Add constants**

Add to `crates/crabcloud-sharing/src/sql.rs` after the existing `UPDATE_EXPIRATION_PG` constants:

```rust
pub(crate) const SELECT_BY_TOKEN_QM: &str = "SELECT id, share_type, share_with, uid_owner, \
    uid_initiator, parent, item_type, item_source, file_source, file_target, permissions, \
    stime, accepted, expiration, token, password, mail_send \
    FROM oc_share WHERE token = ? AND share_type = 3";

pub(crate) const SELECT_BY_TOKEN_PG: &str = "SELECT id, share_type, share_with, uid_owner, \
    uid_initiator, parent, item_type, item_source, file_source, file_target, permissions, \
    stime, accepted, expiration, token, password, mail_send \
    FROM oc_share WHERE token = $1 AND share_type = 3";

pub(crate) const UPDATE_PASSWORD_QM: &str = "UPDATE oc_share SET password = ? WHERE id = ?";
pub(crate) const UPDATE_PASSWORD_PG: &str = "UPDATE oc_share SET password = $1 WHERE id = $2";
```

- [ ] **Step 2: Verify constants reachable**

```bash
cargo build -p crabcloud-sharing
```

Expected: warnings about unused constants — those clear once B3/B4 reference them.

- [ ] **Step 3: Commit**

```bash
git add crates/crabcloud-sharing/src/sql.rs
git commit -m "sharing(sql): SELECT_BY_TOKEN and UPDATE_PASSWORD constants"
```

### Task B3: `Shares::create` accepts `share_type=3`

**Files:**
- Modify: `crates/crabcloud-sharing/src/service.rs`

- [ ] **Step 1: Write the failing test first**

Add to `crates/crabcloud-sharing/tests/sharing_e2e.rs` (top of the file, find an existing `#[tokio::test]` and add a parallel one). Find the helper that builds a `Shares` against a freshly-migrated DB; reuse it. The test:

```rust
#[tokio::test]
async fn link_share_create_persists_token_and_password() {
    for harness in test_support::dialects() {
        let h = harness.fresh().await;
        // owner alice exists, has /Photos in her home storage.
        let req = CreateShareRequest {
            requester: "alice".into(),
            path: "/Photos".into(),
            share_type: ShareType::Link,
            share_with: String::new(),
            permissions: 1, // read
            home_storage_id: h.alice_storage_id.clone(),
            password: Some("hunter2".into()),
            expire_date: None,
        };
        let row = h.shares.create(req).await.expect("link share creates");
        assert_eq!(row.share_type, ShareType::Link);
        assert!(row.token.as_deref().map(str::len) == Some(15));
        assert!(row.password_hash.as_deref().unwrap().starts_with("$argon2id$"));
    }
}
```

You may need to look at `crates/crabcloud-sharing/tests/sharing_e2e.rs` to find the existing harness conventions; mirror them. If `test_support::dialects()` doesn't exist, follow the pattern of running once per dialect with `#[tokio::test(flavor = "multi_thread")]` and a manual `for` over `Dialect`.

- [ ] **Step 2: Run test, watch it fail**

```bash
cargo test -p crabcloud-sharing link_share_create_persists_token_and_password
```

Expected: FAIL with `NotImplemented`.

- [ ] **Step 3: Implement**

In `crates/crabcloud-sharing/src/service.rs`, replace the early `NotImplemented` guard in `create`:

```rust
        if matches!(req.share_type, ShareType::Link) {
            return Err(ShareError::NotImplemented);
        }
```

with:

```rust
        // Link rows take a different code path: no share_with target, password
        // and expiration handled, token generated.
        if matches!(req.share_type, ShareType::Link) {
            return self.create_link(req).await;
        }
```

Then add the new method at the end of `impl Shares`:

```rust
    async fn create_link(&self, req: CreateShareRequest) -> Result<ShareRow, ShareError> {
        if req.permissions & 1 == 0 && req.permissions & 4 == 0 {
            return Err(ShareError::BadPermissions);
        }
        let perms = SharePermissions::from_wire(req.permissions);

        let storage_path = parse_wire_path(&req.path)?;
        let row = self
            .filecache
            .lookup(&req.home_storage_id, &storage_path)
            .await
            .map_err(map_filecache)?
            .ok_or(ShareError::PathNotOwned)?;
        if row.storage_id != req.home_storage_id {
            return Err(ShareError::ReshareRejected);
        }

        let item_type = if row.mimetype.as_str() == crabcloud_filecache::DIRECTORY_MIMETYPE {
            ItemType::Folder
        } else {
            ItemType::File
        };
        // Link rows store the FULL owner path in `file_target` (unlike user/group
        // shares which store only the basename). The auth layer reads this back
        // via `Shares::resolve_by_token` and uses it as the SharedSubrootStorage
        // root, so it must be unambiguous.
        let file_target = format!("/{}", storage_path.as_str());
        let stime = unix_now();
        let share_type_db: i16 = req.share_type.into();
        let perms_db = perms.as_u32() as i32;
        let fileid = row.fileid;

        // Hash the password if one was supplied.
        let password_hash: Option<String> = match req.password.as_deref() {
            Some(pw) => {
                let h = crabcloud_publiclinks::Passwords::new()
                    .hash(pw)
                    .map_err(|_| ShareError::DbError(sqlx::Error::Protocol(
                        "password hash failed".into(),
                    )))?;
                Some(h.as_str().to_string())
            }
            None => None,
        };

        let expiration: Option<NaiveDateTime> = req
            .expire_date
            .and_then(|d| d.and_hms_opt(0, 0, 0));

        // Token generation with retry on UNIQUE collision.
        let token = crabcloud_publiclinks::Tokens::new().generate();
        let id = self.insert_link_row(
            share_type_db, &req.requester, fileid, &file_target, perms_db, stime,
            item_type, expiration.as_ref(), &token.to_string(),
            password_hash.as_deref(),
        ).await?;

        Ok(ShareRow {
            id,
            share_type: ShareType::Link,
            share_with: None,
            uid_owner: req.requester.clone(),
            uid_initiator: req.requester,
            parent: None,
            item_type,
            item_source: fileid,
            file_source: fileid,
            file_target,
            permissions: perms,
            stime,
            accepted: true,
            expiration: expiration.map(|n| DateTime::<Utc>::from_naive_utc_and_offset(n, Utc)),
            token: Some(token.to_string()),
            password_hash,
        })
    }

    async fn insert_link_row(
        &self,
        share_type_db: i16,
        requester: &str,
        fileid: i64,
        file_target: &str,
        perms_db: i32,
        stime: i64,
        item_type: ItemType,
        expiration: Option<&NaiveDateTime>,
        token: &str,
        password: Option<&str>,
    ) -> Result<i64, ShareError> {
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let res = sqlx::query(sql::INSERT_QM)
                    .bind(share_type_db)
                    .bind::<Option<&str>>(None)        // share_with
                    .bind(requester)                    // uid_owner
                    .bind(requester)                    // uid_initiator
                    .bind::<Option<i64>>(None)
                    .bind(item_type.as_db_str())
                    .bind(fileid)
                    .bind(fileid)
                    .bind(file_target)
                    .bind(perms_db)
                    .bind(stime)
                    .bind(1_i16)                        // accepted
                    .bind(expiration)
                    .bind(token)
                    .bind(password)
                    .bind(0_i16)                        // mail_send
                    .execute(p)
                    .await?;
                Ok(res.last_insert_rowid())
            }
            DbPool::MySql(p) => {
                let res = sqlx::query(sql::INSERT_QM)
                    .bind(share_type_db)
                    .bind::<Option<&str>>(None)
                    .bind(requester)
                    .bind(requester)
                    .bind::<Option<i64>>(None)
                    .bind(item_type.as_db_str())
                    .bind(fileid)
                    .bind(fileid)
                    .bind(file_target)
                    .bind(perms_db)
                    .bind(stime)
                    .bind(1_i16)
                    .bind(expiration)
                    .bind(token)
                    .bind(password)
                    .bind(0_i16)
                    .execute(p)
                    .await?;
                Ok(res.last_insert_id() as i64)
            }
            DbPool::Postgres(p) => {
                let row = sqlx::query(sql::INSERT_PG)
                    .bind(share_type_db)
                    .bind::<Option<&str>>(None)
                    .bind(requester)
                    .bind(requester)
                    .bind::<Option<i64>>(None)
                    .bind(item_type.as_db_str())
                    .bind(fileid)
                    .bind(fileid)
                    .bind(file_target)
                    .bind(perms_db)
                    .bind(stime)
                    .bind(1_i16)
                    .bind(expiration)
                    .bind(token)
                    .bind(password)
                    .bind(0_i16)
                    .fetch_one(p)
                    .await?;
                Ok(row.try_get::<i64, _>("id")?)
            }
        }
    }
```

Token-collision retry: leave for now. With 89 bits of entropy a collision is implausible during a single create; if it ever happens, `sqlx::Error::Database` surfaces a `UNIQUE` violation that the caller will receive as `500`. Acceptable for MVP.

- [ ] **Step 4: Run test, watch it pass**

```bash
cargo test -p crabcloud-sharing link_share_create_persists_token_and_password
```

Expected: PASS across all dialects.

- [ ] **Step 5: Commit**

```bash
git add crates/crabcloud-sharing/src/service.rs crates/crabcloud-sharing/tests/sharing_e2e.rs
git commit -m "sharing: implement Shares::create for share_type=3 (public link)"
```

### Task B4: `Shares::update` for link fields (password, expiration)

**Files:**
- Modify: `crates/crabcloud-sharing/src/service.rs`

- [ ] **Step 1: Write the failing test**

Append to `tests/sharing_e2e.rs`:

```rust
#[tokio::test]
async fn link_share_update_sets_password_and_expiration() {
    for harness in test_support::dialects() {
        let h = harness.fresh().await;
        let row = h.shares.create(CreateShareRequest {
            requester: "alice".into(),
            path: "/Photos".into(),
            share_type: ShareType::Link,
            share_with: String::new(),
            permissions: 1,
            home_storage_id: h.alice_storage_id.clone(),
            password: None,
            expire_date: None,
        }).await.unwrap();

        let updated = h.shares.update(row.id, &UserId::new("alice").unwrap(), UpdateShareFields {
            password: Some(Some("newpw".into())),
            expire_date: Some(Some(chrono::NaiveDate::from_ymd_opt(2030, 1, 1).unwrap())),
            ..Default::default()
        }).await.unwrap();

        assert!(updated.password_hash.is_some());
        assert!(updated.expiration.is_some());

        // Clear password.
        let cleared = h.shares.update(row.id, &UserId::new("alice").unwrap(), UpdateShareFields {
            password: Some(None),
            ..Default::default()
        }).await.unwrap();
        assert!(cleared.password_hash.is_none());
    }
}
```

- [ ] **Step 2: Run test, watch it fail**

Expected: FAIL — current `update` returns `NotImplemented` when `password.is_some()`.

- [ ] **Step 3: Implement**

In `Shares::update`, replace:

```rust
        if fields.password.is_some() || fields.note.is_some() {
            return Err(ShareError::NotImplemented);
        }
```

with:

```rust
        if fields.note.is_some() {
            return Err(ShareError::NotImplemented);
        }
        if let Some(pw_opt) = &fields.password {
            // Only Link rows accept password updates.
            if !matches!(existing.share_type, ShareType::Link) {
                return Err(ShareError::BadPermissions);
            }
            let hashed: Option<String> = match pw_opt {
                Some(pw) => Some(
                    crabcloud_publiclinks::Passwords::new()
                        .hash(pw)
                        .map_err(|_| {
                            ShareError::DbError(sqlx::Error::Protocol("password hash failed".into()))
                        })?
                        .as_str()
                        .to_string(),
                ),
                None => None,
            };
            run_update_password(&self.pool, id, hashed.as_deref()).await?;
        }
```

Add the helper next to `run_update_permissions`:

```rust
async fn run_update_password(
    pool: &DbPool,
    id: i64,
    value: Option<&str>,
) -> Result<(), ShareError> {
    match pool {
        DbPool::Sqlite(p) => {
            sqlx::query(sql::UPDATE_PASSWORD_QM)
                .bind(value)
                .bind(id)
                .execute(p)
                .await?;
        }
        DbPool::MySql(p) => {
            sqlx::query(sql::UPDATE_PASSWORD_QM)
                .bind(value)
                .bind(id)
                .execute(p)
                .await?;
        }
        DbPool::Postgres(p) => {
            sqlx::query(sql::UPDATE_PASSWORD_PG)
                .bind(value)
                .bind(id)
                .execute(p)
                .await?;
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Run test**

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git commit -am "sharing: Shares::update for link password + expiration"
```

### Task B5: `Shares::resolve_by_token`

**Files:**
- Modify: `crates/crabcloud-sharing/src/service.rs`

- [ ] **Step 1: Add method**

After `Shares::get`, add:

```rust
    /// Look up a share row by token. Returns `None` for unknown / non-link rows
    /// (the SQL is filtered to `share_type = 3`). Does NOT enforce expiration —
    /// the caller must compare `expiration` to `now()` and treat past-expired
    /// as missing.
    pub async fn resolve_by_token(&self, token: &str) -> Result<Option<ShareRow>, ShareError> {
        match self.pool.as_ref() {
            DbPool::Sqlite(p) => {
                let row = sqlx::query(sql::SELECT_BY_TOKEN_QM)
                    .bind(token)
                    .fetch_optional(p)
                    .await?;
                row.map(row_from_sqlite).transpose()
            }
            DbPool::MySql(p) => {
                let row = sqlx::query(sql::SELECT_BY_TOKEN_QM)
                    .bind(token)
                    .fetch_optional(p)
                    .await?;
                row.map(row_from_mysql).transpose()
            }
            DbPool::Postgres(p) => {
                let row = sqlx::query(sql::SELECT_BY_TOKEN_PG)
                    .bind(token)
                    .fetch_optional(p)
                    .await?;
                row.map(row_from_postgres).transpose()
            }
        }
    }
```

- [ ] **Step 2: Test**

Append to `tests/sharing_e2e.rs`:

```rust
#[tokio::test]
async fn resolve_by_token_returns_row() {
    for harness in test_support::dialects() {
        let h = harness.fresh().await;
        let row = h.shares.create(CreateShareRequest {
            requester: "alice".into(),
            path: "/Photos".into(),
            share_type: ShareType::Link,
            share_with: String::new(),
            permissions: 1,
            home_storage_id: h.alice_storage_id.clone(),
            password: None,
            expire_date: None,
        }).await.unwrap();
        let token = row.token.unwrap();
        let found = h.shares.resolve_by_token(&token).await.unwrap().expect("row");
        assert_eq!(found.id, row.id);
        assert!(h.shares.resolve_by_token("DOES_NOT_EXIST_").await.unwrap().is_none());
    }
}
```

- [ ] **Step 3: Run + commit**

```bash
cargo test -p crabcloud-sharing resolve_by_token_returns_row
git commit -am "sharing: Shares::resolve_by_token lookup"
```

### Task B6: Pre-PR sweep + PR

- [ ] **Step 1: Sweep + push + PR**

Standard pre-PR sweep, then:

```bash
git push -u origin sp8/b-sharing-link-create
gh pr create --title "sp8(b): sharing service learns share_type=3" --body ...
```

PR body:

```
## Summary
- `Shares::create` accepts `share_type=3` (lifts NotImplemented gate).
- `Shares::update` writes password + expiration for link rows.
- `Shares::resolve_by_token` adds the by-token lookup needed by the auth layer.
- Multidialect e2e coverage of all three.

## Test plan
- [ ] CI green (fmt-and-clippy, build-wasm, test-sqlite, test-multidialect)
- [ ] `cargo test -p crabcloud-sharing` passes across sqlite/mysql/postgres locally
```

---

# Batch C — Storage / mount integration

**Branch:** `sp8/c-mount-and-create-only`

**Goal:** Teach `SharedSubrootStorage` the create-only file-drop visibility rule, and add `PublicLinkMountResolver` that returns a single mount per request rooted at the linked subtree.

### Task C1: Create-only `SharedSubrootStorage` forbids list/stat children

**Files:**
- Modify: `crates/crabcloud-fs/src/storage/share_subroot.rs`

- [ ] **Step 1: Write failing tests**

Append to the existing test module in `share_subroot.rs`:

```rust
    #[tokio::test]
    async fn create_only_list_root_returns_empty() {
        // File-drop permission set: create-only (bit 4), no read.
        let s = wrap(seed_owner().await, 4);
        let entries = s.list(&StoragePath::root()).await.unwrap();
        assert!(entries.is_empty(),
            "create-only must hide directory listings; got {entries:?}");
    }

    #[tokio::test]
    async fn create_only_list_subdir_forbidden() {
        let s = wrap(seed_owner().await, 4);
        let r = s.list(&StoragePath::new("anywhere").unwrap()).await;
        assert!(matches!(r, Err(StorageError::PermissionDenied)));
    }

    #[tokio::test]
    async fn create_only_stat_child_forbidden() {
        let s = wrap(seed_owner().await, 4);
        let r = s.stat(&StoragePath::new("x.jpg").unwrap()).await;
        assert!(matches!(r, Err(StorageError::PermissionDenied)));
    }

    #[tokio::test]
    async fn create_only_stat_root_allowed() {
        let s = wrap(seed_owner().await, 4);
        // Root must still stat — viewer page needs to verify the linked
        // folder exists before rendering the upload UI.
        let m = s.stat(&StoragePath::root()).await.unwrap();
        assert!(matches!(m.kind, crabcloud_storage::FileKind::Dir));
    }

    #[tokio::test]
    async fn create_only_read_forbidden() {
        let s = wrap(seed_owner().await, 4);
        let r = s.read(&StoragePath::new("x.jpg").unwrap()).await;
        assert!(matches!(r, Err(StorageError::PermissionDenied)));
    }

    #[tokio::test]
    async fn create_only_put_new_allowed() {
        let s = wrap(seed_owner().await, 4);
        s.put_file(
            &StoragePath::new("upload.bin").unwrap(),
            body(b"data"),
            &NoopEventSink,
        ).await.unwrap();
    }

    #[tokio::test]
    async fn create_only_put_overwrite_forbidden() {
        // The existing file should not be overwritable by file-drop uploaders.
        let s = wrap(seed_owner().await, 4);
        let r = s.put_file(
            &StoragePath::new("x.jpg").unwrap(),
            body(b"overwrite"),
            &NoopEventSink,
        ).await;
        assert!(matches!(r, Err(StorageError::PermissionDenied)));
    }
```

- [ ] **Step 2: Run, watch them fail**

```bash
cargo test -p crabcloud-fs share_subroot::tests::create_only
```

Expected: the first three FAIL (currently reads are unconditional in SharedSubrootStorage).

- [ ] **Step 3: Implement**

In `share_subroot.rs`, modify `list`, `stat`, `read`, `read_range`, `exists` to enforce create-only:

```rust
    async fn stat(&self, path: &StoragePath) -> StorageResult<FileMetadata> {
        // Create-only file-drop: hide everything except the linked root itself.
        if self.is_create_only() && !path.is_root() {
            return Err(StorageError::PermissionDenied);
        }
        self.inner.stat(&self.translate(path)?).await
    }

    async fn exists(&self, path: &StoragePath) -> StorageResult<bool> {
        if self.is_create_only() && !path.is_root() {
            return Err(StorageError::PermissionDenied);
        }
        self.inner.exists(&self.translate(path)?).await
    }

    async fn list(&self, path: &StoragePath) -> StorageResult<Vec<DirEntry>> {
        if self.is_create_only() {
            if path.is_root() {
                // The viewer page needs a successful "list me an empty folder"
                // response so it can render the upload zone with no contents.
                return Ok(Vec::new());
            }
            return Err(StorageError::PermissionDenied);
        }
        self.inner.list(&self.translate(path)?).await
    }

    async fn read(&self, path: &StoragePath) -> StorageResult<Pin<Box<dyn AsyncRead + Send>>> {
        if self.is_create_only() {
            return Err(StorageError::PermissionDenied);
        }
        self.inner.read(&self.translate(path)?).await
    }

    async fn read_range(
        &self,
        path: &StoragePath,
        range: Range<u64>,
    ) -> StorageResult<Pin<Box<dyn AsyncRead + Send>>> {
        if self.is_create_only() {
            return Err(StorageError::PermissionDenied);
        }
        self.inner.read_range(&self.translate(path)?, range).await
    }
```

Add the `is_create_only` helper to `impl SharedSubrootStorage`:

```rust
    fn is_create_only(&self) -> bool {
        !self.permissions.contains_read() && self.permissions.allows_create()
    }
```

The `put_file` path also needs to special-case "create-only on existing path" → already enforced (it requires `allows_update()` when existing, which create-only lacks). The `create_only_put_overwrite_forbidden` test exercises this.

Also need to special-case `put_file` to not call `inner.exists(...)` first when `is_create_only` — because `exists` now returns `PermissionDenied` for non-root paths in create-only mode. Modify `put_file`:

```rust
    async fn put_file(
        &self,
        path: &StoragePath,
        body: Pin<Box<dyn AsyncRead + Send>>,
        sink: &dyn EventSink,
    ) -> StorageResult<FileMetadata> {
        let translated = self.translate(path)?;
        // We must check existence to decide between create/update perms, but
        // create-only callers never have update perms. Bypass the wrapper's
        // own `exists` (which we just rigged to deny) and ask the inner storage
        // directly — the wrapper's restriction is for *callers*, not internal
        // bookkeeping.
        let existing = self.inner.exists(&translated).await?;
        let allowed = if existing {
            self.permissions.allows_update()
        } else {
            self.permissions.allows_create()
        };
        if !allowed {
            return Err(StorageError::PermissionDenied);
        }
        self.inner.put_file(&translated, body, sink).await
    }
```

(Already calls `self.inner.exists` so this is fine; no change actually needed once the wrapper-level `exists` becomes restricted. Double-check the current code calls `self.inner.exists` not `self.exists`.)

Same for `begin_multipart`.

- [ ] **Step 4: Run all tests**

```bash
cargo test -p crabcloud-fs share_subroot::tests
```

Expected: all tests pass (the 11 existing + 7 new).

- [ ] **Step 5: Commit**

```bash
git commit -am "fs: SharedSubrootStorage hides children for create-only links"
```

### Task C2: `PublicLinkMountResolver`

**Files:**
- Create: `crates/crabcloud-fs/src/public_link_mount.rs`
- Modify: `crates/crabcloud-fs/src/lib.rs` — `pub mod public_link_mount; pub use public_link_mount::PublicLinkMountResolver;`

- [ ] **Step 1: Inspect existing `ShareMountResolver` for patterns**

```bash
cat crates/crabcloud-fs/src/share_mount.rs
```

(Or the closest equivalent in the source tree — find with `rg "ShareMountResolver" crates/crabcloud-fs`.) Mirror the trait impl shape (`MountResolver`, `resolve_for`).

- [ ] **Step 2: Write `public_link_mount.rs`**

```rust
//! Resolver for anonymous public-link requests. Returns exactly one mount
//! per request: a `SharedSubrootStorage` rooted at the linked subtree with
//! the link's permission bits applied. The owner is the linked share's
//! `uid_owner`; the home storage for that owner is fetched lazily.

use crate::storage::share_subroot::SharedSubrootStorage;
use crate::{Mount, MountMetadata, MountResolver, StorageFactory};
use async_trait::async_trait;
use crabcloud_sharing::SharePermissions;
use crabcloud_storage::{Storage, StoragePath};
use crabcloud_users::UserId;
use std::sync::Arc;

/// Wraps the existing `StorageFactory` so we can build a single-mount view
/// for a given public link.
pub struct PublicLinkMountResolver {
    factory: Arc<dyn StorageFactory>,
    owner: UserId,
    owner_path: StoragePath,
    permissions: SharePermissions,
}

impl PublicLinkMountResolver {
    pub fn new(
        factory: Arc<dyn StorageFactory>,
        owner: UserId,
        owner_path: StoragePath,
        permissions: SharePermissions,
    ) -> Self {
        Self { factory, owner, owner_path, permissions }
    }
}

#[async_trait]
impl MountResolver for PublicLinkMountResolver {
    async fn resolve_for(&self, _uid: &UserId) -> Vec<Mount> {
        // We ignore `_uid` — the resolver was constructed for a specific link
        // and returns the same single mount no matter who asks. The View layer
        // calls this with the virtual "owner_uid" anyway.
        let inner = match self.factory.home_storage(&self.owner).await {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let wrapped: Arc<dyn Storage> = Arc::new(SharedSubrootStorage::new(
            inner,
            self.owner_path.clone(),
            self.permissions,
        ));
        vec![Mount {
            path_prefix: StoragePath::root(),
            storage: wrapped,
            metadata: Some(MountMetadata {
                owner_uid: self.owner.as_str().to_string(),
                permissions: self.permissions,
            }),
        }]
    }
}
```

You will need to adapt the field names of `Mount` / `MountMetadata` / `MountResolver` to match whatever the SP7 code actually shipped — read `crates/crabcloud-fs/src/mount.rs` (or wherever the types live) and adjust.

- [ ] **Step 3: Unit test**

Append to `crates/crabcloud-fs/src/public_link_mount.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crabcloud_storage::memory::MemoryStorage;

    // Minimal StorageFactory stub that returns a preconfigured MemoryStorage.
    struct StubFactory(Arc<dyn Storage>);
    #[async_trait]
    impl StorageFactory for StubFactory {
        async fn home_storage(
            &self,
            _uid: &UserId,
        ) -> Result<Arc<dyn Storage>, crate::FsError> {
            Ok(self.0.clone())
        }
    }

    #[tokio::test]
    async fn resolver_returns_single_mount_for_link() {
        let inner: Arc<dyn Storage> = Arc::new(MemoryStorage::new("alice"));
        // Use the real owner uid; the resolver passes whatever owner it was
        // constructed with, regardless of the requester.
        let resolver = PublicLinkMountResolver::new(
            Arc::new(StubFactory(inner.clone())),
            UserId::new("alice").unwrap(),
            StoragePath::new("Photos").unwrap(),
            SharePermissions::from_wire(1),
        );
        let mounts = resolver
            .resolve_for(&UserId::new("alice").unwrap())
            .await;
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].path_prefix, StoragePath::root());
    }
}
```

- [ ] **Step 4: Build + test**

```bash
cargo test -p crabcloud-fs public_link_mount
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/crabcloud-fs/
git commit -m "fs: PublicLinkMountResolver returns single subroot mount"
```

### Task C3: Pre-PR sweep + PR

Standard sweep + push + PR. PR title `sp8(c): SharedSubrootStorage create-only + PublicLinkMountResolver`. Summary:

```
## Summary
- `SharedSubrootStorage` hides children from create-only file-drop links.
- New `PublicLinkMountResolver` returns the single subroot mount for anonymous viewers.

## Test plan
- [ ] CI green
- [ ] `cargo test -p crabcloud-fs` covers 7 new create-only cases + resolver unit test
```

---

# Batch D — Auth layer + config wiring

**Branch:** `sp8/d-auth-layer`

**Goal:** Land `PublicLinkAuthContext`, `PublicLinkAuthLayer`, the public-link secret in config, and the `AppState` integration. No routes yet — that's Batch E. End-to-end tests use a minimal axum router.

### Task D1: `public_link_secret` config field

**Files:**
- Modify: `crates/crabcloud-config/src/lib.rs`

- [ ] **Step 1: Add the field**

Find the top-level `AppConfig` struct and add:

```rust
    /// 32-byte secret used to HMAC public-link unlock cookies. Loaded from
    /// env `CC_PUBLIC_LINK_SECRET` (base64), or generated and persisted to
    /// `<data_dir>/public_link_secret` on first start.
    #[serde(default)]
    pub public_link_secret: Option<String>,
```

Then in the loader (find where the secret-from-data-dir pattern already exists for session secret — mirror it):

```rust
    pub fn resolve_public_link_secret(&self, data_dir: &Path) -> std::io::Result<Vec<u8>> {
        if let Some(b64) = std::env::var("CC_PUBLIC_LINK_SECRET").ok() {
            if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) {
                if bytes.len() >= 32 { return Ok(bytes); }
            }
        }
        let path = data_dir.join("public_link_secret");
        if path.exists() {
            return std::fs::read(&path);
        }
        let mut buf = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut buf);
        std::fs::write(&path, buf)?;
        Ok(buf.to_vec())
    }
```

Adjust imports as needed (`base64::Engine`, `rand::RngCore`).

- [ ] **Step 2: Commit**

```bash
git commit -am "config: public_link_secret field + resolver"
```

### Task D2: `PublicLinkAuthContext`

**Files:**
- Create: `crates/crabcloud-publiclinks/src/context.rs`

- [ ] **Step 1: Write `context.rs`**

```rust
use crabcloud_sharing::SharePermissions;
use crabcloud_storage::StoragePath;
use crabcloud_users::UserId;

/// Identity carried by anonymous public-link requests. Stored as a request
/// extension by `PublicLinkAuthLayer`; downstream handlers extract it to
/// build the `View`.
#[derive(Debug, Clone)]
pub struct PublicLinkAuthContext {
    pub link_share_id: i64,
    pub owner_uid: UserId,
    pub owner_path: StoragePath,
    pub permissions: SharePermissions,
}
```

Export from `lib.rs`:

```rust
mod context;
// ...
pub use context::PublicLinkAuthContext;
```

- [ ] **Step 2: Build + commit**

```bash
cargo build -p crabcloud-publiclinks
git commit -am "publiclinks: PublicLinkAuthContext extension type"
```

### Task D3: `PublicLinkAuthLayer` middleware

**Files:**
- Create: `crates/crabcloud-publiclinks/src/auth_layer.rs`

- [ ] **Step 1: Define a small lookup trait**

Add to `crates/crabcloud-publiclinks/src/tokens.rs`:

```rust
use async_trait::async_trait;

/// What the auth layer needs from the sharing service. Implemented by
/// `crabcloud-sharing::Shares` via a thin adapter in `crabcloud-core`
/// so this crate stays decoupled from the sharing crate.
#[async_trait]
pub trait TokenLookup: Send + Sync {
    async fn lookup(&self, token: &str) -> Result<Option<LinkRow>, std::io::Error>;
}

#[derive(Debug, Clone)]
pub struct LinkRow {
    pub share_id: i64,
    pub owner_uid: String,
    pub owner_path: String,
    pub permissions: u32,
    pub password_hash: Option<String>,
    pub expiration: Option<chrono::DateTime<chrono::Utc>>,
}
```

Add `async-trait` and `chrono` to `crabcloud-publiclinks/Cargo.toml` if not already there.

- [ ] **Step 2: Write `auth_layer.rs`**

This is the heart of the public-link auth flow. The middleware:

```rust
//! Axum middleware that resolves a public-link token from the request path,
//! enforces expiration, dispatches the password gate (cookie for `/s/{token}`
//! paths, Basic for `/public.php/dav/...` paths), and attaches a
//! `PublicLinkAuthContext` as a request extension.

use crate::{
    cookie::UnlockCookie,
    passwords::{HashedPassword, Passwords},
    ratelimit::{RateLimitDecision, RateLimiter},
    tokens::{Token, TokenLookup},
    PublicLinkAuthContext, PublicLinkError,
};
use axum::{
    body::Body,
    extract::Request,
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::Engine;
use crabcloud_sharing::SharePermissions;
use crabcloud_storage::StoragePath;
use crabcloud_users::UserId;
use std::sync::Arc;

pub struct PublicLinkAuthState {
    pub lookup: Arc<dyn TokenLookup>,
    pub passwords: Arc<Passwords>,
    pub rate_limiter: Arc<RateLimiter>,
    pub secret: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
pub enum AuthSurface {
    /// Browser-facing `/s/{token}` surface; password gate uses cookie.
    Browser,
    /// `/public.php/dav/...`; password gate uses HTTP Basic.
    Dav,
}

pub async fn public_link_auth(
    state: Arc<PublicLinkAuthState>,
    surface: AuthSurface,
    mut req: Request,
    next: Next,
) -> Response {
    let token = match extract_token(req.uri().path(), surface) {
        Some(t) => t,
        None => return not_found(),
    };

    let row = match state.lookup.lookup(token.as_str()).await {
        Ok(Some(r)) => r,
        Ok(None) => return not_found(),
        Err(_) => return server_error(),
    };

    // Expiration → indistinguishable from missing.
    if let Some(exp) = row.expiration {
        if exp < chrono::Utc::now() {
            return not_found();
        }
    }

    if let Some(hashed) = &row.password_hash {
        match surface {
            AuthSurface::Browser => {
                if !browser_unlocked(&state, &token, &req) {
                    // We don't 401 — the viewer page is the password gate.
                    // We add a marker to the request so the viewer handler
                    // knows to render the gate variant.
                    req.extensions_mut().insert(PasswordGateRequired);
                }
            }
            AuthSurface::Dav => {
                // RateLimiter first to avoid revealing whether the password is right.
                if let RateLimitDecision::Throttled { retry_after_secs } =
                    state.rate_limiter.check_password_attempt(token.as_str())
                {
                    return throttled(retry_after_secs);
                }
                if !dav_unlocked(&state, hashed, &req) {
                    return basic_challenge();
                }
            }
        }
    }

    let context = PublicLinkAuthContext {
        link_share_id: row.share_id,
        owner_uid: match UserId::new(row.owner_uid) {
            Ok(u) => u,
            Err(_) => return server_error(),
        },
        owner_path: match StoragePath::new(row.owner_path.trim_start_matches('/')) {
            Ok(p) => p,
            Err(_) => return server_error(),
        },
        permissions: SharePermissions::from_wire(row.permissions),
    };
    req.extensions_mut().insert(context);

    next.run(req).await
}

#[derive(Debug, Clone, Copy)]
pub struct PasswordGateRequired;

fn extract_token(path: &str, surface: AuthSurface) -> Option<Token> {
    // Caller has already routed to /s/{token} or /public.php/dav/files/{token}.
    // The first non-prefix path segment is the token.
    let stripped = match surface {
        AuthSurface::Browser => path.strip_prefix("/s/")?,
        AuthSurface::Dav => path.strip_prefix("/public.php/dav/files/")?,
    };
    let first = stripped.split('/').next()?;
    Token::parse(first)
}

fn browser_unlocked(
    state: &PublicLinkAuthState,
    token: &Token,
    req: &Request,
) -> bool {
    let name = UnlockCookie::cookie_name_for(token.as_str());
    let cookies = req
        .headers()
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|h| h.to_str().ok());
    for header_value in cookies {
        for pair in header_value.split(';') {
            let pair = pair.trim();
            if let Some(rest) = pair.strip_prefix(&format!("{name}=")) {
                if let Ok(_exp) =
                    UnlockCookie::verify(&state.secret, token.as_str(), rest, chrono::Utc::now().timestamp())
                {
                    return true;
                }
            }
        }
    }
    false
}

fn dav_unlocked(state: &PublicLinkAuthState, hashed: &str, req: &Request) -> bool {
    let header = match req.headers().get(header::AUTHORIZATION) {
        Some(h) => h,
        None => return false,
    };
    let s = match header.to_str() {
        Ok(s) => s,
        Err(_) => return false,
    };
    let token_part = match s.strip_prefix("Basic ") {
        Some(t) => t,
        None => return false,
    };
    let decoded = match base64::engine::general_purpose::STANDARD.decode(token_part) {
        Ok(b) => b,
        Err(_) => return false,
    };
    let s = match std::str::from_utf8(&decoded) {
        Ok(s) => s,
        Err(_) => return false,
    };
    // Public-link DAV expects "anonymous:password" or ":password". We ignore
    // the username (some clients send "anonymous", some "").
    let password = s.splitn(2, ':').nth(1).unwrap_or("");
    let hp = HashedPassword::from_stored(hashed.to_string());
    state.passwords.verify(password, &hp)
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "").into_response()
}

fn server_error() -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, "").into_response()
}

fn basic_challenge() -> Response {
    let mut resp = (StatusCode::UNAUTHORIZED, "").into_response();
    resp.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Basic realm=\"public-link\""),
    );
    resp
}

fn throttled(retry_after_secs: u64) -> Response {
    let mut resp = (StatusCode::TOO_MANY_REQUESTS, "").into_response();
    resp.headers_mut().insert(
        header::RETRY_AFTER,
        HeaderValue::from_str(&retry_after_secs.to_string())
            .unwrap_or(HeaderValue::from_static("3600")),
    );
    resp
}

// `Method` import kept for future MKCOL/OPTIONS branching in Batch F.
#[allow(dead_code)]
fn _retain_method(_: Method) {}
```

- [ ] **Step 3: Add export**

In `crabcloud-publiclinks/src/lib.rs`:

```rust
mod auth_layer;
pub use auth_layer::{public_link_auth, AuthSurface, PasswordGateRequired, PublicLinkAuthState};
```

- [ ] **Step 4: Build**

```bash
cargo build -p crabcloud-publiclinks
```

Expected: clean build.

- [ ] **Step 5: Commit**

```bash
git commit -am "publiclinks: auth_layer middleware (cookie + Basic gates)"
```

### Task D4: TokenLookup adapter in `crabcloud-core`

**Files:**
- Modify: `crates/crabcloud-core/src/lib.rs` (or wherever `AppState` lives) — add a small adapter struct that implements `TokenLookup` for `Shares`.

- [ ] **Step 1: Write the adapter**

```rust
use crabcloud_publiclinks::tokens::{LinkRow, TokenLookup};
use crabcloud_sharing::Shares;
use async_trait::async_trait;
use std::sync::Arc;

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
            owner_path: r.file_target.clone(),
            permissions: r.permissions.as_u32(),
            password_hash: r.password_hash,
            expiration: r.expiration,
        }))
    }
}
```

Note: Task B3 stores `file_target = format!("/{}", storage_path.as_str())` for link rows (full owner path), which is what `r.file_target` returns here. The auth layer uses this verbatim as the `SharedSubrootStorage` root.

- [ ] **Step 2: Wire into `AppState`**

`AppState` should expose `pub publiclinks_auth: Arc<PublicLinkAuthState>` built from:

```rust
let shares = Arc::new(/* existing shares */);
let lookup: Arc<dyn TokenLookup> = Arc::new(SharesTokenLookup { shares: shares.clone() });
let rate_limiter = Arc::new(RateLimiter::new());
let passwords = Arc::new(Passwords::new());
let secret = cfg.resolve_public_link_secret(&data_dir)?;
let publiclinks_auth = Arc::new(PublicLinkAuthState { lookup, passwords, rate_limiter, secret });
```

- [ ] **Step 3: Commit**

```bash
git commit -am "core: SharesTokenLookup adapter + AppState wiring"
```

### Task D5: End-to-end auth-layer test with a minimal axum router

**Files:**
- Create: `crates/crabcloud-publiclinks/tests/auth_layer_e2e.rs`

- [ ] **Step 1: Write the test**

```rust
//! End-to-end tests for `public_link_auth` against a minimal axum router.
//! Uses a stub `TokenLookup` so the auth layer's behavior is independent
//! of the sharing service.

use axum::{body::Body, http::{header, Request, StatusCode}, Router};
use crabcloud_publiclinks::{
    cookie::UnlockCookie, passwords::Passwords, ratelimit::RateLimiter,
    tokens::{LinkRow, TokenLookup},
    AuthSurface, PublicLinkAuthState, PasswordGateRequired,
};
use std::sync::Arc;
use tower::ServiceExt;

// Stub lookup ...

// Test: GET /s/<token> with no password → 200 + handler ran
// Test: GET /s/<token> with password, no cookie → 200 + PasswordGateRequired extension set
// Test: GET /s/<token> with password, valid cookie → 200 + handler ran
// Test: GET /s/<token> for unknown token → 404
// Test: GET /s/<token> for expired token → 404
// Test: GET /public.php/dav/files/<token>/foo with password, no Basic → 401
// Test: PUT /public.php/dav/files/<token>/foo with valid Basic → 201
// Test: 11 wrong passwords on DAV → 429 on the 11th
```

Flesh out each test using `tower::ServiceExt::oneshot` against a Router that has the auth layer plus a handler that returns 200 and records whether `PublicLinkAuthContext` was present.

This task is substantial — budget ~30 min. Mirror the pattern in `crates/crabcloud-http/tests/auth_layer.rs`.

- [ ] **Step 2: Run tests**

```bash
cargo test -p crabcloud-publiclinks --test auth_layer_e2e
```

Expected: all 8 cases pass.

- [ ] **Step 3: Commit**

```bash
git commit -am "publiclinks(tests): auth_layer e2e covers cookie + Basic + expiry + throttle"
```

### Task D6: Pre-PR sweep + PR

Standard. PR title: `sp8(d): public_link_secret + PublicLinkAuthLayer + TokenLookup adapter`.

---

# Batch E — OCS create/update + viewer SSR + HTTP handlers

**Branch:** `sp8/e-ocs-and-viewer`

**Goal:** Lift OCS to accept `shareType=3`. Ship the dx SSR viewer page, password gate, download endpoint, file-drop upload endpoint, and folder-zip endpoint. Full e2e coverage of the browser-facing surface.

### Task E1: OCS create accepts `shareType=3`

**Files:**
- Modify: `crates/crabcloud-http/src/routes/ocs/files_sharing.rs`

- [ ] **Step 1: Extend `CreateShareForm`**

Find the deserialization struct for OCS create. Add:

```rust
    #[serde(rename = "password", default)]
    pub password: Option<String>,
    #[serde(rename = "expireDate", default)]
    pub expire_date: Option<String>, // YYYY-MM-DD
```

- [ ] **Step 2: Translate in the handler**

In `create_handler`, where it builds `CreateShareRequest`:

```rust
        let expire_date = form
            .expire_date
            .as_deref()
            .map(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d"))
            .transpose()
            .map_err(|_| {
                ocs_envelope(400, "invalid expireDate", Value::Null, fmt)
            })?;
        let req = CreateShareRequest {
            requester: ctx.user_id.as_str().to_string(),
            path: form.path,
            share_type,
            share_with: form.share_with.unwrap_or_default(),
            permissions: form.permissions.unwrap_or(1),
            home_storage_id,
            password: form.password,
            expire_date,
        };
```

- [ ] **Step 3: Serialize link fields in `share_to_json`**

For link rows, include:

```rust
    if matches!(row.share_type, ShareType::Link) {
        if let Some(t) = &row.token {
            json["token"] = Value::String(t.clone());
            // url is host-dependent — read from AppConfig::public_base_url, fallback to "/s/{token}"
            json["url"] = Value::String(format!("/s/{t}"));
        }
        json["share_with"] = match &row.password_hash {
            Some(_) => Value::String("***".into()),
            None => Value::Null,
        };
    }
```

- [ ] **Step 4: E2E test**

Add to `crates/crabcloud-http/tests/public_link_e2e.rs`:

```rust
#[tokio::test]
async fn ocs_create_link_returns_token_and_url() {
    // standard test harness: login as alice, POST to OCS, assert response shape
}
```

- [ ] **Step 5: Commit**

```bash
git commit -am "ocs: create accepts shareType=3 with password+expireDate"
```

### Task E2: OCS update wires password and expiration

**Files:**
- Modify: `crates/crabcloud-http/src/routes/ocs/files_sharing.rs`

- [ ] **Step 1: Translate update fields**

Mirror the create translation. In `update_handler`, after parsing `UpdateForm`:

```rust
        let expire_date_opt = match form.expire_date.as_deref() {
            None => None,
            Some("") => Some(None),
            Some(s) => Some(Some(chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .map_err(|_| ocs_envelope(400, "invalid expireDate", Value::Null, fmt))?)),
        };
        let password_opt = match form.password.as_deref() {
            None => None,
            Some("") => Some(None),
            Some(pw) => Some(Some(pw.to_string())),
        };
        let fields = UpdateShareFields {
            permissions: form.permissions,
            expire_date: expire_date_opt,
            password: password_opt,
            note: None,
        };
```

- [ ] **Step 2: E2E test**

Add `ocs_update_link_sets_password_and_expiration` to the e2e file.

- [ ] **Step 3: Commit**

```bash
git commit -am "ocs: update wires password+expiration for link rows"
```

### Task E3: dx Router registers `/s/:token` + `/s/:token/*path`

**Files:**
- Modify: `crates/crabcloud-app/src/app.rs`
- Create: `crates/crabcloud-app/src/pages/public_link.rs`
- Modify: `crates/crabcloud-app/src/pages/mod.rs`

- [ ] **Step 1: Wire route in `app.rs`**

Find the Router setup (look for `dioxus_router::components::Route` or `routable!`). Add:

```rust
    #[route("/s/:token")]
    PublicLinkRoot { token: String },
    #[route("/s/:token/*path")]
    PublicLink { token: String, path: Vec<String> },
```

Or follow the dx 0.7 routable enum convention used by Files.

- [ ] **Step 2: Stub the page**

In `pages/public_link.rs`:

```rust
use dioxus::prelude::*;

#[component]
pub fn PublicLinkRoot(token: String) -> Element {
    rsx! {
        div { class: "public-link-viewer",
            h1 { "Public link {token}" }
            p { "Placeholder — viewer UI lands in E4." }
        }
    }
}

#[component]
pub fn PublicLink(token: String, path: Vec<String>) -> Element {
    PublicLinkRoot(token)
}
```

- [ ] **Step 3: Build the WASM bundle**

```bash
just build-wasm  # or whatever the project uses to compile the dx WASM bundle
```

Expected: clean build. Visiting `/s/foo` in a browser should render the placeholder.

- [ ] **Step 4: Commit**

```bash
git add crates/crabcloud-app/
git commit -m "app: stub /s/:token route + placeholder page"
```

### Task E4: Viewer page chrome + folder browse

**Files:**
- Modify: `crates/crabcloud-app/src/pages/public_link.rs`
- Create: server function for "list public link contents"

- [ ] **Step 1: Server function**

Add a `#[server]` function in `crabcloud-app/src/server_fns/public_link.rs`:

```rust
#[server(endpoint = "/public_link/list")]
pub async fn list_public_link(
    token: String,
    path: String,
) -> Result<Vec<FileEntry>, ServerFnError> {
    use crabcloud_publiclinks::tokens::Token;
    use crabcloud_publiclinks::PublicLinkAuthContext;
    use crabcloud_fs::PublicLinkMountResolver;

    let _validated = Token::parse(&token)
        .ok_or_else(|| ServerFnError::ServerError("bad token".into()))?;
    // Resolve via AppState.shares; build a single-mount View; list path.
    // [details: ~30 lines mirroring crabcloud-ui/src/server_fns/files.rs list]
    todo!()
}
```

Implement the body by following `crates/crabcloud-ui/src/server_fns/files.rs::list` line-by-line, swapping the user-derived `View` for one built from `PublicLinkMountResolver`.

- [ ] **Step 2: Wire the page**

In `pages/public_link.rs`, swap the placeholder for a real viewer that calls `list_public_link` and renders `FileRow` (reuse the component from the Files page) for each entry. Add a `Breadcrumb` (also reused).

- [ ] **Step 3: Run dev server, manually verify**

```bash
dx serve --bin crabcloud-app
# in another shell, create a link, visit /s/<token>, confirm the folder listing renders
```

- [ ] **Step 4: Commit**

```bash
git commit -am "app: viewer page renders folder listing for public link"
```

### Task E5: Password gate variant + `POST /s/{token}/unlock`

**Files:**
- Modify: `crates/crabcloud-app/src/pages/public_link.rs`
- Create: `crates/crabcloud-http/src/routes/public_link/mod.rs`
- Modify: `crates/crabcloud-http/src/routes/mod.rs`

- [ ] **Step 1: HTTP handler for `/s/{token}/unlock`**

Create the new module `crabcloud-http/src/routes/public_link/mod.rs`:

```rust
//! Browser-facing public-link endpoints under /s/{token}.

use axum::{extract::{Form, Path, State}, http::{header, HeaderValue, StatusCode}, response::Response, routing::post, Router};
use crabcloud_core::AppState;
use crabcloud_publiclinks::{cookie::UnlockCookie, ratelimit::RateLimitDecision};
use serde::Deserialize;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/s/{token}/unlock", post(unlock_handler))
        // download / upload / zip wired in later tasks
}

#[derive(Deserialize)]
struct UnlockForm {
    password: String,
}

async fn unlock_handler(
    State(state): State<AppState>,
    Path(token): Path<String>,
    Form(form): Form<UnlockForm>,
) -> Response {
    // 1. RateLimiter.check_password_attempt(token) → 429 if throttled.
    // 2. state.publiclinks_auth.lookup.lookup(token) → 404 if missing/expired.
    // 3. If row.password_hash is None → 400 (link has no password).
    // 4. state.publiclinks_auth.passwords.verify(form.password, hash) → 401 on mismatch.
    // 5. Build cookie value, return 302 → /s/{token} with Set-Cookie.
    todo!()
}
```

Implement the body using the helpers from Batch D.

- [ ] **Step 2: Server-side render the gate**

In `pages/public_link.rs`, the SSR loader needs to know whether to render the gate variant. Use a separate `#[server]` function `meta_public_link(token)` that returns:

```rust
struct PublicLinkMeta {
    requires_password: bool,
    cookie_present: bool,
    ...
}
```

If `requires_password && !cookie_present` → render `PasswordGateForm`. Otherwise render the listing.

- [ ] **Step 3: Mount router**

In `crates/crabcloud-http/src/routes/mod.rs`, merge the public-link router into the main router. Make sure NO other auth layers (session, CSRF) wrap `/s/...`.

- [ ] **Step 4: E2E**

Add tests:

- `password_gate_renders_when_no_cookie`
- `unlock_correct_password_sets_cookie_and_redirects`
- `unlock_wrong_password_returns_401`
- `eleven_wrong_passwords_returns_429`

- [ ] **Step 5: Commit**

```bash
git commit -am "publiclink: /s/{token}/unlock + password gate variant"
```

### Task E6: Download endpoint

**Files:**
- Modify: `crates/crabcloud-http/src/routes/public_link/mod.rs`

- [ ] **Step 1: Add route + handler**

```rust
        .route("/s/{token}/download/{*path}", get(download_handler))
```

```rust
async fn download_handler(
    State(state): State<AppState>,
    Path((token, path)): Path<(String, String)>,
    Extension(ctx): Extension<PublicLinkAuthContext>,
    headers: HeaderMap,
) -> Response {
    if !ctx.permissions.contains_read() {
        return (StatusCode::FORBIDDEN, "").into_response();
    }
    let view = build_public_link_view(&state, &ctx).await?;
    let storage_path = match StoragePath::new(&path) { Ok(p) => p, Err(_) => return (StatusCode::BAD_REQUEST, "").into_response() };
    // Range support: reuse existing dav GET range logic if available.
    let reader = view.read(&storage_path).await?;
    // Stream as body...
}
```

Mirror the existing authenticated GET handler for files (`crates/crabcloud-http/src/routes/files.rs` or similar) for content-type detection, Range, etc.

The `build_public_link_view` helper:

```rust
async fn build_public_link_view(state: &AppState, ctx: &PublicLinkAuthContext) -> View {
    let resolver = Arc::new(PublicLinkMountResolver::new(
        state.storage_factory.clone(),
        ctx.owner_uid.clone(),
        ctx.owner_path.clone(),
        ctx.permissions,
    ));
    View::new_with_resolver(state.filecache.clone(), resolver, ctx.owner_uid.clone())
}
```

- [ ] **Step 2: E2E test**

`download_read_link_returns_body`, `download_create_only_link_returns_403`, `download_with_range_returns_partial_content`.

- [ ] **Step 3: Commit**

```bash
git commit -am "publiclink: /s/{token}/download/{*path} streams file"
```

### Task E7: Folder zip endpoint

**Files:**
- Modify: `crates/crabcloud-http/src/routes/public_link/mod.rs`
- Add: `zip` crate dep if not already present.

- [ ] **Step 1: Handler**

```rust
        .route("/s/{token}/zip/{*path}", get(zip_handler))
```

```rust
async fn zip_handler(...) -> Response {
    // 1. Require read bit.
    // 2. List the folder recursively up to 500 entries / 2GiB uncompressed.
    // 3. Stream a zip archive via the `zip` crate's streaming API.
    // 4. Cap reached → 413 Payload Too Large.
}
```

Implementation: use `zip::write::ZipWriter` with `tokio::io` adapters. The cap-checking walk happens before zip start.

- [ ] **Step 2: E2E test**

`zip_folder_returns_zip_with_expected_entries`, `zip_over_cap_returns_413`.

- [ ] **Step 3: Commit**

```bash
git commit -am "publiclink: /s/{token}/zip/{*path} streams folder zip with cap"
```

### Task E8: Upload widget on viewer

**Files:**
- Modify: `crates/crabcloud-app/src/pages/public_link.rs`
- Reuse: existing upload widget component (find in `crates/crabcloud-ui/src/components/upload.rs` or similar)

- [ ] **Step 1: Add upload widget conditionally**

```rust
    if ctx.permissions.allows_create() && !ctx.permissions.contains_read() {
        // file-drop only — show only the upload widget
        rsx! { PublicLinkUploadWidget { token } }
    } else if ctx.permissions.allows_create() {
        // mixed — show both listing and upload
        rsx! {
            FolderListing { ... }
            PublicLinkUploadWidget { token }
        }
    } else {
        rsx! { FolderListing { ... } }
    }
```

- [ ] **Step 2: Commit**

```bash
git commit -am "app: upload widget on file-drop public-link viewer"
```

### Task E9: Upload handler with collision suffix + quota

**Files:**
- Modify: `crates/crabcloud-http/src/routes/public_link/mod.rs`

- [ ] **Step 1: Handler**

```rust
        .route("/s/{token}/upload/{filename}", post(upload_handler))
```

```rust
async fn upload_handler(
    State(state): State<AppState>,
    Path((token, filename)): Path<(String, String)>,
    Extension(ctx): Extension<PublicLinkAuthContext>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if !ctx.permissions.allows_create() {
        return (StatusCode::FORBIDDEN, "").into_response();
    }
    if !is_safe_filename(&filename) {
        return (StatusCode::BAD_REQUEST, "invalid filename").into_response();
    }
    // Rate-limit per IP
    let ip = client_ip(&headers);
    if matches!(state.publiclinks_auth.rate_limiter.check_upload(&ip), RateLimitDecision::Throttled { .. }) {
        return (StatusCode::TOO_MANY_REQUESTS, "").into_response();
    }
    // Quota check
    let content_length = headers.get(header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    let remaining = state.users.quota_remaining(&ctx.owner_uid).await?;
    if content_length > remaining {
        return (StatusCode::INSUFFICIENT_STORAGE, "").into_response();
    }
    // Collision suffix loop
    let view = build_public_link_view(&state, &ctx).await?;
    let final_name = resolve_collision(&view, &filename).await?;
    let storage_path = StoragePath::new(&final_name)?;
    // Stream-write
    let reader = body_to_async_read(body);
    view.create(&storage_path, Box::pin(reader)).await?;
    let resp = serde_json::json!({ "name": final_name });
    (StatusCode::CREATED, axum::Json(resp)).into_response()
}

async fn resolve_collision(view: &View, name: &str) -> Result<String, ...> {
    if !view.stat(&StoragePath::new(name)?).await.is_ok() {
        return Ok(name.to_string());
    }
    let (stem, ext) = split_ext(name);
    for i in 1..=50 {
        let candidate = match ext {
            Some(e) => format!("{stem} ({i}).{e}"),
            None => format!("{stem} ({i})"),
        };
        if view.stat(&StoragePath::new(&candidate)?).await.is_err() {
            return Ok(candidate);
        }
    }
    Err(...)  // 409 Conflict
}

fn is_safe_filename(s: &str) -> bool {
    !s.is_empty()
        && !s.contains('/')
        && !s.contains('\\')
        && !s.contains('\0')
        && !s.starts_with("..")
        && s.chars().all(|c| !c.is_control())
}

fn client_ip(headers: &HeaderMap) -> String {
    headers.get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}
```

- [ ] **Step 2: E2E tests**

- `upload_create_link_writes_file`
- `upload_collision_suffix_increments`
- `upload_unsafe_filename_returns_400`
- `upload_over_quota_returns_507`
- `upload_per_ip_throttled_after_60`

- [ ] **Step 3: Commit**

```bash
git commit -am "publiclink: /s/{token}/upload/{filename} with quota + collision + rate-limit"
```

### Task E10: Pre-PR sweep + PR

Standard. PR title: `sp8(e): OCS link shape + viewer page + download + upload + zip`.

---

# Batch F — Public WebDAV

**Branch:** `sp8/f-public-webdav`

**Goal:** Mount `/public.php/dav/files/{token}/...` and reuse the existing dav adapter under `PublicLinkAuthLayer`. GET/PROPFIND/PUT work; MKCOL/DELETE/MOVE/COPY are forbidden by the storage wrapper's permission checks.

### Task F1: Router wiring

**Files:**
- Create: `crates/crabcloud-http/src/routes/public_dav.rs`
- Modify: `crates/crabcloud-http/src/routes/mod.rs`

- [ ] **Step 1: Inspect existing dav adapter**

```bash
cat crates/crabcloud-http/src/routes/dav.rs
```

Identify the function that takes a `View` + axum `Request` and produces a `Response`. We'll call it directly from our public-dav handler.

- [ ] **Step 2: Build the router**

```rust
//! `/public.php/dav/files/{token}/{*path}` — anonymous WebDAV surface for
//! public links. Reuses the existing dav adapter; `PublicLinkAuthLayer`
//! supplies the `View` via `PublicLinkAuthContext`.

use axum::{extract::{Path, State}, response::Response, routing::any, Router};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/public.php/dav/files/{token}/{*path}", any(public_dav_handler))
        .route("/public.php/dav/files/{token}/", any(public_dav_handler_root))
}

async fn public_dav_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<PublicLinkAuthContext>,
    req: axum::extract::Request,
) -> Response {
    let view = build_public_link_view(&state, &ctx).await;
    crate::routes::dav::serve_via_view(view, req).await
}
```

You'll need to expose `serve_via_view` (or whatever the equivalent is) as `pub(crate)` from `dav.rs`.

- [ ] **Step 3: Mount with auth layer**

In `routes/mod.rs`:

```rust
    let public_dav = public_dav::router().layer(axum::middleware::from_fn_with_state(
        app_state.publiclinks_auth.clone(),
        |state, req, next| public_link_auth(state, AuthSurface::Dav, req, next),
    ));
    let public_link = public_link::router().layer(axum::middleware::from_fn_with_state(
        app_state.publiclinks_auth.clone(),
        |state, req, next| public_link_auth(state, AuthSurface::Browser, req, next),
    ));
```

- [ ] **Step 4: Commit**

```bash
git commit -am "http: /public.php/dav/files/{token} router wired to public_link_auth"
```

### Task F2: PROPFIND + GET e2e

**Files:**
- Create: `crates/crabcloud-http/tests/public_dav_e2e.rs`

- [ ] **Step 1: Write tests**

```rust
#[tokio::test]
async fn propfind_read_link_returns_multistatus() {
    // 1. Build AppState with cfg.filecache.enabled = false
    // 2. Seed alice's home with /Vacation/photo.jpg
    // 3. Create a read-only link on /Vacation
    // 4. PROPFIND /public.php/dav/files/<token>/ → 207 Multi-Status with photo.jpg entry
}

#[tokio::test]
async fn get_read_link_returns_file_body() { ... }

#[tokio::test]
async fn propfind_create_only_link_returns_403() { ... }

#[tokio::test]
async fn put_create_only_link_writes_file() { ... }

#[tokio::test]
async fn put_with_wrong_basic_password_returns_401() { ... }

#[tokio::test]
async fn put_with_correct_basic_password_writes() { ... }

#[tokio::test]
async fn delete_read_link_returns_403() { ... }
```

Use the test patterns from `crates/crabcloud-http/tests/dav_basic.rs`.

- [ ] **Step 2: Run + iterate until green**

```bash
cargo test -p crabcloud-http --test public_dav_e2e
```

- [ ] **Step 3: Commit**

```bash
git commit -am "http(tests): public DAV e2e covers PROPFIND/GET/PUT/auth"
```

### Task F3: Pre-PR sweep + PR

Standard. PR title: `sp8(f): public WebDAV at /public.php/dav/files/{token}`.

---

# Batch G — Smoke binary + final polish

**Branch:** `sp8/g-smoke-and-polish`

**Goal:** Ship a dx smoke binary, sweep the workspace, ensure docs are current, open the final PR that closes SP8.

### Task G1: Smoke binary

**Files:**
- Create: `crates/crabcloud-app/src/bin/smoke_public_link.rs`

- [ ] **Step 1: Write the binary**

```rust
//! Headless smoke for /s/{token} viewer + download. Boots the dx server in
//! the background, seeds a public link, hits /s/<token> and asserts the
//! response shape, then GET /s/<token>/download/<file> and asserts bytes.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Spin up server with cfg.filecache.enabled = false
    // 2. Seed alice + /Vacation/photo.jpg + read-only link
    // 3. reqwest GET /s/<token> → assert 200 and HTML body contains "photo.jpg"
    // 4. reqwest GET /s/<token>/download/photo.jpg → assert body == seeded bytes
    // 5. Tear down server, exit 0
}
```

Mirror `crates/crabcloud-app/src/bin/smoke_*.rs` if one already exists.

- [ ] **Step 2: Run locally**

```bash
cargo run --bin smoke-public-link
```

Expected: exit 0.

- [ ] **Step 3: Commit**

```bash
git commit -am "app: smoke_public_link headless verification binary"
```

### Task G2: Final workspace sweep

- [ ] **Step 1: Run the full battery**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
just build-wasm   # or equivalent
```

All four must pass. Fix any drift.

- [ ] **Step 2: Open the PR**

```bash
git push -u origin sp8/g-smoke-and-polish
gh pr create --title "sp8(g): smoke binary + workspace polish" --body "$(cat <<'EOF'
## Summary
- `smoke-public-link` headless verifier.
- Workspace fmt/clippy/test pass.
- SP8 sub-project complete.

## Test plan
- [ ] CI green (fmt-and-clippy, build-wasm, test-sqlite, test-multidialect, e2e)
- [ ] `cargo run --bin smoke-public-link` exits 0 locally

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

---

## Acceptance criteria (spec → coverage map)

| Spec section | Test / artifact |
|---|---|
| §2 Decision 1 (Nextcloud URLs) | Routes registered at `/s/{token}` and `/public.php/dav/files/{token}` (E3, F1). |
| §2 Decision 2 (virtual user) | `PublicLinkMountResolver` returns single mount as owner_uid (C2 test). |
| §2 Decision 3 (signed cookie) | `cookie::tests` + `auth_layer_e2e` browser path (A4, D5). |
| §2 Decision 4 (permissions) | E2E for read/create-only/mixed (E6, E9, F2 tests). |
| §2 Decision 5 (token format) | `tokens::tests` for length, charset, uniqueness (A2). |
| §2 Decision 6 (Argon2id) | `passwords::tests` (A3). |
| §2 Decision 7 (expiration → 404) | `auth_layer_e2e::expired_token_returns_404` (D5). |
| §2 Decision 8 (file-drop suffix) | `upload_collision_suffix_increments` (E9). |
| §2 Decision 9 (rate limiting) | `ratelimit::tests` + `eleven_wrong_passwords_returns_429` + `upload_per_ip_throttled_after_60` (A5, D5, E5, E9). |
| §2 Decision 10 (new crate boundary) | Crate exists, sharing depends on it; no inverse dep (A1, B1). |
| §2 Decision 11 (dedicated viewer route) | `pages/public_link.rs` route + e2e (E3-E8). |
| §3.1 anonymous download flow | `download_read_link_returns_body` (E6). |
| §3.2 file-drop upload flow | `upload_create_link_writes_file` + quota + collision (E9). |
| §3.3 password gate flow | `unlock_correct_password_sets_cookie_and_redirects` (E5). |
| §4.1 schema | No migration; reuses SP7 columns. Verified by Batch B tests on all 3 dialects. |
| §4.2 HTTP endpoints | Every row mapped to a handler in E3-E9 + F1-F2. |
| §5 auth flow | `auth_layer.rs` + `auth_layer_e2e` (D3, D5). |
| §6 file-drop semantics | `share_subroot::tests::create_only_*` (C1) + upload tests (E9). |
| §7.1-7.4 testing strategy | All unit/integration/e2e tests in batches A-F. |
| §8 risks | Mitigations baked into the implementation; no separate task. |
