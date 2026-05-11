//! axum router composition. `build_router(state)` returns the assembled
//! `Router` that the server binds to. Sub-routers are added one at a time
//! as Phase 3 tasks land them.

use axum::routing::get;
use axum::Router;
use rustcloud_core::AppState;

use crate::routes::status;

/// Build the full HTTP router for the application.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/status.php", get(status::handler))
        .with_state(state)
}
