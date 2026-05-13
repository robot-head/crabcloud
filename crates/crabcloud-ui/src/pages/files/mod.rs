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

    let _ = path;
    rsx! {
        div { class: "files-page",
            chrome::TopBar { ctx: ctx.clone() }
            div { class: "files-body",
                chrome::Sidebar {}
                main { class: "files-main",
                    states::EmptyFolder {}
                }
            }
        }
    }
}
