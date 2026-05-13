//! Files web UI — `/apps/files/<path>`. The browser-facing app for browsing,
//! reading, and writing the user's home storage. See
//! `docs/superpowers/specs/2026-05-12-files-web-ui-design.md`.

pub mod path;

pub mod chrome;

pub mod states;

#[cfg(feature = "server")]
pub mod ssr;

use crate::context::RequestContext;
use dioxus::prelude::*;

/// Files page entry point. For Batch A this renders a placeholder list so
/// the route is wired end-to-end; Batch B replaces the body with real data.
#[component]
pub fn Files(ctx: RequestContext, path: String) -> Element {
    let _ = (ctx, path);
    rsx! {
        main { class: "files-page",
            p { "Files (placeholder — batch A)" }
        }
    }
}
