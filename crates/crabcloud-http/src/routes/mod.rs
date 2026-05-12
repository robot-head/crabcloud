//! HTTP route modules. UI rendering and the legacy login/status endpoints
//! now live as Dioxus `#[server]` functions in `crabcloud-ui` — only the
//! OCS REST + WebDAV surfaces remain here.

pub mod dav;
pub mod ocs;
