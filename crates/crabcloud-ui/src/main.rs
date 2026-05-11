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
        // Hydration (`Config::new().hydrate(true)`) needs a base64 hydration
        // blob in `window.initial_dioxus_hydration_data` produced by dx 0.7's
        // *fullstack* server pipeline (serialized signal/suspense state). Our
        // manual SSR via `dioxus_ssr::pre_render` doesn't emit that blob, so
        // calling `atob(undefined)` would JS-panic the WASM the moment the
        // bundle ran — which is why post-Dioxus-0.6→0.7 the page froze with
        // `data-hydrated="false"`. Fall back to fresh client-side mount: the
        // SSR DOM is briefly visible, then `dioxus_web` rebuilds the same
        // tree on top of `#main`, and `App`'s `use_effect` flips the marker.
        // Migrating to the fullstack feature (needed for true hydration +
        // streaming) is tracked separately.
        dioxus_web::launch::launch(
            AppRoot,
            Vec::new(),
            vec![Box::new(dioxus_web::Config::new().hydrate(false))],
        );
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
