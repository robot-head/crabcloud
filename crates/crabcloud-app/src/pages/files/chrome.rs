//! Page chrome: top bar (logo, app name, user chip) + left sidebar
//! (currently "All files", "Shared with you", and "Deleted files").
//! See spec §2 (decision 7) and SP12 (trash bin).

use crate::context::RequestContext;
use dioxus::prelude::*;

#[component]
pub fn TopBar(ctx: RequestContext) -> Element {
    let display = ctx
        .display_name
        .clone()
        .unwrap_or_else(|| "User".to_string());
    let initial = display
        .chars()
        .next()
        .unwrap_or('U')
        .to_uppercase()
        .to_string();
    rsx! {
        header { class: "topbar",
            a { class: "topbar-brand", href: "/", "Crabcloud" }
            nav { class: "topbar-nav",
                a { class: "topbar-link active", href: "/apps/files/", "Files" }
            }
            div { class: "topbar-spacer" }
            div { class: "topbar-user", title: "{display}", "{initial}" }
        }
    }
}

#[component]
pub fn Sidebar() -> Element {
    // "Shared with you" chip — enabled only when the caller has at
    // least one accepted incoming share. The chip itself navigates to
    // `/apps/files/` (the recipient view is the root of their home in
    // SP7; the share-mount shows up as an ordinary folder there). When
    // disabled it stays in place greyed-out so the layout doesn't shift
    // once a share lands.
    let incoming = use_resource(move || async { crate::server_fns::count_incoming_shares().await });
    let enabled = matches!(incoming.read().as_ref(), Some(Ok(n)) if *n > 0);
    let nav = use_navigator();
    rsx! {
        aside { class: "sidebar",
            ul { class: "sidebar-list",
                li { class: "sidebar-item active",
                    span { class: "sidebar-icon", "📂" }
                    span { "All files" }
                }
                li {
                    class: if enabled { "sidebar-item sidebar-link" } else { "sidebar-item sidebar-link-disabled" },
                    onclick: move |_| {
                        if enabled {
                            nav.push("/apps/files/");
                        }
                    },
                    span { class: "sidebar-icon", "🌐" }
                    span { "Shared with you" }
                }
                li {
                    class: "sidebar-item sidebar-link",
                    onclick: move |_| {
                        nav.push("/trash");
                    },
                    span { class: "sidebar-icon", "🗑" }
                    span { "Deleted files" }
                }
            }
        }
    }
}
