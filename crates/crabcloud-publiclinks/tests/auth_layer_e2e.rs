//! End-to-end tests for `public_link_auth` against a minimal axum router.
//!
//! Uses a stub `TokenLookup` driven by a `HashMap<&str, LinkRow>` so the
//! auth layer's behaviour is exercised independently of the sharing
//! service. The handler under the layer reports back via response headers
//! whether `PublicLinkAuthContext` / `PasswordGateRequired` were attached,
//! which keeps assertions cheap.

#![allow(unused_crate_dependencies)]

use async_trait::async_trait;
use axum::{
    body::Body,
    extract::Request,
    http::{header, HeaderValue, StatusCode},
    middleware::{from_fn_with_state, Next},
    response::{IntoResponse, Response},
    routing::any,
    Router,
};
use base64::Engine;
use chrono::{Duration, Utc};
use crabcloud_publiclinks::{
    public_link_auth, AuthSurface, LinkRow, Passwords, PublicLinkAuthContext, PublicLinkAuthState,
    RateLimiter, TokenLookup, UnlockCookie,
};
use std::{collections::HashMap, sync::Arc};
use tower::ServiceExt;

const SECRET: &[u8] = b"e2e-test-secret-32-bytes--------";

// --- Stub TokenLookup --------------------------------------------------------

struct StubLookup {
    rows: HashMap<String, LinkRow>,
}

impl StubLookup {
    fn new() -> Self {
        Self {
            rows: HashMap::new(),
        }
    }
    fn insert(mut self, token: &str, row: LinkRow) -> Self {
        self.rows.insert(token.to_string(), row);
        self
    }
}

#[async_trait]
impl TokenLookup for StubLookup {
    async fn lookup(&self, token: &str) -> Result<Option<LinkRow>, std::io::Error> {
        Ok(self.rows.get(token).cloned())
    }
}

// --- Fixture helpers ---------------------------------------------------------

/// 15-char `[A-Za-z0-9]` token shape (matches `Token::parse`).
fn tok(seed: char) -> String {
    let mut s = String::with_capacity(15);
    for i in 0..15 {
        let c = (b'A' + ((i as u8 + seed as u8) % 26)) as char;
        s.push(c);
    }
    s
}

fn row(share_id: i64, owner: &str, path: &str, perms: u32) -> LinkRow {
    LinkRow {
        share_id,
        owner_uid: owner.into(),
        owner_path: path.into(),
        permissions: perms,
        password_hash: None,
        expiration: None,
    }
}

fn row_with_password(share_id: i64, owner: &str, path: &str, perms: u32, pw: &str) -> LinkRow {
    let mut r = row(share_id, owner, path, perms);
    r.password_hash = Some(Passwords::new().hash(pw).unwrap().as_str().to_string());
    r
}

/// Probe handler — records whether the auth context / gate marker were set.
///
/// Emits both `x-needs-gate` (from the `PasswordGateRequired` marker
/// extension) and `x-ctx-gate-required` (from the field on
/// `PublicLinkAuthContext`). The two should always agree on the Browser
/// surface — that redundancy is the whole point of the field: handlers can
/// gate-check off the context alone without rummaging through extensions.
async fn probe(req: Request) -> Response {
    let ctx = req.extensions().get::<PublicLinkAuthContext>();
    let has_ctx = ctx.is_some();
    let needs_gate = req
        .extensions()
        .get::<crabcloud_publiclinks::PasswordGateRequired>()
        .is_some();
    let share_id = ctx.map(|c| c.link_share_id).unwrap_or(0);
    let ctx_gate_required = ctx.map(|c| c.password_gate_required).unwrap_or(false);
    let mut resp = (StatusCode::OK, "ok").into_response();
    let h = resp.headers_mut();
    h.insert(
        "x-has-context",
        HeaderValue::from_static(if has_ctx { "1" } else { "0" }),
    );
    h.insert(
        "x-needs-gate",
        HeaderValue::from_static(if needs_gate { "1" } else { "0" }),
    );
    h.insert(
        "x-ctx-gate-required",
        HeaderValue::from_static(if ctx_gate_required { "1" } else { "0" }),
    );
    h.insert(
        "x-share-id",
        HeaderValue::from_str(&share_id.to_string()).unwrap(),
    );
    resp
}

fn router_for(state: Arc<PublicLinkAuthState>, surface: AuthSurface) -> Router {
    // `from_fn_with_state` clones `state` per request; we wrap it in a
    // closure that forwards to `public_link_auth(state, surface, ...)`.
    //
    // Mirrors `build_router`'s mount shape: the inner router exposes
    // nest-relative routes and is nested under the surface prefix so the
    // auth middleware sees the post-strip path.
    let state_for_mw = state.clone();
    let mw = move |req: Request, next: Next| {
        let s = state_for_mw.clone();
        async move { public_link_auth(s, surface, req, next).await }
    };
    let inner = Router::new()
        // Catch-all so we don't care which exact path each test hits.
        .route("/{*rest}", any(probe))
        .route_layer(from_fn_with_state(state, mw));
    let prefix = match surface {
        AuthSurface::Browser => "/s",
        AuthSurface::Dav => "/public.php/dav/files",
    };
    Router::new().nest(prefix, inner)
}

fn make_state(lookup: Arc<dyn TokenLookup>) -> Arc<PublicLinkAuthState> {
    Arc::new(PublicLinkAuthState {
        lookup,
        passwords: Arc::new(Passwords::new()),
        rate_limiter: Arc::new(RateLimiter::new()),
        secret: SECRET.to_vec(),
    })
}

// --- Browser surface tests ---------------------------------------------------

#[tokio::test]
async fn browser_no_password_attaches_context() {
    let t = tok('A');
    let lookup = Arc::new(StubLookup::new().insert(&t, row(101, "alice", "/Photos", 1)));
    let state = make_state(lookup);
    let app = router_for(state, AuthSurface::Browser);

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/s/{t}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers().get("x-has-context").unwrap(), "1");
    assert_eq!(resp.headers().get("x-needs-gate").unwrap(), "0");
    // No password on this link → context's gate flag must be false.
    assert_eq!(resp.headers().get("x-ctx-gate-required").unwrap(), "0");
    assert_eq!(resp.headers().get("x-share-id").unwrap(), "101");
}

#[tokio::test]
async fn browser_with_password_no_cookie_attaches_gate_marker() {
    let t = tok('B');
    let lookup =
        Arc::new(StubLookup::new().insert(&t, row_with_password(202, "bob", "/Secret", 1, "pw1")));
    let state = make_state(lookup);
    let app = router_for(state, AuthSurface::Browser);

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/s/{t}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // Gate marker is set so the viewer can branch to the password form.
    // The auth context is also attached (the viewer needs it once unlocked,
    // and downstream handlers shouldn't have to re-resolve the token); the
    // gate marker is the additional signal, not a replacement.
    assert_eq!(resp.headers().get("x-needs-gate").unwrap(), "1");
    assert_eq!(resp.headers().get("x-has-context").unwrap(), "1");
    // The same gate state is mirrored onto the context's field so handlers
    // can fail-closed off the context alone without inspecting extensions.
    assert_eq!(resp.headers().get("x-ctx-gate-required").unwrap(), "1");
}

#[tokio::test]
async fn browser_with_password_valid_cookie_attaches_context() {
    let t = tok('C');
    let lookup =
        Arc::new(StubLookup::new().insert(&t, row_with_password(303, "carol", "/X", 1, "pw1")));
    let state = make_state(lookup);
    let app = router_for(state, AuthSurface::Browser);

    let exp = Utc::now().timestamp() + 3600;
    let cookie_value = UnlockCookie::sign(SECRET, &t, exp);
    let cookie_name = UnlockCookie::cookie_name_for(&t);

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/s/{t}"))
                .header(header::COOKIE, format!("{cookie_name}={cookie_value}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers().get("x-has-context").unwrap(), "1");
    assert_eq!(resp.headers().get("x-needs-gate").unwrap(), "0");
    // Valid cookie → gate is satisfied; context's flag must be false so
    // downstream handlers know they can act on this context.
    assert_eq!(resp.headers().get("x-ctx-gate-required").unwrap(), "0");
}

#[tokio::test]
async fn browser_unknown_token_returns_404() {
    let lookup = Arc::new(StubLookup::new());
    let state = make_state(lookup);
    let app = router_for(state, AuthSurface::Browser);

    let t = tok('D');
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/s/{t}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn browser_expired_token_returns_404() {
    let t = tok('E');
    let mut r = row(404, "dan", "/Old", 1);
    r.expiration = Some(Utc::now() - Duration::hours(1));
    let lookup = Arc::new(StubLookup::new().insert(&t, r));
    let state = make_state(lookup);
    let app = router_for(state, AuthSurface::Browser);

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/s/{t}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// --- DAV surface tests -------------------------------------------------------

#[tokio::test]
async fn dav_with_password_no_basic_returns_401_with_challenge() {
    let t = tok('F');
    let lookup =
        Arc::new(StubLookup::new().insert(&t, row_with_password(505, "eve", "/F", 1, "pw1")));
    let state = make_state(lookup);
    let app = router_for(state, AuthSurface::Dav);

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/public.php/dav/files/{t}/foo"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let www = resp.headers().get(header::WWW_AUTHENTICATE).unwrap();
    assert_eq!(www.to_str().unwrap(), "Basic realm=\"public-link\"");
}

#[tokio::test]
async fn dav_with_wrong_basic_password_returns_401() {
    let t = tok('G');
    let lookup =
        Arc::new(StubLookup::new().insert(&t, row_with_password(606, "fay", "/G", 1, "pw1")));
    let state = make_state(lookup);
    let app = router_for(state, AuthSurface::Dav);

    let creds = base64::engine::general_purpose::STANDARD.encode("anonymous:wrong");
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/public.php/dav/files/{t}/foo"))
                .header(header::AUTHORIZATION, format!("Basic {creds}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn dav_with_correct_basic_attaches_context() {
    let t = tok('H');
    let lookup =
        Arc::new(StubLookup::new().insert(&t, row_with_password(707, "gus", "/H", 1, "pw1")));
    let state = make_state(lookup);
    let app = router_for(state, AuthSurface::Dav);

    let creds = base64::engine::general_purpose::STANDARD.encode("anonymous:pw1");
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/public.php/dav/files/{t}/foo"))
                .header(header::AUTHORIZATION, format!("Basic {creds}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers().get("x-has-context").unwrap(), "1");
    assert_eq!(resp.headers().get("x-share-id").unwrap(), "707");
    // DAV never carries the gate flag — DAV 401s on missing/wrong
    // credentials before context construction, so when the context exists
    // on the DAV surface the flag is unconditionally false.
    assert_eq!(resp.headers().get("x-ctx-gate-required").unwrap(), "0");
}

#[tokio::test]
async fn dav_eleventh_wrong_password_is_throttled() {
    // Per-test fresh state isolates the per-token rate limiter.
    let t = tok('I');
    let lookup =
        Arc::new(StubLookup::new().insert(&t, row_with_password(808, "hank", "/I", 1, "pw1")));
    let state = make_state(lookup);
    let creds = base64::engine::general_purpose::STANDARD.encode("anonymous:wrong");

    // First 10 attempts: allowed → 401.
    for i in 0..10 {
        // Cloning the Router per call gives us a fresh service; the state
        // (including the rate limiter) is shared by Arc.
        let app = router_for(state.clone(), AuthSurface::Dav);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/public.php/dav/files/{t}/foo"))
                    .header(header::AUTHORIZATION, format!("Basic {creds}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "attempt {i} should be 401"
        );
    }

    // 11th attempt: throttled.
    let app = router_for(state, AuthSurface::Dav);
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/public.php/dav/files/{t}/foo"))
                .header(header::AUTHORIZATION, format!("Basic {creds}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(resp.headers().get(header::RETRY_AFTER).is_some());
}
