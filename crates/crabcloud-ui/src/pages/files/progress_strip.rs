//! Inline progress strip. Shows in-progress jobs with their percent + a
//! cancel button, queued-count summary, and failed jobs with Retry.

use crate::pages::files::upload::{JobState, UploadJob};
use dioxus::prelude::*;

#[component]
pub fn UploadProgressStrip(
    jobs: Vec<UploadJob>,
    on_cancel: EventHandler<u64>,
    on_retry: EventHandler<u64>,
) -> Element {
    if jobs.is_empty() {
        return rsx! { "" };
    }
    let queued = jobs
        .iter()
        .filter(|j| matches!(j.state, JobState::Queued))
        .count();
    rsx! {
        div { class: "files-progress",
            for job in jobs.iter().filter(|j| matches!(j.state, JobState::InProgress { .. })) {
                {
                    let percent = match job.state {
                        JobState::InProgress { percent } => percent,
                        _ => 0,
                    };
                    let id = job.id;
                    let name = job.name.clone();
                    rsx! {
                        div { class: "files-progress-row",
                            span { class: "files-progress-icon", "⬆" }
                            div { class: "files-progress-body",
                                div { class: "files-progress-name", "{name} · {percent}%" }
                                div { class: "files-progress-bar",
                                    div { class: "files-progress-fill", style: "width: {percent}%" }
                                }
                            }
                            button {
                                class: "files-progress-cancel",
                                onclick: move |_| on_cancel.call(id),
                                "Cancel"
                            }
                        }
                    }
                }
            }
            if queued > 0 {
                div { class: "files-progress-queued", "+ {queued} queued" }
            }
            for job in jobs.iter().filter(|j| matches!(j.state, JobState::Failed { .. })) {
                {
                    let reason = match &job.state {
                        JobState::Failed { reason } => reason.clone(),
                        _ => String::new(),
                    };
                    let id = job.id;
                    let name = job.name.clone();
                    rsx! {
                        div { class: "files-progress-row files-progress-failed",
                            span { class: "files-progress-icon", "⚠" }
                            div { class: "files-progress-body",
                                div { class: "files-progress-name", "{name}" }
                                div { class: "files-progress-reason", "{reason}" }
                            }
                            button {
                                class: "files-progress-retry",
                                onclick: move |_| on_retry.call(id),
                                "Retry"
                            }
                        }
                    }
                }
            }
        }
    }
}
