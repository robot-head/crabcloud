//! SSR rendering helpers: HTML shell head, app body render, and HTML escape.
//!
//! The actual axum handler lives in `rustcloud-http` (it depends on extractors
//! defined there — `OptionalUser` / `SessionHandle`). Keeping these helpers
//! here means `rustcloud-ui` has no cyclic dependency on `rustcloud-http`.

use crate::app::Route;
use crate::context::RequestContext;
use crate::hydration::render_hydration_script;
use dioxus::prelude::*;
use dioxus_history::{History, MemoryHistory};
use dioxus_router::prelude::Router;
use std::rc::Rc;

/// Standard `<!DOCTYPE html>` prefix the handler prepends to the SSR document.
pub const HTML_DOCTYPE: &str = "<!DOCTYPE html>\n";

/// Render the `<head>` fragment for the SSR shell: meta tags, CSS link, CSRF
/// `requesttoken` meta, the `__dx_ctx` hydration script, and the WASM module
/// script tag.
pub fn render_head_html(ctx: &RequestContext) -> String {
    let mut out = String::new();
    out.push_str("<meta charset=\"utf-8\">");
    out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">");
    out.push_str("<title>Rustcloud</title>");
    out.push_str("<link rel=\"stylesheet\" href=\"/assets/app.css\">");
    out.push_str(&format!(
        "<meta name=\"requesttoken\" content=\"{}\">",
        html_escape(&ctx.request_token)
    ));
    out.push_str(&render_hydration_script(ctx));
    // The WASM client bundle. dx places it at /assets/dioxus/<name>.js by
    // default; we mount the assets root at /assets/ so this path resolves
    // to target/dx/.../public/dioxus/<name>.js.
    out.push_str("<script type=\"module\" src=\"/assets/dioxus/rustcloud-ui.js\" defer></script>");
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
    dioxus_ssr::render(&vdom)
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
    rsx! { Router::<Route> {} }
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
