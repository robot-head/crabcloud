//! WASM browser entry point. `dx build` compiles this against
//! `wasm32-unknown-unknown` and emits the hydration bundle.

// On non-wasm targets this binary collapses to a single eprintln! so the
// `[dependencies]` block looks fully unused — they're consumed only by the
// `#[cfg(target_arch = "wasm32")]` branch above.
#![allow(unused_crate_dependencies)]

#[cfg(target_arch = "wasm32")]
mod web {
    use crabcloud_ui::{App, RequestContext};
    use dioxus::prelude::*;

    /// Top-level component. Reads the hydration payload from the DOM on first
    /// render, installs it as the request context, then mounts the **same**
    /// `App` component the SSR side rendered.
    ///
    /// Hydration requires the WASM tree to match the SSR tree element-for-
    /// element. SSR renders `<App />` (which wraps `Router::<Route>` inside
    /// a `<div id="app-root" data-hydrated="...">` host with a use_effect
    /// that flips the marker post-mount). If this entry point mounted
    /// `<Router>` directly, the WASM tree would skip that wrapper div and
    /// Dioxus would refuse to hydrate — leaving `data-hydrated="false"`
    /// frozen in the DOM (which is exactly what the Playwright suite watches
    /// for as the hydration signal).
    #[component]
    pub fn AppRoot() -> Element {
        let ctx = use_hook(|| {
            read_hydration_context().unwrap_or_else(|| RequestContext::anonymous("en", ""))
        });
        use_context_provider(|| ctx.clone());
        rsx! { App {} }
    }

    pub fn launch() {
        // We deliberately don't enable `dioxus-web/hydrate` (see Cargo.toml).
        // `Config::new()` defaults to `hydrate = false`, so `dioxus_web` takes
        // the plain client-render branch and renders `<App />` into `#main`.
        //
        // `dioxus_web` *appends* the rebuilt tree to its root element rather
        // than replacing it, so without intervention the SSR'd content and
        // the client-rendered content stack — `#app-root` ends up duplicated.
        // Wipe `#main` before launching to make the client mount the
        // canonical tree. The cost is a brief CSR flash; SEO + first-paint
        // benefits of SSR are preserved because the response body is still
        // fully pre-rendered for crawlers and slow connections.
        clear_main();
        dioxus_web::launch::launch(
            AppRoot,
            Vec::new(),
            vec![Box::new(dioxus_web::Config::new())],
        );
    }

    fn clear_main() {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Some(document) = window.document() else {
            return;
        };
        if let Some(main) = document.get_element_by_id("main") {
            main.set_inner_html("");
        }
    }

    fn read_hydration_context() -> Option<RequestContext> {
        let window = web_sys::window()?;
        let document = window.document()?;
        let el = document.get_element_by_id("__dx_ctx")?;
        let json = el.text_content()?;
        serde_json::from_str(&json).ok()
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {
    console_error_panic_hook::set_once();
    web::launch();
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("crabcloud-ui-web is a WASM-only entry point");
}
