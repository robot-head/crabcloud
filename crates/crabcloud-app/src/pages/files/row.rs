//! Single row in the file list. Renders either the standard view or an
//! inline rename input when `rename_active == true`. ⋯ menu emits events
//! for rename/delete (cut is added in Batch D).

use crate::pages::preview_mime::is_previewable_mime;
use crate::server_fns::FileEntry;
use dioxus::prelude::*;

/// Inline JS swapped in via the thumbnail `<img>`'s `onerror` attribute.
/// On 404 / 415 / network error, replaces the `<img>` with a fresh
/// `<span class="files-icon">📄</span>` so the row degrades gracefully
/// to the generic file emoji that non-previewable rows already use.
/// Kept as a `const &str` (rather than inlined in `rsx!`) so the `{...}`
/// in the literal JSON-ish JS body isn't parsed by `rsx!` as a format
/// placeholder.
const THUMB_ONERROR_JS: &str = "this.onerror=null;this.replaceWith(Object.assign(document.createElement('span'),{className:'files-icon',textContent:'📄'}))";

#[derive(Props, Clone, PartialEq)]
pub struct FileRowProps {
    pub entry: FileEntry,
    pub user_id: String,
    pub rename_active: bool,
    pub selected: bool,
    pub on_open_folder: EventHandler<String>,
    pub on_toggle_select: EventHandler<String>,
    pub on_rename_start: EventHandler<String>,
    pub on_rename_commit: EventHandler<(String, String)>, // (from_path, new_name)
    pub on_rename_cancel: EventHandler<()>,
    pub on_delete: EventHandler<String>,
    pub on_share: EventHandler<String>,
}

#[component]
pub fn FileRow(props: FileRowProps) -> Element {
    let FileRowProps {
        entry,
        user_id,
        rename_active,
        selected,
        on_open_folder,
        on_toggle_select,
        on_rename_start,
        on_rename_commit,
        on_rename_cancel,
        on_delete,
        on_share,
    } = props;

    let icon = if entry.is_dir { "📁" } else { "📄" };
    let size = if entry.is_dir {
        "—".to_string()
    } else {
        format_size(entry.size)
    };
    let mtime = format_mtime(entry.mtime_ms);
    let path_for_open = entry.path.clone();
    let path_for_toggle = entry.path.clone();
    let path_for_rename_start = entry.path.clone();
    let path_for_delete = entry.path.clone();
    let path_for_share = entry.path.clone();
    let path_for_commit_enter = entry.path.clone();
    let path_for_commit_blur = entry.path.clone();
    let shared_by = entry.shared_by.clone();
    let share_count = entry.share_count;
    let entry_name_for_enter = entry.name.clone();
    let entry_name_for_blur = entry.name.clone();

    let mut menu_open = use_signal(|| false);
    let mut rename_value = use_signal(|| entry.name.clone());

    let name_cell = if rename_active {
        rsx! {
            span { class: "files-icon", "{icon}" }
            input {
                class: "files-rename-input",
                value: "{rename_value}",
                autofocus: true,
                oninput: move |e| rename_value.set(e.value()),
                onkeydown: move |e: KeyboardEvent| {
                    match e.key() {
                        Key::Enter => {
                            e.prevent_default();
                            let new_name = rename_value().trim().to_string();
                            if new_name.is_empty() || new_name == entry_name_for_enter {
                                on_rename_cancel.call(());
                            } else {
                                on_rename_commit.call((path_for_commit_enter.clone(), new_name));
                            }
                        }
                        Key::Escape => {
                            e.prevent_default();
                            on_rename_cancel.call(());
                        }
                        _ => {}
                    }
                },
                onblur: move |_| {
                    let new_name = rename_value().trim().to_string();
                    if new_name.is_empty() || new_name == entry_name_for_blur {
                        on_rename_cancel.call(());
                    } else {
                        on_rename_commit.call((path_for_commit_blur.clone(), new_name));
                    }
                },
            }
        }
    } else if entry.is_dir {
        rsx! {
            button {
                class: "files-name files-name-folder",
                onclick: move |_| on_open_folder.call(path_for_open.clone()),
                span { class: "files-icon", "{icon}" }
                "{entry.name}"
            }
        }
    } else {
        let href = format!("/dav/files/{user_id}{}", entry.path);
        // Render an inline thumbnail (`<img>`) for previewable mimes when
        // the server-side listing populated a fileid. The `onerror`
        // handler swaps the broken image back to the generic emoji icon
        // on 404 / 415 / network error so the row degrades gracefully —
        // matches the server's allowlist (`provider_for_mime`) but
        // tolerates drift (server says 415 → UI shows the emoji).
        let thumb_url = match (entry.fileid, entry.mime.as_deref()) {
            (Some(fid), Some(mime)) if is_previewable_mime(mime) => {
                Some(format!("/api/files/preview/{fid}?size=64"))
            }
            _ => None,
        };
        rsx! {
            a {
                class: "files-name files-name-file",
                href: "{href}",
                onclick: move |evt: MouseEvent| evt.stop_propagation(),
                if let Some(url) = thumb_url {
                    img {
                        class: "files-thumb",
                        src: "{url}",
                        alt: "",
                        loading: "lazy",
                        "onerror": "{THUMB_ONERROR_JS}",
                    }
                } else {
                    span { class: "files-icon", "{icon}" }
                }
                "{entry.name}"
            }
        }
    };

    rsx! {
        tr { class: if selected { "files-row files-row-selected" } else { "files-row" },
            td { class: "files-cell files-check",
                input {
                    r#type: "checkbox",
                    checked: selected,
                    onchange: move |_| on_toggle_select.call(path_for_toggle.clone()),
                }
            }
            td { class: "files-cell",
                {name_cell}
                if let Some(by) = &shared_by {
                    span { class: "row-shared-by", "(shared by {by})" }
                }
                if share_count > 0 {
                    span { class: "row-share-chip", "🔗 {share_count}" }
                }
            }
            td { class: "files-cell files-size", "{size}" }
            td { class: "files-cell files-mtime", "{mtime}" }
            td { class: "files-cell files-actions",
                button {
                    class: "files-overflow-btn",
                    onclick: move |_| menu_open.set(!menu_open()),
                    "⋯"
                }
                if menu_open() {
                    div { class: "files-overflow-menu",
                        button {
                            class: "files-overflow-item",
                            onclick: move |_| {
                                menu_open.set(false);
                                on_rename_start.call(path_for_rename_start.clone());
                            },
                            "Rename"
                        }
                        button {
                            class: "files-overflow-item files-overflow-danger",
                            onclick: move |_| {
                                menu_open.set(false);
                                on_delete.call(path_for_delete.clone());
                            },
                            "Delete"
                        }
                        button {
                            class: "files-overflow-item",
                            onclick: move |_| {
                                menu_open.set(false);
                                on_share.call(path_for_share.clone());
                            },
                            "🔗  Share"
                        }
                    }
                }
            }
        }
    }
}

fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[0])
    } else {
        format!("{:.1} {}", size, UNITS[unit])
    }
}

fn format_mtime(mtime_ms: i64) -> String {
    let now_ms = current_time_ms();
    let delta_secs = ((now_ms - mtime_ms).max(0)) / 1000;
    if delta_secs < 60 {
        return "just now".into();
    }
    if delta_secs < 3_600 {
        return format!("{} min ago", delta_secs / 60);
    }
    if delta_secs < 86_400 {
        return format!("{} hr ago", delta_secs / 3_600);
    }
    if delta_secs < 7 * 86_400 {
        return format!("{} days ago", delta_secs / 86_400);
    }
    if delta_secs < 30 * 86_400 {
        return format!("{} weeks ago", delta_secs / (7 * 86_400));
    }
    format!("{} months ago", delta_secs / (30 * 86_400))
}

#[cfg(target_arch = "wasm32")]
fn current_time_ms() -> i64 {
    js_sys::Date::now() as i64
}

#[cfg(not(target_arch = "wasm32"))]
fn current_time_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_size_bytes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
    }

    #[test]
    fn format_size_kb() {
        assert_eq!(format_size(2048), "2.0 KB");
    }

    #[test]
    fn format_size_mb() {
        assert_eq!(format_size(2 * 1024 * 1024 + 100 * 1024), "2.1 MB");
    }
}
