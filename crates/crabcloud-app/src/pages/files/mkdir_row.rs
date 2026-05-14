//! Inline "New folder" row. Appears at the top of the file list while the
//! user is creating a directory. Enter commits, Escape cancels.

use dioxus::prelude::*;

#[component]
pub fn MkdirRow(on_commit: EventHandler<String>, on_cancel: EventHandler<()>) -> Element {
    let mut name = use_signal(|| "New folder".to_string());

    let on_keydown = move |evt: KeyboardEvent| match evt.key() {
        Key::Enter => {
            evt.prevent_default();
            let v = name().trim().to_string();
            if !v.is_empty() {
                on_commit.call(v);
            }
        }
        Key::Escape => {
            evt.prevent_default();
            on_cancel.call(());
        }
        _ => {}
    };

    rsx! {
        tr { class: "files-row files-row-mkdir",
            td { class: "files-cell files-check" }
            td { class: "files-cell",
                span { class: "files-icon", "📁" }
                input {
                    class: "files-mkdir-input",
                    value: "{name}",
                    autofocus: true,
                    oninput: move |e| name.set(e.value()),
                    onkeydown: on_keydown,
                    onblur: move |_| {
                        let v = name().trim().to_string();
                        if v.is_empty() {
                            on_cancel.call(());
                        } else {
                            on_commit.call(v);
                        }
                    },
                }
            }
            td { class: "files-cell files-size", "—" }
            td { class: "files-cell files-mtime", "just now" }
        }
    }
}
