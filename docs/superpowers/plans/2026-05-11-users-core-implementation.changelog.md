# Sub-project 2a (Core User Store) — Changelog

Completed: 2026-05-11

## What works

- `crabcloud-users` crate with `UserId` / `Email` / `GroupId` validating newtypes, `User` / `Group` records, `UsersError` (status + client-message mapping into `crabcloud-core::Error`).
- `UserStore` / `GroupStore` / `PreferenceStore` async traits + `Sql*` implementations (multi-dialect, hand-dispatched per pool variant).
- `PasswordVerifier` trait + `BcryptVerifier` with sentinel-hash constant-time fake-verify on lookup miss.
- `UsersService` façade — `verify`, `lookup`, `set_password`, `is_admin`, `groups_of`, `preferences`.
- `BootstrapAdminBackend` shim — synthesizes a virtual admin when `config.bootstrap_admin` is set; promotes-on-first-write into `oc_users` and the `admin` group, retiring itself.
- Phase 3's `Session` gains `two_factor_passed: bool` (always `true` in 2a; placeholder for sub-project 2c).
- `SessionStore::destroy_all_for` / `destroy_all_for_except` backed by an `instance_id:sessions_by_user:{uid}` side-index in cache.
- `/index.php/login` now consults `state.users.verify(...)` instead of the inline bootstrap_admin check.
- New OCS endpoints: `GET /ocs/v2.php/cloud/user` (self info; matches Nextcloud's `{id, display-name, email, groups, enabled, last-login}`), `PUT /ocs/v2.php/cloud/user` (self-service password/displayname/email; `currentpassword` required; password change kicks other devices).
- New `AdminUser` extractor (`AuthenticatedUser` + admin-group check).
- New CLI subcommands on `crabcloud-server`: `user-add`, `user-set-password`, `user-delete`, `group-add-member`, `group-remove-member`. Passwords prompted via `rpassword`.
- Migration 0002 creates `oc_users` + `oc_groups` + `oc_group_user` + `oc_preferences` per-dialect; seeds the `admin` group.

## What's deferred

- Admin OCS endpoints (`POST` / `PUT` / `DELETE /ocs/v2.php/cloud/users`) — own follow-up sub-project.
- Groups OCS endpoints (`/ocs/v2.php/cloud/groups`) — same.
- App passwords + Bearer/Basic auth — sub-project 2b.
- 2FA framework — sub-project 2c.
- OAuth2 server — sub-project 2d.
- LDAP backend — sub-project 2e.
- SAML backend — sub-project 2f.
- Password reset via email — needs mail-sending sub-project.
- Settings UI for self-service — needs the settings UI sub-project.
- Multi-backend composition (`CompositeUserStore`) — deferred to 2e.
- Sub-admins, group quotas, file-system mappings — long-tail.
- Case-insensitive `uid` matching — needs a generated column.
- Password strength policy — Nextcloud's `password_policy` app equivalent, future.
- Legacy password hash formats (sha1/sha256/argon2-via-PHP) — only bcrypt today.

## Known limitations

- MySQL email-uniqueness is enforced application-side (no partial unique index on the dialect).
- `sessions_by_user` index in `MemoryCache` grows linearly per user — fine single-node; revisit when the Redis cache backend lands.
- `BootstrapAdminBackend::set_display_name` / `set_email` return `ReadOnly` for the virtual admin — promote-then-set is a polish item.
- CLI `user-add` prompts password on stdin; no `--password-stdin` flag. Add when scripted provisioning is a real need.
- **Playwright E2E is currently broken on master from a pre-existing Dioxus 0.7 hashed-bundle hydration regression (introduced in PR #10 and not yet resolved). All in-process integration tests pass. Tracking PR #14 (draft) has a partial fix.**

## Acceptance status

| # | Criterion | Status |
|---|---|---|
| 1 | `cargo xtask check-all` against SQLite + MySQL + Postgres | OK |
| 2 | `crabcloud-server user-add alice --admin` + login succeeds | OK (covered by `crabcloud-users/tests/users_flow.rs::login_with_real_user_succeeds`) |
| 3 | `bootstrap_admin` + empty DB → admin logs in | OK (`BootstrapAdminBackend` tests + `state.rs::users_service_assembled_with_bootstrap_admin`) |
| 4 | Self-service password change against virtual admin promotes into DB | OK (`bootstrap_shim::tests::set_password_on_virtual_admin_promotes_to_db`) |
| 5 | Disabled user gets 401 | OK (`service::tests::verify_fails_for_disabled_user`) |
| 6 | `PUT /ocs/v2.php/cloud/user key=password` updates hash + kicks other sessions, keeps current | OK (`routes::ocs::user::tests::put_self_password_change_destroys_other_sessions`) |
| 7 | `GET /ocs/v2.php/cloud/user` returns the expected envelope | OK (`tests/users_flow.rs::get_self_returns_payload`) |
| 8 | Playwright E2E still passes | DEFERRED — pre-existing dx 0.7 hashed-bundle hydration regression on master (PR #14 draft). Sub-project 2a does not regress the bootstrap-admin shim path that the suite covers; resolution tracked separately. |
| 9 | `git grep -i rustcloud` empty | OK |
| 10 | `[workspace.lints]` `-D warnings` clean for `crabcloud-users` | OK |
