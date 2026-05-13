//! Centered modal that confirms a destructive delete. Lists the items (up
//! to 5 explicitly, then "and N more") and requires explicit click to
//! confirm.

use dioxus::prelude::*;

#[component]
pub fn DeleteModal(
    paths: Vec<String>,
    on_cancel: EventHandler<()>,
    on_confirm: EventHandler<()>,
) -> Element {
    let count = paths.len();
    let preview: Vec<String> = paths
        .iter()
        .take(5)
        .map(|p| p.rsplit('/').next().unwrap_or(p).to_string())
        .collect();
    let extra = count.saturating_sub(preview.len());
    let single_title = preview.first().cloned().unwrap_or_default();
    let body = preview.join(", ");
    rsx! {
        div { class: "files-modal-backdrop", onclick: move |_| on_cancel.call(()),
            div {
                class: "files-modal",
                onclick: move |e: MouseEvent| e.stop_propagation(),
                div { class: "files-modal-title",
                    if count == 1 { "Delete {single_title}?" } else { "Delete {count} items?" }
                }
                div { class: "files-modal-body",
                    "{body}"
                    if extra > 0 { " and {extra} more" }
                }
                div { class: "files-modal-actions",
                    button {
                        class: "files-modal-cancel",
                        onclick: move |_| on_cancel.call(()),
                        "Cancel"
                    }
                    button {
                        class: "files-modal-confirm",
                        onclick: move |_| on_confirm.call(()),
                        "Delete"
                    }
                }
            }
        }
    }
}
