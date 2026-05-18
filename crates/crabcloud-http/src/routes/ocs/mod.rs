//! OCS sub-router under `/ocs/v2.php`.

pub mod activity;
pub mod admin_groups;
pub mod admin_users;
pub mod app_password;
pub mod capabilities;
pub mod envelope;
pub mod files_sharing;
pub mod files_trashbin;
pub mod files_versions;
pub mod user;

use axum::routing::{delete, get, put};
use axum::Router;
use crabcloud_core::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .nest("/v2.php/apps/activity/api/v2", activity::router())
        .nest("/v2.php/apps/files_sharing/api/v1", files_sharing::router())
        .nest(
            "/v2.php/apps/files_trashbin/api/v1",
            files_trashbin::router(),
        )
        .nest(
            "/v2.php/apps/files_versions/api/v1",
            files_versions::router(),
        )
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
        .route(
            "/v2.php/cloud/users",
            get(admin_users::list_users).post(admin_users::create_user),
        )
        .route(
            "/v2.php/cloud/users/{uid}",
            get(admin_users::get_user)
                .put(admin_users::edit_user)
                .delete(admin_users::delete_user),
        )
        .route(
            "/v2.php/cloud/users/{uid}/enable",
            put(admin_users::enable_user),
        )
        .route(
            "/v2.php/cloud/users/{uid}/disable",
            put(admin_users::disable_user),
        )
        .route(
            "/v2.php/cloud/users/{uid}/groups",
            get(admin_users::list_user_groups)
                .post(admin_users::add_user_to_group)
                .delete(admin_users::remove_user_from_group),
        )
        .route(
            "/v2.php/cloud/groups",
            get(admin_groups::list_groups).post(admin_groups::create_group),
        )
        .route(
            "/v2.php/cloud/groups/{gid}",
            get(admin_groups::list_group_members).delete(admin_groups::delete_group),
        )
}
