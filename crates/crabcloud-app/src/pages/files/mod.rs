//! Files web UI — `/apps/files/<path>`. The browser-facing app for browsing,
//! reading, and writing the user's home storage. See
//! `docs/superpowers/specs/2026-05-12-files-web-ui-design.md`.

pub mod breadcrumb;
pub mod chrome;
pub mod delete_modal;
pub mod list;
pub mod mkdir_row;
pub mod path;
pub mod progress_strip;
pub mod row;
pub mod search_bar;
pub mod share_modal;
pub mod states;
pub mod toolbar;
pub mod upload;
pub mod versions_panel;

#[cfg(feature = "server")]
pub mod ssr;

use crate::context::RequestContext;
use crate::pages::files::breadcrumb::Breadcrumb;
use crate::pages::files::delete_modal::DeleteModal;
use crate::pages::files::list::FileList;
use crate::pages::files::mkdir_row::MkdirRow;
use crate::pages::files::progress_strip::UploadProgressStrip;
use crate::pages::files::share_modal::ShareModal;
use crate::pages::files::toolbar::Toolbar;
use crate::pages::files::upload::{DropOverlay, JobState, UploadQueue};
use crate::pages::files::versions_panel::VersionsPanel;
use crate::server_fns::{delete, list_dir, mkdir, move_paths, rename, FileEntry};
#[cfg(target_arch = "wasm32")]
use dioxus::html::HasFileData;
use dioxus::prelude::*;
use std::collections::HashSet;

#[derive(Clone, PartialEq)]
pub struct Clipboard {
    pub source_dir: String,
    pub paths: Vec<String>,
}

#[component]
pub fn Files(ctx: RequestContext, path: String) -> Element {
    // SSR-only: redirect anonymous visitors to login with redirect_url.
    #[cfg(feature = "server")]
    {
        let current_path = format!(
            "/apps/files{}",
            if path == "/" {
                String::new()
            } else {
                path.clone()
            }
        );
        if ssr::redirect_if_anonymous(&ctx.user_id, &current_path) {
            return rsx! { "" };
        }
    }

    let user_id = ctx.user_id.clone().unwrap_or_default();
    let mut path_sig = use_signal(|| path.clone());
    let mut refresh = use_signal(|| 0u64);
    let mut selection: Signal<HashSet<String>> = use_signal(HashSet::new);

    // Keep the route's `path` prop in sync with the signal. Back/forward
    // navigation re-renders this component with a new `path` prop; the
    // `use_reactive` adapter (Dioxus 0.7.9) re-fires the effect when the
    // captured non-reactive prop value changes. Selection is also cleared
    // because it's path-scoped; clipboard intentionally persists across
    // navigation so a user can cut here and paste in a different folder.
    use_effect(use_reactive((&path,), move |(p,)| {
        path_sig.set(p);
        selection.set(HashSet::new());
    }));

    // The list_dir server fn is only invoked from the client (wasm) side.
    // SSR renders the chrome + a pending skeleton; the resource starts in
    // the `Pending` state and is populated after hydration.
    let entries = use_resource(move || {
        let p = path_sig();
        let _ = refresh();
        async move { list_dir(p).await.map_err(|e| format!("{e}")) }
    });

    // Both Breadcrumb and FileList want to push a Files route for a target
    // path. `Navigator` is `Copy`, so we hand a fresh closure to each prop
    // instead of trying to alias one closure between two `EventHandler`
    // call-sites (which would move-once).
    let nav = use_navigator();
    let on_open_folder = move |target: String| {
        let segs = path::path_to_segments(&target);
        nav.push(crate::Route::FilesRoute { segments: segs });
    };
    let on_navigate_breadcrumb = move |target: String| {
        let segs = path::path_to_segments(&target);
        nav.push(crate::Route::FilesRoute { segments: segs });
    };
    let on_retry = move |_| refresh.set(refresh() + 1);

    // New mutation state — C5.
    let mut rename_target: Signal<Option<String>> = use_signal(|| None);
    let mut delete_target: Signal<Option<Vec<String>>> = use_signal(|| None);
    let mut mkdir_active = use_signal(|| false);
    // Share-modal target — SP7 E2/E3. When `Some(path)`, `ShareModal` is
    // mounted at the bottom of the page DOM (similar to `DeleteModal`).
    let mut share_path: Signal<Option<String>> = use_signal(|| None);
    // Versions panel target — SP13 D. When `Some((fileid, name))`, the
    // `VersionsPanel` modal is mounted at the bottom of the page DOM.
    // The filename is captured at click time so the header reads
    // sensibly even if the row's data shifts between open and close.
    let mut pending_versions: Signal<Option<(i64, String)>> = use_signal(|| None);

    // Cut/paste state — D3. The clipboard intentionally outlives a single
    // folder view so the user can navigate from source to destination
    // between cut and paste.
    let mut clipboard: Signal<Option<Clipboard>> = use_signal(|| None);

    // Upload state — E4. Queue holds per-file job records and is read by
    // the inline progress strip; drag_active toggles the drop overlay.
    let mut upload_queue: Signal<UploadQueue> = use_signal(UploadQueue::default);
    let mut drag_active = use_signal(|| false);

    let on_toggle_select = move |p: String| {
        let mut s = selection();
        if !s.insert(p.clone()) {
            s.remove(&p);
        }
        selection.set(s);
    };
    let on_rename_start = move |p: String| rename_target.set(Some(p));
    let on_rename_cancel = move |_| rename_target.set(None);
    let on_rename_commit = move |(from, new_name): (String, String)| {
        spawn(async move {
            let to = match from.rsplit_once('/') {
                Some(("", _)) => format!("/{new_name}"),
                Some((parent, _)) => format!("{parent}/{new_name}"),
                None => format!("/{new_name}"),
            };
            let _ = rename(from, to).await;
            rename_target.set(None);
            refresh.set(refresh() + 1);
        });
    };
    let on_delete = move |p: String| delete_target.set(Some(vec![p]));
    let on_share = move |p: String| share_path.set(Some(p));
    let on_show_versions =
        move |(fid, name): (i64, String)| pending_versions.set(Some((fid, name)));
    let on_delete_confirm = move |_| {
        if let Some(paths) = delete_target() {
            spawn(async move {
                let _ = delete(paths).await;
                delete_target.set(None);
                refresh.set(refresh() + 1);
            });
        }
    };
    let on_delete_cancel = move |_| delete_target.set(None);

    let on_mkdir_start = move |_| mkdir_active.set(true);
    let on_mkdir_cancel = move |_| mkdir_active.set(false);
    let on_mkdir_commit = move |name: String| {
        let parent = path_sig();
        let new_path = if parent == "/" {
            format!("/{name}")
        } else {
            format!("{parent}/{name}")
        };
        spawn(async move {
            let _ = mkdir(new_path).await;
            mkdir_active.set(false);
            refresh.set(refresh() + 1);
        });
    };

    // Cut/paste + bulk selection handlers — D3.
    let on_cut = move |_| {
        let s = selection();
        if !s.is_empty() {
            clipboard.set(Some(Clipboard {
                source_dir: path_sig(),
                paths: s.into_iter().collect(),
            }));
            selection.set(HashSet::new());
        }
    };
    let on_clear_selection = move |_| selection.set(HashSet::new());
    let on_clear_clipboard = move |_| clipboard.set(None);
    let on_delete_selection = move |_| {
        let s = selection();
        if !s.is_empty() {
            delete_target.set(Some(s.into_iter().collect()));
        }
    };
    let on_paste = move |_| {
        if let Some(cb) = clipboard() {
            let dest = path_sig();
            if dest == cb.source_dir {
                return;
            }
            spawn(async move {
                let _ = move_paths(cb.paths, dest).await;
                clipboard.set(None);
                refresh.set(refresh() + 1);
            });
        }
    };

    // Upload kick-off closure — E4. For each file: append a Queued job to
    // the queue, then spawn an async task that drives the per-file upload
    // through InProgress -> Completed/Failed. Wasm-only because
    // `web_sys::File` doesn't exist on the native (SSR) build; the native
    // stub below keeps callers compiling.
    let user_id_for_uploads = user_id.clone();
    #[cfg(target_arch = "wasm32")]
    let kick_upload = {
        let user_id = user_id_for_uploads;
        move |files: Vec<web_sys::File>| {
            let dest_dir = path_sig();
            for f in files {
                let name = f.name();
                let size = f.size() as u64;
                let dest_path = if dest_dir == "/" {
                    format!("/{name}")
                } else {
                    format!("{dest_dir}/{name}")
                };
                let id = upload_queue
                    .write()
                    .enqueue(name.clone(), size, dest_path.clone());
                let uid = user_id.clone();
                let dp = dest_path.clone();
                let f_clone = f.clone();
                spawn(async move {
                    upload_queue
                        .write()
                        .update(id, |j| j.state = JobState::InProgress { percent: 0 });
                    // `on_progress` must satisfy `Fn`; `Signal::write` is
                    // `&mut self` so closing over `upload_queue` and calling
                    // `.write()` infers `FnMut`. `write_unchecked` is the
                    // `&self`-shaped sibling — borrow checking still runs
                    // at runtime, and each call is brief enough that we
                    // can't reach a panic from the strip's read.
                    let on_progress = move |percent: u8| {
                        upload_queue
                            .write_unchecked()
                            .update(id, |j| j.state = JobState::InProgress { percent });
                    };
                    match crate::pages::files::upload::upload_one(uid, dp, f_clone, on_progress)
                        .await
                    {
                        Ok(()) => {
                            upload_queue
                                .write()
                                .update(id, |j| j.state = JobState::Completed);
                            refresh.set(refresh() + 1);
                        }
                        Err(reason) => {
                            upload_queue
                                .write()
                                .update(id, |j| j.state = JobState::Failed { reason });
                        }
                    }
                });
            }
        }
    };
    // Native build: no-op stub so the rest of the closures compile. Call
    // sites that touch `web_sys` types are all wasm-gated, so this body
    // is unreachable in practice. We hold onto `user_id_for_uploads`
    // inside the closure (rather than dropping it with `let _ = ...`) so
    // the closure is `Clone` but not `Copy`, matching the wasm shape; this
    // dodges a `clippy::clone_on_copy` lint at the call sites.
    #[cfg(not(target_arch = "wasm32"))]
    let kick_upload = {
        let _uid = user_id_for_uploads;
        move |_files: Vec<()>| {
            let _ = &_uid;
        }
    };

    // Trigger the hidden file input when the toolbar Upload button is
    // clicked. The actual file collection happens in the input's
    // onchange handler.
    let on_upload_click = move |_| {
        #[cfg(target_arch = "wasm32")]
        {
            use wasm_bindgen::JsCast;
            if let Some(input) = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.get_element_by_id("files-file-input"))
                .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
            {
                input.click();
            }
        }
    };

    let entries_view: Option<Result<Vec<FileEntry>, String>> = entries.read().clone();

    // `kick_upload` is consumed by two event handlers (file input change
    // and drop). Closures with `Signal` captures are `Clone` because
    // `Signal` is `Copy` and `user_id: String` is `Clone`; we clone once
    // per call site so each handler owns its copy.
    let kick_upload_input = kick_upload.clone();
    let kick_upload_drop = kick_upload;

    rsx! {
        div { class: "files-page",
            chrome::TopBar { ctx: ctx.clone() }
            div { class: "files-body",
                chrome::Sidebar {}
                main {
                    class: "files-main",
                    ondragover: move |evt| {
                        evt.prevent_default();
                        drag_active.set(true);
                    },
                    ondragleave: move |_| drag_active.set(false),
                    ondrop: move |evt| {
                        evt.prevent_default();
                        drag_active.set(false);
                        #[cfg(target_arch = "wasm32")]
                        {
                            let files = collect_files_from_event(&evt);
                            kick_upload_drop.clone()(files);
                        }
                        #[cfg(not(target_arch = "wasm32"))]
                        {
                            let _ = evt;
                            let _ = &kick_upload_drop;
                        }
                    },

                    input {
                        r#type: "file",
                        id: "files-file-input",
                        multiple: true,
                        style: "display: none",
                        onchange: move |evt| {
                            #[cfg(target_arch = "wasm32")]
                            {
                                let files = collect_web_files(&evt.files());
                                kick_upload_input.clone()(files);
                            }
                            #[cfg(not(target_arch = "wasm32"))]
                            {
                                let _ = evt;
                                let _ = &kick_upload_input;
                            }
                        },
                    }

                    Toolbar {
                        selection_count: selection().len(),
                        clipboard_count: clipboard().as_ref().map(|c| c.paths.len()).unwrap_or(0),
                        clipboard_source: clipboard().as_ref().map(|c| c.source_dir.clone()),
                        can_paste: clipboard()
                            .as_ref()
                            .map(|c| c.source_dir != path_sig())
                            .unwrap_or(false),
                        on_new_folder: on_mkdir_start,
                        on_upload: on_upload_click,
                        on_cut,
                        on_delete_selection,
                        on_clear_selection,
                        on_paste,
                        on_clear_clipboard,
                    }
                    UploadProgressStrip {
                        jobs: upload_queue().jobs.clone(),
                        on_cancel: move |id: u64| {
                            upload_queue.write().remove(id);
                        },
                        on_retry: move |id: u64| {
                            upload_queue.write().update(id, |j| j.state = JobState::Queued);
                        },
                    }
                    Breadcrumb {
                        path: path_sig(),
                        on_navigate: on_navigate_breadcrumb,
                    }
                    if mkdir_active() {
                        table { class: "files-table",
                            tbody {
                                MkdirRow { on_commit: on_mkdir_commit, on_cancel: on_mkdir_cancel }
                            }
                        }
                    }
                    FileList {
                        entries: entries_view,
                        user_id: user_id.clone(),
                        selection: selection(),
                        rename_target: rename_target(),
                        on_open_folder,
                        on_toggle_select,
                        on_rename_start,
                        on_rename_commit,
                        on_rename_cancel,
                        on_delete,
                        on_share,
                        on_show_versions,
                        on_retry,
                    }
                    DropOverlay { visible: drag_active(), current_folder: path_sig() }
                }
            }
            if let Some(paths) = delete_target() {
                DeleteModal {
                    paths,
                    on_cancel: on_delete_cancel,
                    on_confirm: on_delete_confirm,
                }
            }
            if let Some(p) = share_path() {
                ShareModal {
                    path: p,
                    on_close: move |_| share_path.set(None),
                }
            }
            if let Some((fid, name)) = pending_versions() {
                VersionsPanel {
                    fileid: fid,
                    filename: name,
                    on_close: move |_| pending_versions.set(None),
                }
            }
        }
    }
}

/// Drain a `Vec<FileData>` (returned by Dioxus form/drag events) into
/// `Vec<web_sys::File>`. The web platform's `NativeFileData` impl wraps
/// a real `web_sys::File`, exposed via `FileData::inner().downcast_ref`.
/// Files that fail to downcast (e.g. serialized/synthetic test data) are
/// silently dropped — there is no meaningful fallback for the upload
/// path.
#[cfg(target_arch = "wasm32")]
fn collect_web_files(files: &[dioxus::html::FileData]) -> Vec<web_sys::File> {
    files
        .iter()
        .filter_map(|f| f.inner().downcast_ref::<web_sys::File>().cloned())
        .collect()
}

/// Pull files out of a drag event. Dioxus 0.7 surfaces drop files via
/// `HasFileData::files()` on `DragData`, so we go through the same
/// downcast path as the file-input handler.
#[cfg(target_arch = "wasm32")]
fn collect_files_from_event(evt: &Event<DragData>) -> Vec<web_sys::File> {
    collect_web_files(&evt.files())
}
