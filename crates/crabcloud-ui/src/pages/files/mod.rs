//! Files web UI — `/apps/files/<path>`. The browser-facing app for browsing,
//! reading, and writing the user's home storage. See
//! `docs/superpowers/specs/2026-05-12-files-web-ui-design.md`.

pub mod breadcrumb;
pub mod chrome;
pub mod list;
pub mod path;
pub mod row;
pub mod states;

#[cfg(feature = "server")]
pub mod ssr;

use crate::context::RequestContext;
use crate::pages::files::breadcrumb::Breadcrumb;
use crate::pages::files::list::FileList;
use crate::server_fns::list_dir;
use dioxus::prelude::*;

#[component]
pub fn Files(ctx: RequestContext, path: String) -> Element {
    // SSR-only: redirect anonymous visitors to login with redirect_url.
    #[cfg(feature = "server")]
    {
        let current_path = format!(
            "/apps/files{}",
            if path == "/" {
                String::new()
            } else {
                path.clone()
            }
        );
        if ssr::redirect_if_anonymous(&ctx.user_id, &current_path) {
            return rsx! { "" };
        }
    }

    let user_id = ctx.user_id.clone().unwrap_or_default();
    let mut path_sig = use_signal(|| path.clone());
    let mut refresh = use_signal(|| 0u64);

    // Keep the route's `path` prop in sync with the signal. Back/forward
    // navigation re-renders this component with a new `path` prop; the
    // `use_reactive` adapter (Dioxus 0.7.9) re-fires the effect when the
    // captured non-reactive prop value changes.
    use_effect(use_reactive((&path,), move |(p,)| {
        path_sig.set(p);
    }));

    // The list_dir server fn is only invoked from the client (wasm) side.
    // SSR renders the chrome + a pending skeleton; the resource starts in
    // the `Pending` state and is populated after hydration.
    let entries = use_resource(move || {
        let p = path_sig();
        let _ = refresh();
        async move { list_dir(p).await.map_err(|e| format!("{e}")) }
    });

    // Both Breadcrumb and FileList want to push a Files route for a target
    // path. `Navigator` is `Copy`, so we hand a fresh closure to each prop
    // instead of trying to alias one closure between two `EventHandler`
    // call-sites (which would move-once).
    let nav = use_navigator();
    let on_open_folder = move |target: String| {
        let segs = path::path_to_segments(&target);
        nav.push(crate::Route::FilesRoute { segments: segs });
    };
    let on_navigate_breadcrumb = move |target: String| {
        let segs = path::path_to_segments(&target);
        nav.push(crate::Route::FilesRoute { segments: segs });
    };
    let on_retry = move |_| refresh.set(refresh() + 1);

    let entries_view = entries.read().clone();

    rsx! {
        div { class: "files-page",
            chrome::TopBar { ctx: ctx.clone() }
            div { class: "files-body",
                chrome::Sidebar {}
                main { class: "files-main",
                    Breadcrumb {
                        path: path_sig(),
                        on_navigate: on_navigate_breadcrumb,
                    }
                    FileList {
                        entries: entries_view,
                        user_id: user_id,
                        on_open_folder,
                        on_retry,
                    }
                }
            }
        }
    }
}
