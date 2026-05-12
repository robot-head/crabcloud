# Sub-project 2b (App passwords + Bearer/Basic auth) — Changelog

Completed: 2026-05-12

## What works

- `oc_authtoken` table — full upstream Nextcloud schema (17 columns) including columns reserved for E2E key pairs, scope, expires, password_invalid, remote_wipe. Multi-dialect migration 0003 covers SQLite, MySQL, Postgres.
- `crabcloud-users` gains `AuthToken`, `AuthTokenType { Session, AppPassword }`, `RawToken` (SecretString-wrapped 72-byte OsRng → 96-char URL-safe base64), and `hash_token(raw, secret)` (SHA-512 of `raw || secret`, hex-encoded).
- `TokenStore` async trait + multi-dialect `SqlTokenStore` + read-through `TokenAuthCache` (positive 30s TTL, negative 5s TTL, activity-bump rate-limit 30s).
- `AppPasswordService` façade: `mint(uid, login_name, name, kind, remember) -> (AuthToken, RawToken)`, `verify(raw) -> AuthToken`, `list`, `lookup_by_id`, `revoke`, `revoke_other_sessions`, `invalidate_all_for_user`.
- `UsersService.set_password` cascades `invalidate_all_for_user` so every token row is marked `password_invalid=1` when the primary password rotates (documented retry semantics on partial failure).
- `crabcloud-http::AuthContext` + `AuthMethod { Session, Bearer, Basic }` request extension.
- `AuthLayer` Tower middleware: tries Bearer → Basic → Cookie in order; header arms fail loud (401), cookie arm fails quiet (stale cookie → anonymous, so `/login` still reachable post-secret-rotation). Constant-time uid compare on Basic.
- `AuthenticatedUser` / `OptionalUser` / `AdminUser` extractors rewired to read `AuthContext` from request extensions.
- Browser cookies are now DB-authoritative — the cookie payload is the raw token, hashed for lookup against `oc_authtoken`. The cache layer keeps a hot read-through.
- `SessionLayer` slimmed: it owns CSRF / two-factor blob storage keyed by `token_id` and applies pending cookie mutations stashed by handlers via `PendingCookie::Set { raw_token, token_id, max_age_secs }` or `PendingCookie::Destroy`.
- `POST /index.php/login` mints an `AuthToken` of kind `Session`, sets the cookie to the raw token via `PendingCookie::Set`, persists csrf_token + two_factor_passed under the new row's id.
- `POST /index.php/login/v2` + `POST /index.php/login/v2/poll` Nextcloud client bootstrap server fns; `GET /index.php/login/v2/flow/<flow-id>` Dioxus page with Authorize button; `login_v2_authorize` server fn mints the AppPassword AuthToken and writes `{loginName, appPassword}` into the poll cache record.
- `GET /ocs/v2.php/core/getapppassword` (Session-only, 403 from Bearer/Basic) — mints a bridge app password for the cookie-authed user.
- `DELETE /ocs/v2.php/core/apppassword` (any auth method) — revokes the calling request's own token.
- `PUT /ocs/v2.php/cloud/user key=password` now: requires Session auth (403 from Bearer/Basic), verifies `currentpassword`, calls `set_password` (which cascades password_invalid), mints a fresh Session token for the caller, revokes other sessions, rotates CSRF, and swaps the cookie via `PendingCookie::Set`.
- CSRF middleware (`crabcloud-http::csrf`) gates only on `AuthMethod::Session`; Bearer/Basic skip. Defense-in-depth: rejects empty / whitespace-only tokens regardless of equality.
- Settings → Security Dioxus page at `/settings/security`: lists active tokens (Browser session vs App password), Revoke per row (except current), "Log out everywhere else", and a Create form that displays the new token once with a Dismiss button.
- 4 new `#[server]` fns gated to `AuthMethod::Session`: `list_app_passwords`, `create_app_password`, `revoke_app_password` (symmetric error for unknown vs not-yours ids), `destroy_other_sessions`.
- CLI subcommands on `crabcloud-server`: `app-password-add <uid> <name>` (prints `id=` and `token=`), `app-password-list <uid>` (tab-separated), `app-password-revoke <id>`.

## What's deferred

- OAuth2 server (`/apps/oauth2/api/v1/token`, RFC 6749 client registration) — sub-project 2d. Will land as a third `AuthTokenType` and a new `AuthMethod` arm in `AuthLayer`.
- 2FA framework — sub-project 2c. The `Session.two_factor_passed` flag exists; 2b sets it to true on every successful login. 2c will gate it.
- Token scopes (filesystem-only, etc.) — schema column `scope` exists but always-null in 2b. A future sub-project introduces `enum TokenScope` + per-endpoint enforcement.
- E2E encryption key pairs in `oc_authtoken` — schema columns exist; always-null in 2b. The E2E sub-project will populate them.
- Remote-wipe initiator — `remote_wipe` column exists; the auth path honors `remote_wipe=1` already, but admin endpoints to trigger a wipe ship later.
- Expired-token sweep — no background cron in 2b; rows linger until manual revoke or `password_invalid` cascade.
- Pre-2b session migration — existing cache-only sessions are invalidated on first deploy after 2b. Documented; users see one re-login.
- `oc_authtoken.password` column — reserved for the future mount-credentials sub-project; 2b always writes NULL.
- `remember` cookie behaviour — 2b stores the checkbox state on the row but the cookie still uses `SESSION_IDLE_TTL` (30 min). Longer Max-Age when remember=true is a UX follow-up.

## Known limitations

- `login_v2_poll`'s "not yet authorized" response is HTTP 500 rather than 404 — `ServerFnError` doesn't carry a status code. Nextcloud clients retry on any non-200 so wire-impact is nil; a tighter response code lands when a typed `LoginV2Error` enum with `IntoResponse` is introduced.
- Secret rotation invalidates every stored token (the hash mixes in `config.secret`). Operators rotating the secret must re-distribute app passwords. A future per-secret-version hash prefix can support lazy re-hash on first auth.
- The settings page's refresh-after-mutation logic is inlined at four call sites due to Dioxus 0.7's `FnMut`/`Clone` closure ergonomics. A future Dioxus upgrade can collapse to a single helper.
- `crabcloud-users` test harness pulls in `crabcloud-ui` (with the `server` feature) as a dev-dep to trigger `#[server]` fn registration. A future cleanup moves those integration tests into a dedicated `crates/crabcloud-e2e` crate to break the dev-dep on the UI crate.
- New `AppPassword` rows are unrate-limited via `getapppassword` and `create_app_password`. A cookie-authed attacker can rapid-mint hundreds of tokens. Rate-limiting lands as a follow-up.

## Acceptance status

| # | Criterion | Status |
|---|---|---|
| 1 | `cargo xtask check-all` clean against SQLite + MySQL + Postgres | OK (CI green on master) |
| 2 | `crabcloud-server user-add alice && app-password-add alice "DAV"` returns a token; `curl -u alice:<token>` → 200 on `/ocs/v2.php/cloud/user` | OK (covered by `crabcloud-users/tests/users_flow.rs::getapppassword_via_cookie_mints_bridge_token` end-to-end + CLI parse tests) |
| 3 | Same token via `Authorization: Bearer <token>` → 200 | OK (`crabcloud-http/tests/auth_layer.rs::bearer_with_minted_token_authenticates`) |
| 4 | Wrong-token Basic → 401; wrong-uid+right-token Basic → 401 | OK (`auth_layer.rs::bearer_with_unknown_token_returns_401`, `basic_uid_mismatch_returns_401`) |
| 5 | `POST /index.php/login` still works; cookie is now a raw token whose hash sits in `oc_authtoken` | OK (`users_flow.rs::get_self_returns_payload`, `put_self_password_change_destroys_other_sessions`) |
| 6 | `/login/v2` poll cycle: client receives a token after the user clicks Authorize | OK (`users_flow.rs::login_v2_full_cycle`) |
| 7 | Settings UI lists active tokens; revoke + re-list works; "Log out everywhere else" preserves current | OK at the wire-level (`users_flow.rs::delete_app_password_revokes_current_token`); pixel-perfect UI flow exercised by `e2e/tests/app_password.spec.ts` |
| 8 | `PUT /ocs/v2.php/cloud/user key=password` returns 403 when authenticated via Basic/Bearer | OK (`routes/ocs/user.rs::tests::put_self_password_change_via_bearer_is_403`) |
| 9 | `PUT /ocs/v2.php/cloud/user key=password` under cookie: marks every other token row `password_invalid=1` AND destroys their cache | OK (`crabcloud-users/src/service.rs::tests::set_password_cascades_invalidate_when_app_passwords_attached` + `routes/ocs/user.rs::tests::put_self_password_change_destroys_other_sessions`) |
| 10 | Playwright `hydration.spec.ts` still green | OK (CI) |
| 11 | `[workspace.lints]` `-D warnings` clean for `crabcloud-users` and `crabcloud-http` | OK (CI fmt-and-clippy) |
| 12 | `git grep -i rustcloud` empty | OK |
