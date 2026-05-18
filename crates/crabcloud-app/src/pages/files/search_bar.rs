//! Top-bar search input + dropdown overlay.
//!
//! Debounced 300ms input → `search_files(q, None)` server fn → dropdown
//! of up to 10 hits. Each row renders the basename + path + a
//! mime-derived icon. Clicking a row navigates to the file's containing
//! folder via the existing `/apps/files/...` route.
//!
//! Dropdown closes on Escape / blur / click outside (blur is the
//! click-outside surrogate — we rely on the input losing focus when
//! the user clicks anywhere off it). A short post-blur delay keeps a
//! click on a dropdown row from racing the close.
//!
//! Mirrors the `share_modal::gloo_timers_future_compat` debounce
//! idiom: gloo-timers on wasm, no-op on native so SSR still compiles.
//! The native build of the page never reaches the dropdown (Dioxus
//! only renders this on the client), but the entire `pages` tree
//! compiles host-side for SSR.

use crate::server_fns::search::{search_files, SearchHitDto, SearchResponseDto};
use dioxus::prelude::*;

// Anchor gloo-timers on the native build — `unused_crate_dependencies`
// fires otherwise because the real call sites are wasm-only.
#[cfg(not(target_arch = "wasm32"))]
use gloo_timers as _;

/// Top-bar search bar. Holds its own query / hits / open state; the
/// parent (`TopBar`) just renders `<SearchBar />`.
#[component]
pub fn SearchBar() -> Element {
    let mut query = use_signal::<String>(String::new);
    let mut hits = use_signal::<Vec<SearchHitDto>>(Vec::new);
    let mut open = use_signal::<bool>(|| false);
    let mut loading = use_signal::<bool>(|| false);
    let mut last_error = use_signal::<Option<String>>(|| None);

    // Debounced effect: on every query mutation kick a task that
    // sleeps 300ms then checks whether the query still matches what
    // it captured. A fresher keystroke racing this task makes the
    // post-sleep `query() != q` check fire and we drop the stale call.
    // No separate cancellation token needed.
    use_effect(move || {
        let q = query();
        spawn(async move {
            gloo_timers_future_compat(300).await;
            if query() != q {
                return;
            }
            if q.trim().is_empty() {
                hits.set(Vec::new());
                loading.set(false);
                last_error.set(None);
                return;
            }
            loading.set(true);
            match search_files(q.clone(), None).await {
                Ok(SearchResponseDto { hits: h, .. }) => {
                    hits.set(h);
                    last_error.set(None);
                }
                Err(e) => {
                    last_error.set(Some(format!("Search failed: {e}")));
                    hits.set(Vec::new());
                }
            }
            loading.set(false);
        });
    });

    let on_input = move |e: FormEvent| {
        query.set(e.value());
        open.set(true);
    };
    let on_focus = move |_evt: FocusEvent| open.set(true);
    let on_blur = move |_evt: FocusEvent| {
        // Tiny delay so a click on a result anchor registers before
        // the dropdown unmounts. 150ms matches the share-modal
        // debounce-grace pattern.
        spawn(async move {
            gloo_timers_future_compat(150).await;
            open.set(false);
        });
    };
    let on_keydown = move |evt: KeyboardEvent| {
        if evt.key() == Key::Escape {
            open.set(false);
        }
    };

    let is_open = open();
    let q_snapshot = query();
    let hits_snapshot = hits();
    let is_loading = loading();
    let error_snapshot = last_error();

    let q_for_input = q_snapshot.clone();
    rsx! {
        div { class: "search-bar",
            input {
                r#type: "search",
                class: "search-bar-input",
                placeholder: "Search files…",
                value: "{q_for_input}",
                oninput: on_input,
                onfocus: on_focus,
                onblur: on_blur,
                onkeydown: on_keydown,
                aria_label: "Search files",
                aria_autocomplete: "list",
            }
            if is_open {
                SearchDropdown {
                    query: q_snapshot,
                    hits: hits_snapshot,
                    loading: is_loading,
                    error: error_snapshot,
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct DropdownProps {
    query: String,
    hits: Vec<SearchHitDto>,
    loading: bool,
    error: Option<String>,
}

#[component]
fn SearchDropdown(props: DropdownProps) -> Element {
    rsx! {
        div { class: "search-dropdown", role: "listbox",
            if let Some(err) = &props.error {
                p { class: "search-dropdown-error", role: "alert", "{err}" }
            } else if props.query.trim().is_empty() {
                p { class: "search-dropdown-hint", "Type to search" }
            } else if props.loading {
                p { class: "search-dropdown-loading", "Searching…" }
            } else if props.hits.is_empty() {
                p { class: "search-dropdown-empty", "No matches." }
            } else {
                ul { class: "search-dropdown-list",
                    for hit in props.hits.iter() {
                        SearchHitRow { key: "{hit.fileid}", hit: hit.clone() }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct HitProps {
    hit: SearchHitDto,
}

#[component]
fn SearchHitRow(props: HitProps) -> Element {
    let h = props.hit;
    let nav_to = parent_route(&h.path);
    let icon = icon_for_mime(&h.mime);
    rsx! {
        li { class: "search-dropdown-row", role: "option",
            a { href: "{nav_to}",
                span { class: "search-dropdown-icon", aria_hidden: "true", "{icon}" }
                span { class: "search-dropdown-name", "{h.basename}" }
                span { class: "search-dropdown-path", "{h.path}" }
            }
        }
    }
}

/// Build the `/apps/files/<parent>` route for a hit. `path` is the
/// viewer-relative absolute path stored in the search index (e.g.
/// `/photos/vacation.jpg`); the route slug is the parent directory
/// (everything up to the last `/`). For top-level files the route is
/// `/apps/files/` (the home root).
fn parent_route(path: &str) -> String {
    let trimmed = path.strip_prefix('/').unwrap_or(path);
    match trimmed.rsplit_once('/') {
        Some((parent, _)) if !parent.is_empty() => format!("/apps/files/{parent}"),
        _ => "/apps/files/".to_string(),
    }
}

/// Map a mime to a 1-character display icon. Mirrors
/// `pages::activity::icon_for_event_type` — emoji-only, no asset
/// dependency, no localization. Fall-through is the generic `📄`,
/// which is also used for `text/*`.
fn icon_for_mime(mime: &str) -> &'static str {
    if mime.starts_with("image/") {
        "🖼"
    } else if mime.starts_with("video/") {
        "🎬"
    } else if mime.starts_with("audio/") {
        "🎵"
    } else if mime == "application/pdf" {
        "📕"
    } else {
        "📄"
    }
}

/// 300ms / 150ms debounce sleep. Uses `gloo-timers` on wasm and a
/// no-op on native (SSR) so the component compiles in both build
/// modes. Mirrors the equivalent helper in `pages::files::share_modal`.
#[cfg(target_arch = "wasm32")]
async fn gloo_timers_future_compat(ms: u32) {
    gloo_timers::future::TimeoutFuture::new(ms).await;
}

#[cfg(not(target_arch = "wasm32"))]
async fn gloo_timers_future_compat(_ms: u32) {
    // SSR / native build: no event loop to debounce, return immediately.
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    fn hit(fileid: i64, basename: &str, path: &str, mime: &str) -> SearchHitDto {
        SearchHitDto {
            fileid,
            basename: basename.into(),
            path: path.into(),
            mime: mime.into(),
            mtime: 1_700_000_000,
            size: 1024,
        }
    }

    /// `parent_route` strips the basename and prefixes the files route.
    /// Top-level files land on `/apps/files/`; nested files land on
    /// `/apps/files/<parent-dirs>`.
    #[test]
    fn parent_route_handles_nested_and_top_level() {
        assert_eq!(parent_route("/photos/vacation.jpg"), "/apps/files/photos");
        assert_eq!(parent_route("/docs/q3/report.docx"), "/apps/files/docs/q3");
        assert_eq!(parent_route("/readme.md"), "/apps/files/");
        // Path without a leading slash still works.
        assert_eq!(parent_route("notes.txt"), "/apps/files/");
        assert_eq!(parent_route("folder/notes.txt"), "/apps/files/folder");
    }

    /// `icon_for_mime` returns a deterministic per-family glyph.
    #[test]
    fn icon_for_mime_handles_common_families() {
        assert_eq!(icon_for_mime("image/jpeg"), "🖼");
        assert_eq!(icon_for_mime("video/mp4"), "🎬");
        assert_eq!(icon_for_mime("audio/mpeg"), "🎵");
        assert_eq!(icon_for_mime("application/pdf"), "📕");
        assert_eq!(icon_for_mime("text/plain"), "📄");
        // Unknown mime falls through to the generic file icon.
        assert_eq!(
            icon_for_mime("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
            "📄"
        );
    }

    /// `query=""` + `open=true` renders the "Type to search" hint and
    /// no list element.
    #[test]
    fn dropdown_empty_query_renders_hint() {
        #[component]
        fn Wrapper() -> Element {
            rsx! {
                SearchDropdown {
                    query: String::new(),
                    hits: Vec::<SearchHitDto>::new(),
                    loading: false,
                    error: None,
                }
            }
        }
        let html = dioxus::ssr::render_element(rsx! { Wrapper {} });
        assert!(
            html.contains("Type to search"),
            "expected hint copy, got: {html}"
        );
        assert!(!html.contains("<ul"), "no list element expected: {html}");
    }

    /// `loading=true` renders the "Searching…" placeholder.
    #[test]
    fn dropdown_loading_state_renders_placeholder() {
        #[component]
        fn Wrapper() -> Element {
            rsx! {
                SearchDropdown {
                    query: "vac".to_string(),
                    hits: Vec::<SearchHitDto>::new(),
                    loading: true,
                    error: None,
                }
            }
        }
        let html = dioxus::ssr::render_element(rsx! { Wrapper {} });
        assert!(
            html.contains("search-dropdown-loading"),
            "expected loading class, got: {html}"
        );
        assert!(
            html.contains("Searching"),
            "expected loading copy, got: {html}"
        );
        assert!(!html.contains("<ul"), "no list element expected: {html}");
    }

    /// Hits render with basename, path, navigable href, and a wrapping
    /// `<ul class="search-dropdown-list">`. Locks the rendered shape so
    /// a class-name or copy change is a visible diff.
    #[test]
    fn dropdown_renders_hits_with_basename_path_and_href() {
        let hits = vec![
            hit(1, "vacation.jpg", "/photos/vacation.jpg", "image/jpeg"),
            hit(
                2,
                "report.docx",
                "/docs/report.docx",
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            ),
        ];
        #[component]
        fn Wrapper(hits: Vec<SearchHitDto>) -> Element {
            rsx! {
                SearchDropdown {
                    query: "v".to_string(),
                    hits,
                    loading: false,
                    error: None,
                }
            }
        }
        let html = dioxus::ssr::render_element(rsx! { Wrapper { hits } });
        assert!(
            html.contains("search-dropdown-list"),
            "expected wrapping list element, got: {html}"
        );
        assert!(
            html.contains("vacation.jpg"),
            "expected first basename: {html}"
        );
        assert!(
            html.contains("/photos/vacation.jpg"),
            "expected first path: {html}"
        );
        assert!(
            html.contains("/apps/files/photos"),
            "expected first href to parent dir: {html}"
        );
        assert!(
            html.contains("report.docx"),
            "expected second basename: {html}"
        );
        assert!(
            html.contains("/apps/files/docs"),
            "expected second href to parent dir: {html}"
        );
        // Image-mime icon present (the docx falls through to the
        // generic file icon, which is shared with text/plain).
        assert!(html.contains("🖼"), "expected image icon: {html}");
    }

    /// `query` non-empty, `hits` empty, not loading → "No matches.".
    #[test]
    fn dropdown_empty_hits_renders_no_matches() {
        #[component]
        fn Wrapper() -> Element {
            rsx! {
                SearchDropdown {
                    query: "asdfqwerty".to_string(),
                    hits: Vec::<SearchHitDto>::new(),
                    loading: false,
                    error: None,
                }
            }
        }
        let html = dioxus::ssr::render_element(rsx! { Wrapper {} });
        assert!(
            html.contains("No matches."),
            "expected empty-results copy, got: {html}"
        );
        assert!(!html.contains("<ul"), "no list element expected: {html}");
    }

    /// An `error` value renders inside a `role="alert"` paragraph and
    /// suppresses every other branch (hint, hits, loading).
    #[test]
    fn dropdown_error_branch_renders_alert() {
        #[component]
        fn Wrapper() -> Element {
            rsx! {
                SearchDropdown {
                    query: "v".to_string(),
                    hits: Vec::<SearchHitDto>::new(),
                    loading: false,
                    error: Some("Search failed: boom".to_string()),
                }
            }
        }
        let html = dioxus::ssr::render_element(rsx! { Wrapper {} });
        assert!(
            html.contains("role=\"alert\""),
            "expected role=alert on error banner, got: {html}"
        );
        assert!(
            html.contains("Search failed: boom"),
            "expected error message: {html}"
        );
        assert!(
            !html.contains("No matches."),
            "error suppresses empty branch: {html}"
        );
    }
}
