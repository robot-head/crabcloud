//! Server-only helpers for the Files page. The SSR branch checks the
//! session and, if absent, commits a 303 + Location header so the browser
//! redirects to the login page before the page body is sent.

#![cfg(feature = "server")]

use dioxus::fullstack::FullstackContext;

/// If the current request is unauthenticated, commit a 303 redirect to
/// `/login?redirect_url=<encoded current path>` and return `true` so the
/// caller can short-circuit page rendering.
///
/// The redirect target is the `/login` UI route, not the `/index.php/login`
/// server function — the latter is POST-only (it processes credentials) and
/// would return 405 on the GET that a browser does when following a 303.
///
/// `current_path` is the user-facing absolute path the user requested,
/// e.g. `/apps/files/photos/vacation`.
///
/// In Dioxus 0.7 the response is mutated through two entry points on
/// `FullstackContext`: `add_response_header` (instance method) for headers
/// and the static `commit_http_status` for the status code. The `Location`
/// header is what makes the 303 actually redirect the browser.
pub fn redirect_if_anonymous(user_id: &Option<String>, current_path: &str) -> bool {
    if user_id.is_some() {
        return false;
    }
    let Some(fs) = FullstackContext::current() else {
        return false;
    };
    let encoded = url_encode(current_path);
    let location = format!("/login?redirect_url={encoded}");
    let header_value: axum::http::HeaderValue = match location.parse() {
        Ok(v) => v,
        // `location` is built from a path we url-encoded ourselves, so the
        // only way this fails is a programmer error in `url_encode`. Bail
        // out without committing rather than panic during SSR.
        Err(_) => return false,
    };
    fs.add_response_header(axum::http::header::LOCATION, header_value);
    FullstackContext::commit_http_status(axum::http::StatusCode::SEE_OTHER, None);
    true
}

/// URL-encode using application/x-www-form-urlencoded rules. We avoid the
/// `url` crate dep for one helper — the input is a path we built ourselves
/// (no NULs, no `?`, no `#`).
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_encode_passthrough_safe() {
        assert_eq!(url_encode("/apps/files/photos"), "/apps/files/photos");
    }

    #[test]
    fn url_encode_escapes_space_and_question() {
        assert_eq!(url_encode("/a b?c"), "/a%20b%3Fc");
    }

    #[test]
    fn url_encode_escapes_unicode() {
        assert_eq!(url_encode("/é"), "/%C3%A9");
    }
}
