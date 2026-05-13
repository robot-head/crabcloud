//! Toolbar: New / Upload buttons plus chips for current selection and the
//! cut-clipboard. Chips are compact and live in the same row as the
//! buttons (spec §11 / decision 11).

use dioxus::prelude::*;

#[derive(Props, Clone, PartialEq)]
pub struct ToolbarProps {
    pub selection_count: usize,
    pub clipboard_count: usize,
    pub clipboard_source: Option<String>,
    pub can_paste: bool,
    pub on_new_folder: EventHandler<()>,
    pub on_upload: EventHandler<()>,
    pub on_cut: EventHandler<()>,
    pub on_delete_selection: EventHandler<()>,
    pub on_clear_selection: EventHandler<()>,
    pub on_paste: EventHandler<()>,
    pub on_clear_clipboard: EventHandler<()>,
}

#[component]
pub fn Toolbar(props: ToolbarProps) -> Element {
    let ToolbarProps {
        selection_count,
        clipboard_count,
        clipboard_source,
        can_paste,
        on_new_folder,
        on_upload,
        on_cut,
        on_delete_selection,
        on_clear_selection,
        on_paste,
        on_clear_clipboard,
    } = props;

    let clipboard_label = match &clipboard_source {
        Some(src) => format!("✂ {clipboard_count} on clipboard from {src}"),
        None => format!("✂ {clipboard_count} on clipboard"),
    };

    rsx! {
        div { class: "files-toolbar",
            button {
                class: "files-tb-btn files-tb-primary",
                onclick: move |_| on_new_folder.call(()),
                "+ New folder"
            }
            button {
                class: "files-tb-btn",
                onclick: move |_| on_upload.call(()),
                "⬆ Upload"
            }
            if selection_count > 0 {
                div { class: "files-chip files-chip-selection",
                    span { "{selection_count} selected" }
                    button {
                        class: "files-chip-action",
                        onclick: move |_| on_cut.call(()),
                        "✂ Cut"
                    }
                    button {
                        class: "files-chip-action files-chip-danger",
                        onclick: move |_| on_delete_selection.call(()),
                        "🗑 Delete"
                    }
                    button {
                        class: "files-chip-close",
                        onclick: move |_| on_clear_selection.call(()),
                        "✕"
                    }
                }
            }
            if clipboard_count > 0 {
                div { class: "files-chip files-chip-clipboard",
                    span { "{clipboard_label}" }
                    button {
                        class: "files-chip-action",
                        disabled: !can_paste,
                        onclick: move |_| on_paste.call(()),
                        "Paste here"
                    }
                    button {
                        class: "files-chip-close",
                        onclick: move |_| on_clear_clipboard.call(()),
                        "✕"
                    }
                }
            }
        }
    }
}
