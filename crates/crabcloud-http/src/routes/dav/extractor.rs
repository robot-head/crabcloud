//! Extract `(uid, user_path)` from a DAV request. The URL is shaped
//! `/files/{user}/{*path}` where `path` may be empty (root). The
//! authenticated user MUST match `{user}` — cross-user access lives in
//! the sharing sub-project.

use crate::extractors::auth::AuthenticatedUser;
use crate::routes::dav::error::{DavError, DavResult};
use crabcloud_fs::UserPath;
use crabcloud_users::UserId;

/// Validate that `url_user` matches `authed.user_id` and produce a `(UserId, UserPath)`.
/// `url_path` is the captured wildcard segment (may be empty).
pub fn resolve_target(
    authed: &AuthenticatedUser,
    url_user: &str,
    url_path: &str,
) -> DavResult<(UserId, UserPath)> {
    if url_user != authed.user_id {
        return Err(DavError::Forbidden);
    }
    let uid =
        UserId::new(url_user).map_err(|e| DavError::BadRequest(format!("invalid user id: {e}")))?;
    // `url_path` is the captured rest after `/files/{user}/`. The leading `/`
    // is already consumed by axum's path-template; prepend it for `UserPath`.
    let user_path = if url_path.is_empty() {
        UserPath::root()
    } else {
        // URL-decode in case the client percent-encoded segments.
        let decoded = urlencoding::decode(url_path)
            .map_err(|e| DavError::BadRequest(format!("invalid url encoding: {e}")))?;
        UserPath::new(format!("/{decoded}"))
            .map_err(|e| DavError::BadRequest(format!("invalid path: {e}")))?
    };
    Ok((uid, user_path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth_context::AuthMethod;

    fn authed(uid: &str) -> AuthenticatedUser {
        AuthenticatedUser {
            user_id: uid.to_string(),
            auth_method: AuthMethod::Session,
        }
    }

    #[test]
    fn resolves_root_for_empty_path() {
        let (uid, p) = resolve_target(&authed("alice"), "alice", "").unwrap();
        assert_eq!(uid.as_str(), "alice");
        assert!(p.is_root());
    }

    #[test]
    fn resolves_nested_path() {
        let (uid, p) = resolve_target(&authed("alice"), "alice", "photos/cat.jpg").unwrap();
        assert_eq!(uid.as_str(), "alice");
        assert_eq!(p.as_str(), "/photos/cat.jpg");
    }

    #[test]
    fn decodes_percent_escapes() {
        let (_uid, p) = resolve_target(&authed("alice"), "alice", "hello%20world.txt").unwrap();
        assert_eq!(p.as_str(), "/hello world.txt");
    }

    #[test]
    fn cross_user_returns_forbidden() {
        assert!(matches!(
            resolve_target(&authed("alice"), "bob", ""),
            Err(DavError::Forbidden)
        ));
    }

    #[test]
    fn dotdot_segment_rejected() {
        assert!(matches!(
            resolve_target(&authed("alice"), "alice", "a/../b"),
            Err(DavError::BadRequest(_))
        ));
    }
}
