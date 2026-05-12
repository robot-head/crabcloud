//! OCS sub-router under `/ocs/v2.php`.

pub mod app_password;
pub mod capabilities;
pub mod user;

use axum::routing::{delete, get};
use axum::Router;
use crabcloud_core::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v2.php/cloud/capabilities", get(capabilities::handler))
        .route(
            "/v2.php/cloud/user",
            get(user::get_self).put(user::put_self),
        )
        .route(
            "/v2.php/core/getapppassword",
            get(app_password::get_app_password),
        )
        .route(
            "/v2.php/core/apppassword",
            delete(app_password::delete_app_password),
        )
}
