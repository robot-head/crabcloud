//! Empty / loading / error states for the Files page. See spec §13.

use dioxus::prelude::*;

#[component]
pub fn EmptyFolder() -> Element {
    rsx! {
        div { class: "files-empty",
            div { class: "files-empty-icon", "📂" }
            div { class: "files-empty-title", "This folder is empty" }
            div { class: "files-empty-sub", "Drop files here, or click ", strong { "Upload" }, " above." }
        }
    }
}

#[component]
pub fn LoadError(reason: String, on_retry: EventHandler<()>) -> Element {
    rsx! {
        div { class: "files-error",
            div { class: "files-error-icon", "⚠️" }
            div { class: "files-error-title", "Couldn't load this folder" }
            if !reason.is_empty() {
                div { class: "files-error-sub", "{reason}" }
            }
            button {
                class: "files-error-retry",
                onclick: move |_| on_retry.call(()),
                "Retry"
            }
        }
    }
}

#[component]
pub fn Skeleton() -> Element {
    rsx! {
        div { class: "files-skeleton",
            for _ in 0..4 {
                div { class: "files-skeleton-row",
                    span { class: "files-skel-cell files-skel-check" }
                    span { class: "files-skel-cell files-skel-name" }
                    span { class: "files-skel-cell files-skel-size" }
                    span { class: "files-skel-cell files-skel-mtime" }
                }
            }
        }
    }
}
