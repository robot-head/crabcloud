//! Per-file "Versions" panel — opened from the file row's ⋯ menu.
//!
//! Lists every stored version of a single file (via [`list_versions`])
//! and exposes Restore / Delete per row. Mirrors `pages::trash`'s
//! per-row in-flight tracking + dismissable error banner + confirm
//! modal so a single misclick can't permanently drop a version.
//!
//! The panel is rendered as a modal (`.files-modal-*` chrome) when the
//! parent's `pending_versions_fileid` signal is `Some(fileid)`. Closing
//! the panel (backdrop click, ✕ button, or the parent's `on_close`)
//! resets the signal to `None`, which unmounts the component.
//!
//! On any successful mutation (Restore or Delete) the resource is
//! refetched via a monotonic refresh counter so the affected row
//! disappears (or, for Restore, the new pre-snapshot row appears).

use crate::server_fns::versions::{delete_version, list_versions, restore_version, VersionDto};
use dioxus::prelude::*;
use std::collections::HashSet;

#[derive(Props, Clone, PartialEq)]
pub struct VersionsPanelProps {
    /// The fileid whose versions we're listing. Owned by the parent's
    /// `pending_versions_fileid` signal; closing the panel resets that
    /// signal to `None`, which unmounts this component.
    pub fileid: i64,
    /// File name to show in the panel header (best-effort: empty
    /// string is acceptable but ugly). Resolved by the parent from
    /// the same FileEntry that triggered the menu click.
    pub filename: String,
    pub on_close: EventHandler<()>,
}

#[component]
pub fn VersionsPanel(props: VersionsPanelProps) -> Element {
    let VersionsPanelProps {
        fileid,
        filename,
        on_close,
    } = props;

    let mut refresh = use_signal(|| 0u64);
    // Per-row in-flight tracking. A row's id sits in this set from the
    // moment its Restore or Delete handler fires until the server-fn
    // future resolves and the refetch is requested. Mirrors `trash.rs`
    // so multiple rows can have in-flight mutations in parallel.
    let mut in_flight: Signal<HashSet<i64>> = use_signal(HashSet::new);
    // Per-row delete confirm. `Some(id)` opens the confirm modal for
    // that row; `None` closes it. Mirrors the trash page's purge-
    // confirm flow — deleting a single version is permanent.
    let mut pending_delete: Signal<Option<i64>> = use_signal(|| None);
    // Last error from a failed mutation. Surfaced in a dismissable
    // banner inside the panel; cleared on dismiss or next success.
    let mut last_error: Signal<Option<String>> = use_signal(|| None);

    // `list_versions` is only invoked from the client (wasm) side. SSR
    // renders the panel chrome + a pending skeleton; the resource
    // starts `Pending` and is populated after hydration.
    let entries = use_resource(move || async move {
        let _ = refresh();
        list_versions(fileid).await.map_err(|e| format!("{e}"))
    });

    let on_restore = move |id: i64| {
        if in_flight.read().contains(&id) {
            return;
        }
        in_flight.write().insert(id);
        spawn(async move {
            match restore_version(id).await {
                Ok(_) => last_error.set(None),
                Err(e) => last_error.set(Some(format!("Failed to restore version: {e}"))),
            }
            in_flight.write().remove(&id);
            refresh.set(refresh() + 1);
        });
    };

    let on_delete_request = move |id: i64| pending_delete.set(Some(id));
    let on_delete_cancel = move |_evt: MouseEvent| pending_delete.set(None);
    let on_delete_confirmed = move |_evt: MouseEvent| {
        let Some(id) = pending_delete() else {
            return;
        };
        if in_flight.read().contains(&id) {
            return;
        }
        in_flight.write().insert(id);
        pending_delete.set(None);
        spawn(async move {
            match delete_version(id).await {
                Ok(_) => last_error.set(None),
                Err(e) => last_error.set(Some(format!("Failed to delete version: {e}"))),
            }
            in_flight.write().remove(&id);
            refresh.set(refresh() + 1);
        });
    };

    let on_dismiss_error = move |_evt: MouseEvent| last_error.set(None);

    let entries_view: Option<Result<Vec<VersionDto>, String>> = entries.read().clone();
    let in_flight_snapshot: HashSet<i64> = in_flight.read().clone();
    let delete_target = pending_delete();
    let error_message = last_error.read().clone();

    rsx! {
        div {
            class: "files-modal-backdrop",
            onclick: move |_| on_close.call(()),
            div {
                class: "files-modal versions-panel",
                onclick: move |e: MouseEvent| e.stop_propagation(),
                div { class: "versions-panel-header",
                    div { class: "files-modal-title", "Versions of {filename}" }
                    button {
                        r#type: "button",
                        class: "versions-panel-close",
                        aria_label: "Close",
                        onclick: move |_| on_close.call(()),
                        "✕"
                    }
                }
                if let Some(msg) = error_message {
                    div {
                        class: "versions-panel-banner",
                        role: "alert",
                        span { class: "versions-panel-banner-icon", aria_hidden: "true", "⚠" }
                        span { class: "versions-panel-banner-text", "{msg}" }
                        button {
                            r#type: "button",
                            class: "versions-panel-banner-close",
                            aria_label: "Dismiss error",
                            onclick: on_dismiss_error,
                            "✕"
                        }
                    }
                }
                VersionsList {
                    entries: entries_view,
                    in_flight: in_flight_snapshot,
                    on_restore,
                    on_delete: on_delete_request,
                }
            }
        }
        if delete_target.is_some() {
            div { class: "files-modal-backdrop", onclick: on_delete_cancel,
                div {
                    class: "files-modal",
                    onclick: move |e: MouseEvent| e.stop_propagation(),
                    div { class: "files-modal-title", "Delete this version?" }
                    div { class: "files-modal-body",
                        "This version will be permanently deleted. This cannot be undone."
                    }
                    div { class: "files-modal-actions",
                        button {
                            r#type: "button",
                            class: "files-modal-cancel",
                            onclick: on_delete_cancel,
                            "Cancel"
                        }
                        button {
                            r#type: "button",
                            class: "files-modal-confirm",
                            onclick: on_delete_confirmed,
                            "Delete"
                        }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct VersionsListProps {
    entries: Option<Result<Vec<VersionDto>, String>>,
    in_flight: HashSet<i64>,
    on_restore: EventHandler<i64>,
    on_delete: EventHandler<i64>,
}

#[component]
fn VersionsList(props: VersionsListProps) -> Element {
    let VersionsListProps {
        entries,
        in_flight,
        on_restore,
        on_delete,
    } = props;
    match entries {
        Some(Ok(rows)) if rows.is_empty() => rsx! {
            p { class: "versions-panel-empty", "No versions yet." }
        },
        Some(Ok(rows)) => rsx! {
            ul { class: "versions-panel-list",
                for entry in rows.into_iter() {
                    {
                        let row_busy = in_flight.contains(&entry.id);
                        rsx! {
                            VersionRow {
                                key: "{entry.id}",
                                entry,
                                busy: row_busy,
                                on_restore,
                                on_delete,
                            }
                        }
                    }
                }
            }
        },
        // Raw `ServerFnError` text isn't useful to a user. Show a
        // friendly placeholder.
        Some(Err(_)) => rsx! {
            p { class: "versions-panel-error", "Couldn't load versions. Try again." }
        },
        None => rsx! { p { class: "versions-panel-loading", "Loading…" } },
    }
}

#[derive(Props, Clone, PartialEq)]
struct VersionRowProps {
    entry: VersionDto,
    busy: bool,
    on_restore: EventHandler<i64>,
    on_delete: EventHandler<i64>,
}

#[component]
fn VersionRow(props: VersionRowProps) -> Element {
    let VersionRowProps {
        entry,
        busy,
        on_restore,
        on_delete,
    } = props;
    let id = entry.id;
    let when = format_version_mtime(entry.version_mtime);
    let size = format_size(entry.size);
    // Composed accessible label so a screen reader announces the row
    // as a single readable sentence (matches the trash page idiom).
    let aria_label = format!("Version from {when}, {size}");

    rsx! {
        li { class: "versions-panel-row", aria_label: "{aria_label}",
            span { class: "versions-panel-row-when", "{when}" }
            span { class: "versions-panel-row-size", "{size}" }
            div { class: "versions-panel-row-actions",
                button {
                    r#type: "button",
                    class: "versions-panel-restore-btn",
                    disabled: busy,
                    onclick: move |_| on_restore.call(id),
                    "Restore"
                }
                button {
                    r#type: "button",
                    class: "versions-panel-delete-btn",
                    disabled: busy,
                    onclick: move |_| on_delete.call(id),
                    "Delete"
                }
            }
        }
    }
}

/// Format a Unix-seconds timestamp as `YYYY-MM-DD HH:MM` (UTC).
/// Matches the trash page's deleted-at format for visual consistency.
/// A relative "N minutes ago" would be friendlier for fresh edits but
/// composes badly when a file has versions spanning hours, days, and
/// weeks all at once.
fn format_version_mtime(unix_secs: i64) -> String {
    use chrono::{TimeZone, Utc};
    Utc.timestamp_opt(unix_secs, 0)
        .single()
        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| unix_secs.to_string())
}

/// Human-readable byte size: "1.2 MiB", "512 B", etc. Uses powers of
/// 1024 because the rest of the storage stack reports KiB/MiB in
/// the same convention. Mirrors the row's `format_size` helper but
/// lives here because the version DTO uses `i64` (sourced from the
/// versions table) where the file row uses `u64`.
fn format_size(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let b = bytes.max(0) as f64;
    let mut size = b;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes.max(0), UNITS[0])
    } else {
        format!("{:.1} {}", size, UNITS[unit])
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    /// SSR snapshot: the panel list rendered with two stubbed versions
    /// emits the formatted timestamps, human-readable sizes, and the
    /// per-row Restore / Delete button text. Locks the rendered shape
    /// so a copy or class-name change is a visible diff.
    #[test]
    fn renders_two_versions_with_action_buttons() {
        let rows = vec![
            VersionDto {
                id: 1,
                version_mtime: 1_700_000_000,
                size: 1234,
            },
            VersionDto {
                id: 2,
                version_mtime: 1_700_000_100,
                size: 2 * 1024 * 1024,
            },
        ];

        #[component]
        fn Wrapper(rows: Vec<VersionDto>) -> Element {
            rsx! {
                VersionsList {
                    entries: Some(Ok(rows)),
                    in_flight: HashSet::<i64>::new(),
                    on_restore: move |_: i64| {},
                    on_delete: move |_: i64| {},
                }
            }
        }
        let html = dioxus::ssr::render_element(rsx! { Wrapper { rows } });
        assert!(
            html.contains("2023-11-14 22:13"),
            "expected first formatted timestamp in HTML, got: {html}"
        );
        assert!(
            html.contains("1.2 KiB"),
            "expected first row size 1.2 KiB in HTML, got: {html}"
        );
        assert!(
            html.contains("2.0 MiB"),
            "expected second row size 2.0 MiB in HTML, got: {html}"
        );
        assert!(
            html.contains("Restore"),
            "expected Restore button text in HTML, got: {html}"
        );
        assert!(
            html.contains("Delete"),
            "expected Delete button text in HTML, got: {html}"
        );
    }

    /// SSR snapshot: an empty version list renders the
    /// "No versions yet." placeholder (and no list element).
    #[test]
    fn empty_state_renders_placeholder() {
        #[component]
        fn Wrapper() -> Element {
            rsx! {
                VersionsList {
                    entries: Some(Ok(Vec::<VersionDto>::new())),
                    in_flight: HashSet::<i64>::new(),
                    on_restore: move |_: i64| {},
                    on_delete: move |_: i64| {},
                }
            }
        }
        let html = dioxus::ssr::render_element(rsx! { Wrapper {} });
        assert!(
            html.contains("No versions yet."),
            "expected empty placeholder in HTML, got: {html}"
        );
        assert!(
            !html.contains("<ul"),
            "empty version list should not render a list element, got: {html}"
        );
    }

    /// Error state renders the friendly placeholder copy, not the raw
    /// `ServerFnError` text. Mirrors the trash page policy: the raw
    /// transport/serde diagnostic isn't useful to the user.
    #[test]
    fn error_state_renders_friendly_copy() {
        #[component]
        fn Wrapper() -> Element {
            rsx! {
                VersionsList {
                    entries: Some(Err::<Vec<VersionDto>, String>(
                        "internal: deadline exceeded".into(),
                    )),
                    in_flight: HashSet::<i64>::new(),
                    on_restore: move |_: i64| {},
                    on_delete: move |_: i64| {},
                }
            }
        }
        let html = dioxus::ssr::render_element(rsx! { Wrapper {} });
        assert!(
            html.contains("class=\"versions-panel-error\""),
            "expected versions-panel-error placeholder in HTML, got: {html}"
        );
        assert!(
            html.contains("load versions. Try again."),
            "expected friendly error copy tail in HTML, got: {html}"
        );
        assert!(
            !html.contains("deadline exceeded"),
            "error placeholder must not leak the raw error string, got: {html}"
        );
    }

    /// Per-row in-flight tracking: only the row whose id is in the
    /// `in_flight` set renders its buttons disabled. The other row
    /// stays interactive so the user can fire mutations in parallel.
    #[test]
    fn per_row_in_flight_disables_only_matching_row() {
        let rows = vec![
            VersionDto {
                id: 1,
                version_mtime: 1_700_000_000,
                size: 100,
            },
            VersionDto {
                id: 2,
                version_mtime: 1_700_000_100,
                size: 200,
            },
        ];

        #[component]
        fn Wrapper(rows: Vec<VersionDto>) -> Element {
            let mut set = HashSet::<i64>::new();
            set.insert(1);
            rsx! {
                VersionsList {
                    entries: Some(Ok(rows)),
                    in_flight: set,
                    on_restore: move |_: i64| {},
                    on_delete: move |_: i64| {},
                }
            }
        }
        let html = dioxus::ssr::render_element(rsx! { Wrapper { rows } });
        let disabled_count = html.matches("disabled").count();
        assert_eq!(
            disabled_count, 2,
            "expected exactly 2 disabled buttons (one row, two buttons), got {disabled_count}: {html}"
        );
    }

    /// Accessibility: the row carries a composed aria-label and the
    /// action buttons render with explicit `type="button"` so they
    /// can't be hijacked by a future surrounding form context.
    #[test]
    fn row_emits_accessibility_attributes() {
        let rows = vec![VersionDto {
            id: 1,
            version_mtime: 1_700_000_000,
            size: 1234,
        }];

        #[component]
        fn Wrapper(rows: Vec<VersionDto>) -> Element {
            rsx! {
                VersionsList {
                    entries: Some(Ok(rows)),
                    in_flight: HashSet::<i64>::new(),
                    on_restore: move |_: i64| {},
                    on_delete: move |_: i64| {},
                }
            }
        }
        let html = dioxus::ssr::render_element(rsx! { Wrapper { rows } });
        assert!(
            html.contains("aria-label=\"Version from 2023-11-14 22:13"),
            "expected composed aria-label on the row, got: {html}"
        );
        assert!(
            html.contains("type=\"button\""),
            "expected explicit type=\"button\" on action buttons, got: {html}"
        );
    }

    #[test]
    fn format_size_bytes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
    }

    #[test]
    fn format_size_kib() {
        assert_eq!(format_size(1234), "1.2 KiB");
    }

    #[test]
    fn format_size_mib() {
        assert_eq!(format_size(2 * 1024 * 1024), "2.0 MiB");
    }

    #[test]
    fn format_size_negative_clamps_to_zero() {
        // i64 size shouldn't realistically be negative, but defensive
        // clamp so a stray sign doesn't render "-1 B" to a user.
        assert_eq!(format_size(-5), "0 B");
    }

    #[test]
    fn format_version_mtime_renders_utc_minute() {
        // 2023-11-14 22:13:20 UTC
        assert_eq!(format_version_mtime(1_700_000_000), "2023-11-14 22:13");
    }
}
