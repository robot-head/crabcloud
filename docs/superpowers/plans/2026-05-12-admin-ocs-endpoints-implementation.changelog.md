# Sub-project admin-ocs — Changelog

Completed: 2026-05-12

## What works

- 14 Nextcloud-compatible admin OCS endpoints, all gated by the `AdminUser` extractor:
  - `GET /ocs/v2.php/cloud/users?search=&limit=&offset=` — paginated user list with case-insensitive substring search on uid/displayname/email.
  - `POST /ocs/v2.php/cloud/users` — create user (validates uid, password, email, displayName, groups[] — rejects unknown groups before creating).
  - `GET /ocs/v2.php/cloud/users/{uid}` — full record (id, display-name, email, groups, enabled, last-login).
  - `PUT /ocs/v2.php/cloud/users/{uid}` — admin override on `key` ∈ {password, displayname, email}.
  - `DELETE /ocs/v2.php/cloud/users/{uid}` — cascades tokens, group memberships, preferences.
  - `PUT /ocs/v2.php/cloud/users/{uid}/enable` — flip enabled=true.
  - `PUT /ocs/v2.php/cloud/users/{uid}/disable` — flip enabled=false AND revoke all tokens (forced logout).
  - `GET/POST/DELETE /ocs/v2.php/cloud/users/{uid}/groups` — list/add/remove group memberships.
  - `GET /ocs/v2.php/cloud/groups?search=&limit=&offset=` — paginated group list.
  - `POST /ocs/v2.php/cloud/groups` — create group.
  - `GET /ocs/v2.php/cloud/groups/{gid}` — list members.
  - `DELETE /ocs/v2.php/cloud/groups/{gid}` — delete (rejects "admin" group).
- New `UserStore` trait methods: `list_users(filter)`, `exists_in_storage(uid)` (default impl + `BootstrapAdminBackend` override).
- New `GroupStore` trait method: `list_groups(filter)`.
- New `UsersService` façades: `disable_user(uid)`, `delete_user(uid)`.
- New `AppPasswordService::revoke_all_for_user(uid)` helper.
- Self-action guards prevent admin from deleting / disabling themselves, removing themselves from the admin group, or rotating their own password via the admin endpoint (must use self-service PUT /cloud/user).
- Structural guards prevent deletion of the `admin` group and prevent disable/remove-from-admin actions that would leave the cluster admin-less.
- Bootstrap virtual admin is invisible to all `{uid}`-path operations via `exists_in_storage`.
- Disable cascade closes the §6.6 auth-path gap from 2b for known callers: a disabled user's existing Bearer/Basic tokens 401 immediately.
- Every admin write emits a `tracing::info!(actor, action, target_uid|target_gid)` event.

## What's deferred

- Sub-admins (`/users/{uid}/subadmins` + per-group admin permission model).
- Quota management (`PUT /cloud/users/{uid}` with `key=quota`).
- Email verification on `PUT email`.
- Rate-limiting on admin write endpoints.
- LDAP/SAML "can this user be edited by OCS?" predicate (lands with those sub-projects).
- DB-backed audit log (we emit `tracing` events; no audit table).
- AuthLayer post-lookup re-check of `user.enabled` (would close a small race window where an admin disables a user during an in-flight Bearer request; today the cascade catches the next request).
- Additional `PUT /cloud/users/{uid}` keys (Nextcloud accepts `phone`, `address`, `website`, etc. — additive when needed).

## Known limitations

- `PUT /cloud/users/{uid}` with `key=password` cascades `password_invalid=1` on every token row owned by the target. The target user is logged out everywhere on the next auth attempt — which is the intended behavior for admin-driven resets, but admins should communicate this to the user.
- `POST /cloud/users` with `groups[]=...` is non-atomic: if the user creation succeeds but a follow-up `add_to_group` fails (transient DB error), the user is created with partial group membership. Operators should retry the create call (the user-create will then 409 `UidAlreadyExists`, signalling to switch to per-group POST `/users/{uid}/groups`).
- The list endpoints use SQL `LIKE %term%` — no full-text search, no fuzzy matching, no ranking.

## Acceptance status

| # | Criterion | Status |
|---|---|---|
| 1 | `cargo xtask check-all` clean against SQLite + MySQL + Postgres | OK (CI green) |
| 2 | Non-admin caller → 403 on every endpoint; anonymous → 401 | OK (`admin_users.rs::tests::{list_users_as_non_admin_returns_403, list_users_anonymous_returns_401}`) |
| 3 | POST creates / GET returns / DELETE cascades tokens + group_user + preferences | OK (`admin_users.rs::tests::delete_user_cascades_tokens_and_memberships` + e2e) |
| 4 | PUT /disable revokes all tokens immediately | OK (`admin_users.rs::tests::disable_user_revokes_tokens`) |
| 5 | Admin PUT key=password cascades target's tokens; admin's session unaffected | OK (`admin_users.rs::tests::admin_password_rotation_cascades_target_tokens`) |
| 6 | GET ?search=... matches uid + displayname + email; pagination | OK (`store::sql::tests::list_users_substring_search_matches_*`) |
| 7 | Self-delete / self-disable / self-remove-from-admin / self-password-via-admin → 400 | OK (`admin_users.rs::tests::{delete_self_returns_400, remove_self_from_admin_group_returns_400, admin_password_rotation_of_self_returns_400}`) |
| 8 | Bootstrap virtual admin invisible to all `{uid}`-path operations | OK (`admin_users.rs::tests::get_virtual_admin_returns_404` + `bootstrap_shim::tests::exists_in_storage_false_for_virtual_admin`) |
| 9 | POST groups / GET members / DELETE groups; DELETE admin → 400 | OK (`admin_groups.rs::tests::*`) |
| 10 | Playwright e2e `admin_ocs.spec.ts` green | OK (CI) |
| 11 | `-D warnings` lints clean for `crabcloud-users` + `crabcloud-http` | OK (CI fmt-and-clippy) |
| 12 | `git grep -i rustcloud` empty | OK |
