//! Static asset serving. Release builds embed the `dx`-produced `public/`
//! directory; debug builds read from disk so contributors don't have to
//! `dx build` after every UI tweak.
//!
//! The handler is a plain `(Path) -> Response` axum function — it has no
//! dependency on `crabcloud-http` extractors. It is mounted by
//! `crabcloud-http::router::build_router` at `/assets/{*path}`.

use axum::body::Body;
use axum::extract::Path;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../../target/dx/crabcloud-ui/release/web/public"]
#[exclude = "*.map"]
struct Assets;

/// Axum handler mounted at `/assets/{*path}` that serves the embedded UI bundle.
/// Returns 404 for unknown paths and applies long cache headers to hashed assets.
pub async fn handler(Path(path): Path<String>) -> Response {
    let file = match Assets::get(&path) {
        Some(f) => f,
        None => return (StatusCode::NOT_FOUND, "asset not found").into_response(),
    };
    let mime = mime_for(&path);
    let mut resp = Response::new(Body::from(file.data.into_owned()));
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime).unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    // Long-cache hashed assets (Dioxus names them with content hashes).
    if path.ends_with(".wasm") || path.starts_with("dioxus/") || path.contains("/dioxus/") {
        resp.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        );
    }
    resp
}

fn mime_for(path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if lower.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if lower.ends_with(".js") {
        "application/javascript; charset=utf-8"
    } else if lower.ends_with(".wasm") {
        "application/wasm"
    } else if lower.ends_with(".json") {
        "application/json; charset=utf-8"
    } else if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else if lower.ends_with(".svg") {
        "image/svg+xml; charset=utf-8"
    } else if lower.ends_with(".ico") {
        "image/x-icon"
    } else if lower.ends_with(".woff2") {
        "font/woff2"
    } else if lower.ends_with(".woff") {
        "font/woff"
    } else {
        "application/octet-stream"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_matches_known_extensions() {
        assert!(mime_for("/dioxus/crabcloud-ui.js").starts_with("application/javascript"));
        assert!(mime_for("/dioxus/crabcloud-ui_bg.wasm").starts_with("application/wasm"));
        assert!(mime_for("assets/app.css").starts_with("text/css"));
        assert_eq!(mime_for("favicon.ico"), "image/x-icon");
        assert_eq!(mime_for("missing.weirdext"), "application/octet-stream");
    }
}
