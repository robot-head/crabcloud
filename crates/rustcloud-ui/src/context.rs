//! Per-request context carried into Dioxus SSR rendering and emitted as the
//! hydration payload for the browser to pick up.
//!
//! See spec §8.2.

use serde::{Deserialize, Serialize};

/// Per-request data threaded through SSR rendering and re-hydrated on the
/// client. JSON-serialized into the HTML payload for the WASM bundle to pick up.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequestContext {
    /// Authenticated user ID, or `None` for anonymous requests.
    pub user_id: Option<String>,
    /// Display name (Phase 4 simplification: same as `user_id`; the real users
    /// sub-project will resolve a proper display name from the user store).
    pub display_name: Option<String>,
    /// Resolved locale tag — e.g. "en", "de", "fr_FR".
    pub locale: String,
    /// CSRF request token from the session, exposed to the browser so
    /// authenticated XHR can include it in the `requesttoken` header.
    pub request_token: String,
    /// Cached `cloud/capabilities` ETag. Phase 4 ships `None`; Phase 5+ can
    /// surface it for clients that want conditional capability refresh.
    pub capabilities_etag: Option<String>,
}

impl RequestContext {
    /// Build a context for an unauthenticated request.
    pub fn anonymous(locale: impl Into<String>, request_token: impl Into<String>) -> Self {
        Self {
            user_id: None,
            display_name: None,
            locale: locale.into(),
            request_token: request_token.into(),
            capabilities_etag: None,
        }
    }

    /// Build a context for an authenticated request. `display_name` is set to
    /// `user_id` as a Phase 4 simplification.
    pub fn authenticated(
        user_id: impl Into<String>,
        locale: impl Into<String>,
        request_token: impl Into<String>,
    ) -> Self {
        let uid = user_id.into();
        Self {
            user_id: Some(uid.clone()),
            display_name: Some(uid),
            locale: locale.into(),
            request_token: request_token.into(),
            capabilities_etag: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anonymous_has_no_user() {
        let ctx = RequestContext::anonymous("en", "tok-123");
        assert!(ctx.user_id.is_none());
        assert!(ctx.display_name.is_none());
        assert_eq!(ctx.locale, "en");
        assert_eq!(ctx.request_token, "tok-123");
    }

    #[test]
    fn authenticated_populates_user_and_display_name() {
        let ctx = RequestContext::authenticated("alice", "de", "tok-456");
        assert_eq!(ctx.user_id.as_deref(), Some("alice"));
        assert_eq!(ctx.display_name.as_deref(), Some("alice"));
    }

    #[test]
    fn round_trips_via_json() {
        let ctx = RequestContext::authenticated("alice", "en", "tok-789");
        let s = serde_json::to_string(&ctx).unwrap();
        let back: RequestContext = serde_json::from_str(&s).unwrap();
        assert_eq!(ctx, back);
    }
}
