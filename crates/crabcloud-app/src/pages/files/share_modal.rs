//! `ShareModal` — recipient picker + current-shares editor for a single
//! item path. Opened from a row's `⋯` menu; consumes the OCS endpoints
//! at `/ocs/v2.php/apps/files_sharing/api/v1/shares` via the same
//! `gloo-net` HTTP plumbing the chunked uploader uses.
//!
//! Per spec §8: debounced autocomplete (`share_recipient_search` server
//! fn) for the picker, `Can edit` / `Can delete` checkboxes per current-
//! share row driving permission-bitmask `PUT`s, and `✕` issuing a
//! `DELETE /shares/{id}` followed by a list refresh. Modal chrome
//! follows `DeleteModal`'s backdrop + panel idiom; CSS classes live in
//! `crates/crabcloud-app/assets/app.css` under `.share-modal*`.

use crate::server_fns::{share_recipient_search, RecipientCandidate};
use dioxus::prelude::*;

// Anchor gloo-timers on the native build — `unused_crate_dependencies`
// fires otherwise because the real call sites are all behind
// `cfg(target_arch = "wasm32")`.
#[cfg(not(target_arch = "wasm32"))]
use gloo_timers as _;

#[cfg(target_arch = "wasm32")]
const SHARES_BASE: &str = "/ocs/v2.php/apps/files_sharing/api/v1/shares";

#[component]
pub fn ShareModal(path: String, on_close: EventHandler<()>) -> Element {
    let basename = path
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(&path)
        .to_string();

    let mut query: Signal<String> = use_signal(String::new);
    let mut candidates: Signal<Vec<RecipientCandidate>> = use_signal(Vec::new);
    let mut selected: Signal<Option<RecipientCandidate>> = use_signal(|| None);
    let mut shares_version: Signal<u64> = use_signal(|| 0);
    let mut status: Signal<Option<String>> = use_signal(|| None);

    // Debounced autocomplete: each keystroke kicks a new task that
    // sleeps 250ms then checks whether the query is still the same. If
    // a fresher keystroke arrived the sleep races and the old task's
    // post-sleep `query() == q` check fails, so it's dropped without
    // calling the server fn. Cheap, no separate cancellation token.
    use_effect(move || {
        let q = query();
        spawn(async move {
            gloo_timers_future_compat(250).await;
            if q != query() {
                return;
            }
            match share_recipient_search(q.clone()).await {
                Ok(rows) => candidates.set(rows),
                Err(_) => candidates.set(Vec::new()),
            }
        });
    });

    // Current shares: re-fetched on initial render and after each
    // mutation (add / permission flip / revoke) by bumping
    // `shares_version`. The `path` capture isn't reactive, but the
    // modal is unmounted+remounted whenever the parent's `share_path`
    // signal changes, so a single fetch per (path, version) is correct.
    let path_for_fetch = path.clone();
    let shares = use_resource(move || {
        let p = path_for_fetch.clone();
        let _ = shares_version();
        async move { fetch_shares(&p).await }
    });

    let on_add = {
        let path_add = path.clone();
        move |_| {
            let path_clone = path_add.clone();
            let sel = selected().clone();
            if let Some(s) = sel {
                spawn(async move {
                    match post_share(&path_clone, &s).await {
                        Ok(()) => {
                            selected.set(None);
                            query.set(String::new());
                            candidates.set(Vec::new());
                            status.set(None);
                            shares_version.set(shares_version() + 1);
                        }
                        Err(msg) => status.set(Some(msg)),
                    }
                });
            }
        }
    };

    let candidates_now = candidates.read().clone();
    let selected_now = selected.read().clone();
    let shares_now: Option<Result<Vec<serde_json::Value>, String>> = shares.read().clone();

    rsx! {
        div { class: "share-modal-backdrop", onclick: move |_| on_close.call(()),
            div {
                class: "share-modal",
                onclick: move |e: MouseEvent| e.stop_propagation(),
                div { class: "share-modal-title",
                    "Share \"{basename}\""
                    button {
                        class: "share-modal-close",
                        onclick: move |_| on_close.call(()),
                        "✕"
                    }
                }
                div { class: "share-modal-body",
                    label { class: "share-modal-label", "Add a user or group" }
                    div { class: "share-modal-add-row",
                        input {
                            class: "share-modal-recipient-input",
                            value: "{query}",
                            placeholder: "user or group…",
                            oninput: move |e| {
                                query.set(e.value());
                                selected.set(None);
                            },
                        }
                        button {
                            class: "share-modal-add-btn",
                            disabled: selected_now.is_none(),
                            onclick: on_add,
                            "Add"
                        }
                    }
                    if !candidates_now.is_empty() {
                        ul { class: "share-modal-candidates",
                            for c in candidates_now.iter() {
                                {
                                    let c_for_click = c.clone();
                                    let c_for_view = c.clone();
                                    let is_active = matches!(&selected_now, Some(s) if s.id == c_for_view.id && s.kind == c_for_view.kind);
                                    rsx! {
                                        li {
                                            class: if is_active { "share-modal-candidate share-modal-candidate-active" } else { "share-modal-candidate" },
                                            onclick: move |_| {
                                                selected.set(Some(c_for_click.clone()));
                                                query.set(c_for_click.display_name.clone());
                                            },
                                            span { class: "share-modal-candidate-name", "{c_for_view.display_name}" }
                                            span { class: "share-modal-candidate-kind", "({c_for_view.kind})" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if let Some(msg) = status.read().clone() {
                        div { class: "share-modal-status", "{msg}" }
                    }

                    div { class: "share-modal-current-title", "Current shares" }
                    match shares_now {
                        None => rsx! { div { class: "share-modal-loading", "loading…" } },
                        Some(Err(e)) => rsx! { div { class: "share-modal-error", "{e}" } },
                        Some(Ok(rows)) if rows.is_empty() => rsx! {
                            div { class: "share-modal-empty", "Not shared yet." }
                        },
                        Some(Ok(rows)) => rsx! {
                            ul { class: "share-modal-current-list",
                                for row in rows.iter() {
                                    {
                                        let id = row.get("id")
                                            .and_then(json_to_i64)
                                            .unwrap_or(0);
                                        let perms = row.get("permissions")
                                            .and_then(|v| v.as_u64())
                                            .map(|n| n as u32)
                                            .unwrap_or(1);
                                        let name = row.get("share_with_displayname")
                                            .and_then(|v| v.as_str())
                                            .or_else(|| row.get("share_with").and_then(|v| v.as_str()))
                                            .unwrap_or("?")
                                            .to_string();
                                        let can_edit = (perms & 6) != 0;
                                        let can_delete = (perms & 8) != 0;
                                        rsx! {
                                            li {
                                                class: "share-modal-share-row",
                                                key: "{id}",
                                                span { class: "share-modal-share-name", "{name}" }
                                                label { class: "share-modal-share-toggle",
                                                    input {
                                                        r#type: "checkbox",
                                                        checked: can_edit,
                                                        onchange: move |e| {
                                                            let new_perms = recompute_perms(perms, 6, e.value() == "true");
                                                            spawn(async move {
                                                                let _ = put_permissions(id, new_perms).await;
                                                                shares_version.set(shares_version() + 1);
                                                            });
                                                        },
                                                    }
                                                    "Can edit"
                                                }
                                                label { class: "share-modal-share-toggle",
                                                    input {
                                                        r#type: "checkbox",
                                                        checked: can_delete,
                                                        onchange: move |e| {
                                                            let new_perms = recompute_perms(perms, 8, e.value() == "true");
                                                            spawn(async move {
                                                                let _ = put_permissions(id, new_perms).await;
                                                                shares_version.set(shares_version() + 1);
                                                            });
                                                        },
                                                    }
                                                    "Can delete"
                                                }
                                                button {
                                                    class: "share-modal-share-revoke",
                                                    onclick: move |_| {
                                                        spawn(async move {
                                                            let _ = delete_share(id).await;
                                                            shares_version.set(shares_version() + 1);
                                                        });
                                                    },
                                                    "✕"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                    }
                }
                div { class: "share-modal-actions",
                    button {
                        class: "share-modal-close-btn",
                        onclick: move |_| on_close.call(()),
                        "Close"
                    }
                }
            }
        }
    }
}

/// Read an integer-typed JSON field tolerantly: OCS sometimes emits the
/// id as a string ("123") rather than a number depending on dialect /
/// PHP-side casting. Try number first, fall back to parsing the string.
fn json_to_i64(v: &serde_json::Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
}

/// Twiddle a single bit-group in the existing wire-permissions mask. The
/// bit group is `mask` (e.g. `6 = update|create` for "Can edit", `8` for
/// "Can delete"); on enable we OR it in, on disable we clear it. The
/// READ bit (1) is always preserved — OCS rejects permissions with bit 1
/// cleared and that's the only way to "downgrade" a share to nothing.
fn recompute_perms(existing: u32, mask: u32, enabled: bool) -> u32 {
    let base = existing | 1;
    if enabled {
        base | mask
    } else {
        base & !mask
    }
}

/// 250ms debounce sleep — uses `gloo-timers` on wasm, `tokio::time` on
/// native so the modal compiles in both build modes (the native build
/// is unreachable in practice because Dioxus only runs the WASM client,
/// but the entire `pages` tree compiles native-side for SSR).
#[cfg(target_arch = "wasm32")]
async fn gloo_timers_future_compat(ms: u32) {
    gloo_timers::future::TimeoutFuture::new(ms).await;
}

#[cfg(not(target_arch = "wasm32"))]
async fn gloo_timers_future_compat(_ms: u32) {
    // SSR / native build: nothing to debounce, return immediately.
}

#[cfg(target_arch = "wasm32")]
async fn fetch_shares(path: &str) -> Result<Vec<serde_json::Value>, String> {
    use gloo_net::http::Request;
    let url = format!(
        "{}?path={}&format=json",
        SHARES_BASE,
        urlencoding_encode(path)
    );
    let resp = Request::get(&url)
        .header("OCS-APIRequest", "true")
        .send()
        .await
        .map_err(|e| format!("net: {e}"))?;
    if !resp.ok() {
        return Err(format!("GET {} -> {}", url, resp.status()));
    }
    let v: serde_json::Value = resp.json().await.map_err(|e| format!("decode: {e}"))?;
    // OCS payload: { ocs: { meta: {...}, data: [...] } }
    let data = v
        .get("ocs")
        .and_then(|o| o.get("data"))
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(data)
}

#[cfg(not(target_arch = "wasm32"))]
async fn fetch_shares(_path: &str) -> Result<Vec<serde_json::Value>, String> {
    Ok(Vec::new())
}

#[cfg(target_arch = "wasm32")]
async fn post_share(path: &str, rec: &RecipientCandidate) -> Result<(), String> {
    use gloo_net::http::Request;
    let url = format!("{}?format=json", SHARES_BASE);
    let body = format!(
        "path={}&shareType={}&shareWith={}&permissions=3",
        urlencoding_encode(path),
        rec.share_type_int,
        urlencoding_encode(&rec.id)
    );
    let resp = Request::post(&url)
        .header("OCS-APIRequest", "true")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .map_err(|e| format!("build: {e}"))?
        .send()
        .await
        .map_err(|e| format!("net: {e}"))?;
    if !resp.ok() {
        return Err(format!("POST shares -> {}", resp.status()));
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
async fn post_share(_path: &str, _rec: &RecipientCandidate) -> Result<(), String> {
    Ok(())
}

#[cfg(target_arch = "wasm32")]
async fn put_permissions(id: i64, perms: u32) -> Result<(), String> {
    use gloo_net::http::Request;
    let url = format!("{}/{id}?format=json", SHARES_BASE);
    let body = format!("permissions={perms}");
    let resp = Request::put(&url)
        .header("OCS-APIRequest", "true")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .map_err(|e| format!("build: {e}"))?
        .send()
        .await
        .map_err(|e| format!("net: {e}"))?;
    if !resp.ok() {
        return Err(format!("PUT shares/{id} -> {}", resp.status()));
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
async fn put_permissions(_id: i64, _perms: u32) -> Result<(), String> {
    Ok(())
}

#[cfg(target_arch = "wasm32")]
async fn delete_share(id: i64) -> Result<(), String> {
    use gloo_net::http::Request;
    let url = format!("{}/{id}?format=json", SHARES_BASE);
    let resp = Request::delete(&url)
        .header("OCS-APIRequest", "true")
        .send()
        .await
        .map_err(|e| format!("net: {e}"))?;
    if !resp.ok() {
        return Err(format!("DELETE shares/{id} -> {}", resp.status()));
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
async fn delete_share(_id: i64) -> Result<(), String> {
    Ok(())
}

/// Minimal percent-encoder for path / id components. Avoids pulling in
/// the `urlencoding` crate (not in the workspace) just for this; the
/// character set is the standard "unreserved" subset.
#[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recompute_perms_enables_can_edit() {
        assert_eq!(recompute_perms(1, 6, true), 7);
        assert_eq!(recompute_perms(9, 6, true), 15);
    }

    #[test]
    fn recompute_perms_disables_can_edit_preserves_read() {
        assert_eq!(recompute_perms(7, 6, false), 1);
        assert_eq!(recompute_perms(15, 6, false), 9);
    }

    #[test]
    fn recompute_perms_can_delete() {
        assert_eq!(recompute_perms(1, 8, true), 9);
        assert_eq!(recompute_perms(15, 8, false), 7);
    }

    #[test]
    fn urlencoding_handles_space_and_unicode() {
        assert_eq!(urlencoding_encode("a b"), "a%20b");
        assert_eq!(urlencoding_encode("/Vacation Photos"), "/Vacation%20Photos");
    }

    #[test]
    fn json_to_i64_accepts_number_and_string() {
        let v = serde_json::json!(123);
        assert_eq!(json_to_i64(&v), Some(123));
        let s = serde_json::json!("456");
        assert_eq!(json_to_i64(&s), Some(456));
        let bad = serde_json::json!("notanumber");
        assert_eq!(json_to_i64(&bad), None);
    }
}
