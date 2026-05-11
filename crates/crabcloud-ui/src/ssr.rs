//! SSR rendering helpers: HTML shell head, app body render, and HTML escape.
//!
//! The actual axum handler lives in `crabcloud-http` (it depends on extractors
//! defined there — `OptionalUser` / `SessionHandle`). Keeping these helpers
//! here means `crabcloud-ui` has no cyclic dependency on `crabcloud-http`.

use crate::app::App;
use crate::context::RequestContext;
use crate::hydration::render_hydration_script;
use dioxus::prelude::*;
use dioxus_history::{History, MemoryHistory};
use std::rc::Rc;

/// Standard `<!DOCTYPE html>` prefix the handler prepends to the SSR document.
pub const HTML_DOCTYPE: &str = "<!DOCTYPE html>\n";

/// The `<script>` tag dx 0.7 injects into its generated `index.html` to load
/// the WASM bundle, extracted at build time by `build.rs`. dx 0.7 hashes the
/// bundle filename in release mode, so a hard-coded path no longer works.
/// Empty if `dx build --release --platform web` hasn't been run yet (e.g.
/// fresh checkout running unit tests) — SSR handlers still produce valid
/// HTML in that case, just without client hydration.
const WASM_SCRIPT_TAG: &str = include_str!(concat!(env!("OUT_DIR"), "/wasm_script_tag.txt"));

/// Render the `<head>` fragment for the SSR shell: meta tags, CSS link, CSRF
/// `requesttoken` meta, the `__dx_ctx` hydration script, and the WASM module
/// script tag.
pub fn render_head_html(ctx: &RequestContext) -> String {
    let mut out = String::new();
    out.push_str("<meta charset=\"utf-8\">");
    out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">");
    out.push_str("<title>Crabcloud</title>");
    out.push_str("<link rel=\"stylesheet\" href=\"/assets/app.css\">");
    out.push_str(&format!(
        "<meta name=\"requesttoken\" content=\"{}\">",
        html_escape(&ctx.request_token)
    ));
    out.push_str(&render_hydration_script(ctx));
    // dx 0.7's hashed-bundle script tag, spliced in verbatim from index.html.
    // dx bakes the `/assets/` server base path into the `src` URL, which
    // matches where the asset handler is mounted.
    out.push_str(WASM_SCRIPT_TAG);
    out
}

/// Render the Dioxus `App` to HTML for `path`, with `ctx` installed in the
/// component context. Seeds the router's history via `MemoryHistory` so the
/// SSR output corresponds to the requested URL. The browser router will
/// reinitialize from `window.location` on hydration.
pub fn render_app_html(ctx: RequestContext, path: &str) -> String {
    let mut vdom = VirtualDom::new_with_props(
        AppWithContext,
        AppWithContextProps {
            ctx,
            initial_path: path.to_string(),
        },
    );
    vdom.rebuild_in_place();
    // `pre_render` (vs plain `render`) is what emits `data-node-hydration`
    // markers on every element. The WASM client's `dioxus_web` runtime reads
    // them on launch to map the existing DOM tree back to its virtual-DOM
    // nodes; without them, hydration silently no-ops and `use_effect`s never
    // fire — so e.g. our `App` component's `data-hydrated` signal stays
    // stuck at `"false"`. This is the dx 0.7 split that bit us after the
    // Dioxus 0.6 → 0.7 upgrade.
    dioxus_ssr::pre_render(&vdom)
}

#[component]
fn AppWithContext(ctx: RequestContext, initial_path: String) -> Element {
    use_context_provider(|| ctx.clone());
    // Seed the router's history with the SSR-requested path. Dioxus 0.6's
    // router resolves the active route via `dioxus_history::history()` which
    // reads `Rc<dyn History>` from the component context.
    use_hook(|| {
        let history: Rc<dyn History> =
            Rc::new(MemoryHistory::with_initial_path(initial_path.clone()));
        provide_context(history);
    });
    rsx! { App {} }
}

/// Minimal HTML attribute/text escape. Used by the head renderer for the
/// `requesttoken` meta tag.
pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// Parse a request path into a `Route`. Falls back to `NotFoundRoute` so the
/// caller doesn't have to unwrap.
pub fn resolve_route(path: &str) -> crate::Route {
    use std::str::FromStr;
    crate::Route::from_str(path)
        .unwrap_or_else(|_| crate::Route::NotFoundRoute { segments: vec![] })
}

/// True if the resolved route is the catch-all 404. Used by the HTTP handler
/// to set the response status.
pub fn is_not_found(route: &crate::Route) -> bool {
    matches!(route, crate::Route::NotFoundRoute { .. })
}

#[cfg(test)]
mod resolve_tests {
    use super::*;

    #[test]
    fn home_path_resolves_to_home_route() {
        let r = resolve_route("/");
        assert!(!is_not_found(&r));
    }

    #[test]
    fn login_path_resolves() {
        let r = resolve_route("/login");
        assert!(!is_not_found(&r));
    }

    #[test]
    fn unknown_path_resolves_to_not_found() {
        let r = resolve_route("/nonexistent/path");
        assert!(is_not_found(&r));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_escape_replaces_special_chars() {
        assert_eq!(html_escape("a<b>&\"'c"), "a&lt;b&gt;&amp;&quot;&#39;c");
    }

    #[test]
    fn head_includes_csrf_meta_and_hydration_script() {
        let ctx = RequestContext::authenticated("alice", "en", "tok-1");
        let head = render_head_html(&ctx);
        assert!(head.contains("name=\"requesttoken\""));
        assert!(head.contains("content=\"tok-1\""));
        assert!(head.contains("<script id=\"__dx_ctx\""));
    }

    #[test]
    fn head_escapes_request_token() {
        let ctx = RequestContext::authenticated("alice", "en", "a<b>");
        let head = render_head_html(&ctx);
        assert!(head.contains("content=\"a&lt;b&gt;\""));
    }
}
