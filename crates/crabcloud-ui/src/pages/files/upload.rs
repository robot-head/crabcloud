//! Upload state machine + drop overlay. Small files PUT to
//! `/dav/files/<user><dest>`; larger files take the chunked path:
//! `upload_begin` server fn -> N PUTs to `/dav/uploads/<user>/<id>/<n>` ->
//! MOVE commit. See `docs/superpowers/specs/2026-05-12-files-web-ui-design.md`
//! section 7.
//!
//! The actual job-pump (deciding which jobs are running, dispatching them,
//! writing back into the shared queue) lives in the Files page; this module
//! owns the data shape (`UploadJob` / `UploadQueue`), the per-job network
//! state machine (`upload_one`), and the visual drop target (`DropOverlay`).

use dioxus::prelude::*;
#[cfg(target_arch = "wasm32")]
use serde::{Deserialize, Serialize};

/// Files <= this size go via a single `PUT /dav/files/...`. Above this we
/// fall through to the chunked path. Matches Nextcloud's default and the
/// `webdav` batch G server tests' single-PUT acceptance window.
pub const SINGLE_PUT_MAX: u64 = 8 * 1024 * 1024;

/// Chunk size for the multi-part path. The MOVE commit handler on the
/// server tolerates any chunk size; 16 MiB is a reasonable browser
/// tradeoff between request overhead and per-chunk memory.
pub const CHUNK_SIZE: u64 = 16 * 1024 * 1024;

/// Per-job lifecycle state. The dispatcher walks `Queued` jobs to
/// `InProgress`, then either `Completed` or `Failed`. Terminal states are
/// kept in the queue (and rendered in the upload tray) until the user
/// dismisses them — they are not auto-removed on success so the user can
/// see the result of an upload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobState {
    Queued,
    InProgress { percent: u8 },
    Completed,
    Failed { reason: String },
}

/// One file's worth of upload state. `id` is a monotonic counter assigned
/// by `UploadQueue::enqueue` — distinct from anything the server hands
/// back so the UI can refer to a job before the network round-trip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadJob {
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub dest_path: String,
    pub state: JobState,
}

/// Append-then-mutate-in-place queue used by the Files page. The
/// dispatcher reads `jobs`, drives the per-file state machine, and calls
/// back into `update` to write percent / terminal state. `next_id` is
/// stable across enqueues so an `id` once handed out keeps identifying
/// the same job for its full lifetime.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UploadQueue {
    next_id: u64,
    pub jobs: Vec<UploadJob>,
}

impl UploadQueue {
    /// Add a new job in the `Queued` state and return its id.
    pub fn enqueue(&mut self, name: String, size: u64, dest_path: String) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.jobs.push(UploadJob {
            id,
            name,
            size,
            dest_path,
            state: JobState::Queued,
        });
        id
    }

    /// Apply `f` to the job with the given `id` if it still exists. Silent
    /// no-op otherwise — the dispatcher may race against the user clearing
    /// the queue, and we'd rather drop the late update than panic.
    pub fn update<F: FnOnce(&mut UploadJob)>(&mut self, id: u64, f: F) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
            f(job);
        }
    }

    /// Drop the job with `id`. Used by the "dismiss" affordance in the
    /// upload tray.
    pub fn remove(&mut self, id: u64) {
        self.jobs.retain(|j| j.id != id);
    }
}

/// Drag-and-drop visual. Rendered absolutely over `.files-main` while a
/// drag is in flight; styled to be obviously a drop target without
/// stealing pointer events (the underlying drop handler is on the main
/// element).
#[component]
pub fn DropOverlay(visible: bool, current_folder: String) -> Element {
    if !visible {
        return rsx! {};
    }
    rsx! {
        div { class: "files-drop-overlay",
            div { class: "files-drop-target",
                div { class: "files-drop-icon", "⬇" }
                div { class: "files-drop-title",
                    "Drop to upload to "
                    em { "{current_folder}" }
                }
            }
        }
    }
}

/// Wire-format of a single committed part, surfaced to the server via the
/// `X-Crabcloud-Part-Tags` header on the MOVE commit. Mirrors the shape
/// the batch G server tests use. wasm-only — the native build doesn't
/// run the chunked uploader.
#[cfg(target_arch = "wasm32")]
#[derive(Serialize, Deserialize, Clone, Debug)]
struct PartTagJson {
    part_number: u32,
    etag: String,
}

/// Drive a single file through whichever upload path its size demands.
/// Calls `on_progress(percent)` after the small-file PUT lands or after
/// each chunk in the chunked path; the page wires this to the per-job
/// `JobState::InProgress { percent }` update. Returns `Err(reason)` on
/// any HTTP / transport error so the caller can transition the job to
/// `Failed`.
#[cfg(target_arch = "wasm32")]
pub async fn upload_one(
    user_id: String,
    dest_path: String,
    file: web_sys::File,
    on_progress: impl Fn(u8) + 'static,
) -> Result<(), String> {
    use gloo_net::http::Request;
    use wasm_bindgen::JsValue;
    use web_sys::Blob;

    let size = file.size() as u64;
    let dest_url = format!("/dav/files/{user_id}{dest_path}");

    if size <= SINGLE_PUT_MAX {
        // `web_sys::File` inherits from `Blob`; the `JsValue` representation
        // is the same DOM object. We hand a clone of the JsValue to
        // gloo-net's body() (which takes `impl Into<JsValue>`).
        let body: JsValue = (&file as &JsValue).clone();
        let resp = Request::put(&dest_url)
            .header("ocs-apirequest", "true")
            .body(body)
            .map_err(|e| format!("build: {e}"))?
            .send()
            .await
            .map_err(|e| format!("net: {e}"))?;
        if !resp.ok() {
            return Err(format!("PUT {} -> {}", dest_url, resp.status()));
        }
        on_progress(100);
        return Ok(());
    }

    // Chunked path. `upload_begin` allocates a server-side staging area
    // and hands back an id we PUT chunks against.
    let begin = crate::server_fns::upload_begin(dest_path.clone())
        .await
        .map_err(|e| format!("upload_begin: {e}"))?;
    let upload_id = begin.upload_id;
    let part_count = u32::try_from(size.div_ceil(CHUNK_SIZE)).map_err(|e| format!("parts: {e}"))?;
    let mut tags: Vec<PartTagJson> = Vec::with_capacity(part_count as usize);
    let blob: &Blob = file.as_ref();
    for n in 1..=part_count {
        let start = u64::from(n - 1) * CHUNK_SIZE;
        let end = (start + CHUNK_SIZE).min(size);
        let slice = blob
            .slice_with_f64_and_f64(start as f64, end as f64)
            .map_err(|e| format!("slice: {e:?}"))?;
        let url = format!("/dav/uploads/{user_id}/{upload_id}/{n}");
        let body: JsValue = (&slice as &JsValue).clone();
        let resp = Request::put(&url)
            .header("ocs-apirequest", "true")
            .body(body)
            .map_err(|e| format!("build: {e}"))?
            .send()
            .await
            .map_err(|e| format!("net: {e}"))?;
        if !resp.ok() {
            return Err(format!("PUT part {n} -> {}", resp.status()));
        }
        let etag = resp
            .headers()
            .get("etag")
            .ok_or_else(|| "missing etag".to_string())?
            .trim_matches('"')
            .to_string();
        tags.push(PartTagJson {
            part_number: n,
            etag,
        });
        on_progress(((end as f64 / size as f64) * 100.0) as u8);
    }
    let tags_json = serde_json::to_string(&tags).map_err(|e| format!("tags: {e}"))?;
    let commit_url = format!("/dav/uploads/{user_id}/{upload_id}/.file");
    let dest_for_move = format!("/dav/files/{user_id}{dest_path}");
    let ok = move_request(&commit_url, &dest_for_move, &tags_json).await?;
    if !ok {
        return Err("MOVE commit failed".to_string());
    }
    on_progress(100);
    Ok(())
}

/// Issue a WebDAV MOVE via `window.fetch` because gloo-net's
/// `Request::put`/`get`/etc. helpers don't expose arbitrary HTTP methods
/// and going through their lower-level builder still wraps in their
/// `Method` enum which doesn't model MOVE. Returns `Ok(true)` on a 2xx
/// response.
#[cfg(target_arch = "wasm32")]
async fn move_request(url: &str, destination: &str, part_tags: &str) -> Result<bool, String> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{Headers, Request, RequestInit, Response};

    let headers = Headers::new().map_err(|e| format!("headers: {e:?}"))?;
    headers
        .set("ocs-apirequest", "true")
        .map_err(|e| format!("hdr: {e:?}"))?;
    headers
        .set("destination", destination)
        .map_err(|e| format!("hdr: {e:?}"))?;
    headers
        .set("x-crabcloud-part-tags", part_tags)
        .map_err(|e| format!("hdr: {e:?}"))?;

    let opts = RequestInit::new();
    opts.set_method("MOVE");
    opts.set_headers(&headers);

    let req = Request::new_with_str_and_init(url, &opts).map_err(|e| format!("req: {e:?}"))?;
    let window = web_sys::window().ok_or_else(|| "no window".to_string())?;
    let resp_value = JsFuture::from(window.fetch_with_request(&req))
        .await
        .map_err(|e| format!("fetch: {e:?}"))?;
    let resp: Response = resp_value
        .dyn_into()
        .map_err(|_| "fetch returned non-Response".to_string())?;
    Ok(resp.ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enqueue_assigns_monotonic_ids() {
        let mut q = UploadQueue::default();
        let a = q.enqueue("a.txt".into(), 1, "/a.txt".into());
        let b = q.enqueue("b.txt".into(), 2, "/b.txt".into());
        assert_eq!(a, 0);
        assert_eq!(b, 1);
        assert_eq!(q.jobs.len(), 2);
        assert_eq!(q.jobs[0].state, JobState::Queued);
    }

    #[test]
    fn update_mutates_in_place() {
        let mut q = UploadQueue::default();
        let id = q.enqueue("a.txt".into(), 1, "/a.txt".into());
        q.update(id, |j| j.state = JobState::InProgress { percent: 42 });
        assert_eq!(q.jobs[0].state, JobState::InProgress { percent: 42 });
    }

    #[test]
    fn update_on_unknown_id_is_noop() {
        let mut q = UploadQueue::default();
        q.update(999, |j| j.state = JobState::Completed);
        assert!(q.jobs.is_empty());
    }

    #[test]
    fn remove_drops_job() {
        let mut q = UploadQueue::default();
        let a = q.enqueue("a.txt".into(), 1, "/a.txt".into());
        let b = q.enqueue("b.txt".into(), 2, "/b.txt".into());
        q.remove(a);
        assert_eq!(q.jobs.len(), 1);
        assert_eq!(q.jobs[0].id, b);
    }

    #[test]
    fn remove_unknown_is_noop() {
        let mut q = UploadQueue::default();
        q.enqueue("a.txt".into(), 1, "/a.txt".into());
        q.remove(999);
        assert_eq!(q.jobs.len(), 1);
    }

    #[test]
    fn ids_keep_advancing_after_remove() {
        let mut q = UploadQueue::default();
        let a = q.enqueue("a.txt".into(), 1, "/a.txt".into());
        q.remove(a);
        let b = q.enqueue("b.txt".into(), 2, "/b.txt".into());
        assert_eq!(b, 1, "ids must not be reused after removal");
    }
}
