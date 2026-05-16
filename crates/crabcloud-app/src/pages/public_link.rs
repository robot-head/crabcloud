//! Anonymous public-link viewer — `/s/:token` and `/s/:token/*path`.
//!
//! Rendered by the dx fullstack runtime under the OCS+app sub-router. The
//! page calls the `list_public_link` and `meta_public_link` server fns to
//! decide whether to render the password gate, the folder listing, the
//! file-drop upload widget, or some combination. The auth layer (mounted
//! on the `/s/{token}` axum router) attaches `PublicLinkAuthContext` and
//! optionally `PasswordGateRequired` to the request before the server fn
//! handler executes — those server fns read the extension and return a
//! single DTO.

use crate::pages::files::breadcrumb::Breadcrumb;
use crate::pages::files::path::segments_to_path;
use crate::server_fns::public_link::{list_public_link, meta_public_link, PublicLinkMeta};
use crate::server_fns::FileEntry;
use dioxus::prelude::*;

#[component]
pub fn PublicLinkRoot(token: String) -> Element {
    rsx! { PublicLink { token, path: Vec::<String>::new() } }
}

#[component]
pub fn PublicLink(token: String, path: Vec<String>) -> Element {
    let token_for_meta = token.clone();
    let meta_resource = use_resource(move || {
        let t = token_for_meta.clone();
        async move { meta_public_link(t).await.map_err(|e| format!("{e}")) }
    });

    let meta_value: Option<Result<PublicLinkMeta, String>> = meta_resource.read().clone();
    match meta_value {
        None => rsx! { div { class: "public-link-viewer public-link-loading", "Loading…" } },
        Some(Err(msg)) => rsx! {
            div { class: "public-link-viewer public-link-error",
                h1 { "This link is unavailable" }
                p { "{msg}" }
            }
        },
        Some(Ok(meta)) => {
            if meta.password_required {
                rsx! { PasswordGate { token: token.clone(), wrong: false } }
            } else {
                rsx! { PublicLinkBody { token, path, meta } }
            }
        }
    }
}

#[component]
fn PasswordGate(token: String, wrong: bool) -> Element {
    let action = format!("/s/{token}/unlock");
    rsx! {
        div { class: "public-link-viewer public-link-gate",
            h1 { "Enter password" }
            p { "This shared link is password protected." }
            form {
                class: "public-link-gate-form",
                method: "post",
                action: "{action}",
                enctype: "application/x-www-form-urlencoded",
                if wrong {
                    p { class: "public-link-gate-error", "Incorrect password. Try again." }
                }
                input {
                    r#type: "password",
                    name: "password",
                    placeholder: "Password",
                    autofocus: true,
                    required: true,
                }
                button { r#type: "submit", "Unlock" }
            }
        }
    }
}

#[component]
fn PublicLinkBody(token: String, path: Vec<String>, meta: PublicLinkMeta) -> Element {
    let current_path = segments_to_path(&path);
    let token_for_list = token.clone();
    let path_for_list = current_path.clone();
    let entries = use_resource(move || {
        let t = token_for_list.clone();
        let p = path_for_list.clone();
        async move { list_public_link(t, p).await.map_err(|e| format!("{e}")) }
    });
    let entries_value: Option<Result<Vec<FileEntry>, String>> = entries.read().clone();

    let create_only = meta.can_create && !meta.can_read;
    let show_listing = meta.can_read;
    let show_upload = meta.can_create;

    rsx! {
        div { class: "public-link-viewer",
            header { class: "public-link-header",
                h1 { "Shared with you" }
                if !create_only {
                    Breadcrumb {
                        path: current_path.clone(),
                        on_navigate: move |_target: String| {
                            // Browser-side navigation is handled by clicking
                            // anchor links elsewhere; the dx Routable enum
                            // wires `/s/:token/*path` so anchors push state
                            // naturally. This handler intentionally no-ops
                            // because Breadcrumb requires a callback.
                        },
                    }
                }
            }

            if show_listing {
                PublicListing {
                    token: token.clone(),
                    base_path: current_path.clone(),
                    entries: entries_value,
                }
            }

            if show_upload {
                PublicUploadWidget { token: token.clone() }
            }

            if create_only && !show_listing {
                p { class: "public-link-filedrop-hint", "Drop a file here or use the upload button to share it with the owner." }
            }
        }
    }
}

#[component]
fn PublicListing(
    token: String,
    base_path: String,
    entries: Option<Result<Vec<FileEntry>, String>>,
) -> Element {
    match entries {
        None => rsx! { div { class: "public-link-listing public-link-loading", "Loading…" } },
        Some(Err(msg)) => rsx! {
            div { class: "public-link-listing public-link-error", "Failed to load: {msg}" }
        },
        Some(Ok(es)) if es.is_empty() => rsx! {
            div { class: "public-link-listing public-link-empty", "This folder is empty." }
        },
        Some(Ok(es)) => rsx! {
            table { class: "files-table public-link-table",
                thead {
                    tr {
                        th { class: "files-th", "Name" }
                        th { class: "files-th files-size", "Size" }
                    }
                }
                tbody {
                    for e in es {
                        PublicRow { token: token.clone(), base_path: base_path.clone(), entry: e.clone() }
                    }
                }
            }
        },
    }
}

#[component]
fn PublicRow(token: String, base_path: String, entry: FileEntry) -> Element {
    let icon = if entry.is_dir { "📁" } else { "📄" };
    let size = if entry.is_dir {
        "—".to_string()
    } else {
        format_size(entry.size)
    };
    if entry.is_dir {
        // Folder anchors point at the same /s/:token/*path route; clicking
        // forces a full navigation which is fine — the SSR side re-renders
        // with the new path and the auth context is re-supplied per request.
        let href = if base_path == "/" {
            format!("/s/{token}/{}", entry.name)
        } else {
            format!("/s/{token}{}/{}", base_path, entry.name)
        };
        rsx! {
            tr { class: "files-row",
                td { class: "files-cell",
                    a { class: "files-name files-name-folder", href: "{href}",
                        span { class: "files-icon", "{icon}" }
                        "{entry.name}"
                    }
                }
                td { class: "files-cell files-size", "{size}" }
            }
        }
    } else {
        // File anchor points at the download endpoint. The leading slash on
        // entry.path is stripped because the download route's `{*path}`
        // captures a path with no leading slash.
        let rel = entry.path.trim_start_matches('/');
        let href = format!("/s/{token}/download/{rel}");
        rsx! {
            tr { class: "files-row",
                td { class: "files-cell",
                    a { class: "files-name files-name-file", href: "{href}",
                        span { class: "files-icon", "{icon}" }
                        "{entry.name}"
                    }
                }
                td { class: "files-cell files-size", "{size}" }
            }
        }
    }
}

#[component]
fn PublicUploadWidget(token: String) -> Element {
    // The widget is intentionally simple: a hidden file input, an explicit
    // button, and an inline status message. The upload posts to the
    // `/s/{token}/upload/{filename}` endpoint, which already handles
    // collision suffixes server-side. For SP8-E we ship the WASM-driven
    // upload only; the SSR fallback shows the controls but no JS-driven
    // upload happens until hydration.
    let _ = token.clone(); // captured by the wasm closure below
    rsx! {
        div { class: "public-link-upload",
            h2 { "Upload a file" }
            p { class: "public-link-upload-hint", "Choose a file and we’ll add it to this shared folder." }
            input {
                r#type: "file",
                id: "public-link-upload-input",
                multiple: false,
            }
            button {
                class: "public-link-upload-btn",
                onclick: {
                    let token = token.clone();
                    move |_| {
                        #[cfg(target_arch = "wasm32")]
                        {
                            do_upload(&token);
                        }
                        #[cfg(not(target_arch = "wasm32"))]
                        {
                            let _ = &token;
                        }
                    }
                },
                "Upload"
            }
            div { id: "public-link-upload-status", class: "public-link-upload-status" }
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn do_upload(token: &str) {
    use wasm_bindgen::JsCast;
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(doc) = window.document() else {
        return;
    };
    let Some(input_el) = doc.get_element_by_id("public-link-upload-input") else {
        return;
    };
    let Ok(input) = input_el.dyn_into::<web_sys::HtmlInputElement>() else {
        return;
    };
    let Some(files) = input.files() else {
        return;
    };
    if files.length() == 0 {
        return;
    }
    let Some(file) = files.get(0) else {
        return;
    };
    let name = file.name();
    let token = token.to_string();
    let url = format!("/s/{token}/upload/{name}");
    let status_id = "public-link-upload-status".to_string();
    dioxus::prelude::spawn(async move {
        let opts = web_sys::RequestInit::new();
        opts.set_method("POST");
        opts.set_body(&file.into());
        let request = match web_sys::Request::new_with_str_and_init(&url, &opts) {
            Ok(r) => r,
            Err(_) => return,
        };
        let promise = window.fetch_with_request(&request);
        let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
        // Mark status; either way the user can refresh to see the file.
        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
            if let Some(el) = doc.get_element_by_id(&status_id) {
                el.set_text_content(Some("Upload finished — refresh to see your file."));
            }
        }
    });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_size_zero() {
        assert_eq!(format_size(0), "0 B");
    }

    #[test]
    fn format_size_kb() {
        assert_eq!(format_size(2048), "2.0 KB");
    }
}
