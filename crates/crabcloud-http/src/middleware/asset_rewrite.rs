//! `AssetRewriteLayer` — rewrites the `href`/`src` of Dioxus `asset!()`-derived
//! attributes in SSR'd HTML.
//!
//! Background: when dx builds the wasm bundle it scans the binary and
//! substitutes each `asset!()` token in-place with the hashed bundled URL
//! (e.g. `/assets/app-dxh<hash>.css`). Our native server binary
//! (`crabcloud-server`) links the same `crabcloud-app` rlib but is built by
//! plain `cargo build`, so it never goes through that substitution pass.
//! The result: the SSR'd HTML carries the original absolute source path
//! (e.g. `C:\Users\…\crabcloud-app\assets\app.css`) as the `<link>` href,
//! which the browser can't load and CSP refuses to apply as a stylesheet.
//!
//! Fix: at startup we read dx's `.manifest.json` (sibling of the `public/`
//! dir it emits) which maps `absolute_source_path` → `bundled_path`. On
//! every `text/html` response we walk the body, replacing each source path
//! with `/assets/<bundled_path>`. If the manifest isn't present (dev
//! workflow, tests) the layer is a cheap pass-through.

use axum::body::{to_bytes, Body};
use axum::http::{header, HeaderValue, Request, Response};
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::task::{Context, Poll};
use tower::{Layer, Service};

/// Max bytes we'll buffer before rewriting. SSR'd HTML for the Files UI is
/// well under 100 KiB; this cap protects against the response body being
/// something pathological (a large download misclassified as html, say).
const MAX_REWRITE_BYTES: usize = 4 * 1024 * 1024;

/// Substitution table: `absolute_source_path` (key from the manifest) →
/// served URL like `/assets/app-dxh<hash>.css`. Shared by every cloned
/// service via `Arc`.
#[derive(Clone, Default)]
pub struct AssetRewriteMap {
    inner: Arc<HashMap<String, String>>,
}

impl AssetRewriteMap {
    /// Empty map — middleware is a no-op.
    pub fn empty() -> Self {
        Self::default()
    }

    /// True if there are no substitutions to apply (avoids the body buffer
    /// round-trip on every HTML response).
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Load the manifest dx emitted next to `public_path`'s parent and build
    /// the substitution table. Returns an empty map (with a warning) if the
    /// manifest is missing or malformed — the server should still start in
    /// dev contexts where dx wasn't invoked.
    pub fn from_public_path(public_path: &Path) -> Self {
        let Some(parent) = public_path.parent() else {
            return Self::empty();
        };
        let manifest_path = parent.join(".manifest.json");
        match std::fs::read_to_string(&manifest_path) {
            Ok(text) => match parse_manifest(&text) {
                Ok(map) => {
                    tracing::info!(
                        manifest = %manifest_path.display(),
                        entries = map.len(),
                        "loaded dx asset manifest"
                    );
                    Self {
                        inner: Arc::new(map),
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        manifest = %manifest_path.display(),
                        error = %e,
                        "failed to parse dx asset manifest; SSR asset hrefs may break"
                    );
                    Self::empty()
                }
            },
            Err(e) => {
                tracing::debug!(
                    manifest = %manifest_path.display(),
                    error = %e,
                    "no dx asset manifest found; running without href rewrites"
                );
                Self::empty()
            }
        }
    }

    /// Locate the manifest using the same precedence Dioxus uses for the
    /// public dir: `$DIOXUS_PUBLIC_PATH` if set, else `<exe_dir>/public`.
    pub fn from_env() -> Self {
        let public = std::env::var_os("DIOXUS_PUBLIC_PATH")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|d| d.join("public")))
            });
        match public {
            Some(p) => Self::from_public_path(&p),
            None => Self::empty(),
        }
    }
}

/// Parse the `assets` table from a dx `.manifest.json` and project it to the
/// `source_path → URL` shape the middleware applies. The manifest schema
/// (per dx 0.7) is `{ "assets": { "<src>": [{ "bundled_path": "...", ... }] } }`.
fn parse_manifest(text: &str) -> serde_json::Result<HashMap<String, String>> {
    let raw: ManifestRaw = serde_json::from_str(text)?;
    let mut out = HashMap::with_capacity(raw.assets.len());
    for (src, entries) in raw.assets {
        // Each source maps to a vec of build-output entries (always 1 in
        // practice, but the schema allows multiple); we use the first.
        if let Some(entry) = entries.into_iter().next() {
            out.insert(src, format!("/assets/{}", entry.bundled_path));
        }
    }
    Ok(out)
}

#[derive(serde::Deserialize)]
struct ManifestRaw {
    assets: HashMap<String, Vec<ManifestEntry>>,
}

#[derive(serde::Deserialize)]
struct ManifestEntry {
    bundled_path: String,
}

/// `tower::Layer` constructor.
#[derive(Clone)]
pub struct AssetRewriteLayer {
    map: AssetRewriteMap,
}

impl AssetRewriteLayer {
    pub fn new(map: AssetRewriteMap) -> Self {
        Self { map }
    }
}

impl<S> Layer<S> for AssetRewriteLayer {
    type Service = AssetRewrite<S>;
    fn layer(&self, inner: S) -> Self::Service {
        AssetRewrite {
            inner,
            map: self.map.clone(),
        }
    }
}

#[derive(Clone)]
pub struct AssetRewrite<S> {
    inner: S,
    map: AssetRewriteMap,
}

impl<S> Service<Request<Body>> for AssetRewrite<S>
where
    S: Service<Request<Body>, Response = Response<Body>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
{
    type Response = Response<Body>;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Response<Body>, S::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let mut inner = self.inner.clone();
        let map = self.map.clone();
        Box::pin(async move {
            let resp = inner.call(req).await?;
            if map.is_empty() || !is_html(resp.headers().get(header::CONTENT_TYPE)) {
                return Ok(resp);
            }
            Ok(rewrite_body(resp, &map.inner).await)
        })
    }
}

fn is_html(ct: Option<&HeaderValue>) -> bool {
    ct.and_then(|v| v.to_str().ok())
        .map(|s| s.starts_with("text/html"))
        .unwrap_or(false)
}

async fn rewrite_body(resp: Response<Body>, map: &HashMap<String, String>) -> Response<Body> {
    let (parts, body) = resp.into_parts();
    let bytes = match to_bytes(body, MAX_REWRITE_BYTES).await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(error = %e, "asset_rewrite: failed to buffer html body; passing through");
            return Response::from_parts(parts, Body::empty());
        }
    };

    // Substitute on the byte string. Source paths can be either Windows
    // (`C:\Users\…`) or POSIX (`/home/runner/…`) depending on the host
    // that built the binary, so we don't assume a separator — we just use
    // whatever strings the manifest gave us.
    let mut text = match String::from_utf8(bytes.to_vec()) {
        Ok(s) => s,
        Err(e) => {
            // Not UTF-8 — likely a misclassified binary response. Skip
            // rewriting and reconstruct an unmodified body.
            tracing::warn!(error = %e, "asset_rewrite: html body wasn't utf-8; passing through");
            return Response::from_parts(parts, Body::from(e.into_bytes()));
        }
    };

    for (src, url) in map {
        if text.contains(src) {
            text = text.replace(src, url);
        }
    }

    // Body length changed — strip a stale content-length so axum/hyper
    // recomputes it. (Most SSR responses use chunked encoding, but the
    // belt-and-braces avoids a future regression where one set it.)
    let mut parts = parts;
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, Body::from(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    fn html(body: &'static str) -> Response<Body> {
        let mut resp = Response::new(Body::from(body));
        resp.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        resp
    }

    #[tokio::test]
    async fn empty_map_passes_response_through_unchanged() {
        let app = Router::new()
            .route(
                "/",
                get(|| async { html("<link href=\"C:\\src\\app.css\">") }),
            )
            .layer(AssetRewriteLayer::new(AssetRewriteMap::empty()));
        let req = Request::builder().uri("/").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let bytes = to_bytes(resp.into_body(), MAX_REWRITE_BYTES).await.unwrap();
        assert_eq!(&bytes[..], b"<link href=\"C:\\src\\app.css\">");
    }

    #[tokio::test]
    async fn html_with_mapped_source_path_gets_rewritten() {
        let mut entries = HashMap::new();
        entries.insert(
            "C:\\src\\app.css".to_string(),
            "/assets/app-dxh1234.css".to_string(),
        );
        let map = AssetRewriteMap {
            inner: Arc::new(entries),
        };
        let app = Router::new()
            .route(
                "/",
                get(|| async { html("<link href=\"C:\\src\\app.css\">") }),
            )
            .layer(AssetRewriteLayer::new(map));
        let req = Request::builder().uri("/").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let bytes = to_bytes(resp.into_body(), MAX_REWRITE_BYTES).await.unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(text.contains("/assets/app-dxh1234.css"));
        assert!(!text.contains("C:\\src\\app.css"));
    }

    #[tokio::test]
    async fn non_html_response_skips_rewrite() {
        let mut entries = HashMap::new();
        entries.insert("/src/app.css".to_string(), "/assets/x.css".to_string());
        let map = AssetRewriteMap {
            inner: Arc::new(entries),
        };
        let app = Router::new()
            .route(
                "/api",
                get(|| async {
                    let mut r = Response::new(Body::from("{\"path\":\"/src/app.css\"}"));
                    r.headers_mut().insert(
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("application/json"),
                    );
                    r
                }),
            )
            .layer(AssetRewriteLayer::new(map));
        let req = Request::builder().uri("/api").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let bytes = to_bytes(resp.into_body(), MAX_REWRITE_BYTES).await.unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(text.contains("/src/app.css"));
        assert!(!text.contains("/assets/x.css"));
    }

    #[test]
    fn parse_manifest_extracts_bundled_paths() {
        let text = r#"{
            "assets": {
                "C:\\Users\\foo\\app.css": [
                    { "bundled_path": "app-dxh1234.css" }
                ],
                "/home/runner/main.js": [
                    { "bundled_path": "main-dxh5678.js" }
                ]
            }
        }"#;
        let map = parse_manifest(text).unwrap();
        assert_eq!(
            map.get("C:\\Users\\foo\\app.css"),
            Some(&"/assets/app-dxh1234.css".to_string())
        );
        assert_eq!(
            map.get("/home/runner/main.js"),
            Some(&"/assets/main-dxh5678.js".to_string())
        );
    }
}
