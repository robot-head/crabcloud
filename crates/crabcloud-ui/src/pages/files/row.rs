//! Single row in the file list. Click on a folder row navigates into it;
//! file rows are anchor links to the WebDAV GET URL so the browser handles
//! the download/inline-view natively. See spec §8.

use crate::server_fns::FileEntry;
use dioxus::prelude::*;

#[component]
pub fn FileRow(entry: FileEntry, user_id: String, on_open_folder: EventHandler<String>) -> Element {
    let icon = if entry.is_dir { "📁" } else { "📄" };
    let size = if entry.is_dir {
        "—".to_string()
    } else {
        format_size(entry.size)
    };
    let mtime = format_mtime(entry.mtime_ms);
    let name_cell = if entry.is_dir {
        let path = entry.path.clone();
        rsx! {
            button {
                class: "files-name files-name-folder",
                onclick: move |_| on_open_folder.call(path.clone()),
                span { class: "files-icon", "{icon}" }
                "{entry.name}"
            }
        }
    } else {
        let href = format!("/dav/files/{user_id}{}", entry.path);
        rsx! {
            a {
                class: "files-name files-name-file",
                href: "{href}",
                onclick: move |evt: MouseEvent| evt.stop_propagation(),
                span { class: "files-icon", "{icon}" }
                "{entry.name}"
            }
        }
    };
    rsx! {
        tr { class: "files-row",
            td { class: "files-cell files-check", input { r#type: "checkbox" } }
            td { class: "files-cell", {name_cell} }
            td { class: "files-cell files-size", "{size}" }
            td { class: "files-cell files-mtime", "{mtime}" }
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
