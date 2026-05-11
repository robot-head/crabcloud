//! OCS sub-router under `/ocs/v2.php`.

pub mod capabilities;

use axum::routing::get;
use axum::Router;
use rustcloud_core::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/v2.php/cloud/capabilities", get(capabilities::handler))
}
