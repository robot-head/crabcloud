# Smoke-test result — dx-built server consolidation

**Date:** 2026-05-14
**Outcome:** GO

## What was tested

`dx build --release --server` invoked against `crates/crabcloud-server` with
`dioxus = { workspace = true, features = ["fullstack"] }` on the direct dep
line (replacing the previous `features = ["server"]`) and a minimal
`Dioxus.toml` (`[application] name = "crabcloud-server"`,
`[web.app] default_platform = "web"`). The runtime shape of `crabcloud-server`
was already dx-compatible (`Cmd::Serve` in `crates/crabcloud-server/src/main.rs`
line 95 mounts `dioxus::server::router(crabcloud_ui::App)` into `build_router`);
only the build metadata was missing.

The smoke test exercised the riskiest assumption from §6 of the design spec:
that dx 0.7's custom linker can wrap the full server dependency tree (sqlx,
axum, tower, hyper, manganis, dioxus_fullstack, the works). It also verified
that dx's link-time substitution actually fixes the
`AssetRewriteLayer`-bypassing bug at the root: the SSR'd `<link
rel="stylesheet">` href.

## What was observed

`dx build --release --server` compiled 695 crates and finished cleanly in
roughly 5 minutes wall-clock on a warm cache, producing
`target/dx/crabcloud-server/release/web/server.exe` (22 MB) plus
`public/assets/app-dxha1c368aca845a77f.css`. The dx-built binary ran `migrate`
and `user-add --password-stdin` to completion, served `GET /status.php` with
`HTTP/1.1 200 OK`, and — the critical check — emitted SSR'd HTML at `GET
/login` containing exactly one stylesheet link:
`<link rel="stylesheet" href="/assets/app-dxha1c368aca845a77f.css" type="text/css"/>`.
That hashed URL also resolves to `HTTP/1.1 200 OK content-type: text/css` from
the public-asset handler, end-to-end.

Initial invocations with `dx build --release` and `dx build --release
--fullstack` both bailed with `Could not automatically detect target triple`;
explicitly passing `--server` was required. (For Batch C this means
`crabcloud-app` will need an analogous explicit platform flag in its dx
invocation — likely `--fullstack` once `default_platform` is set
appropriately.)

## Decision

Proceed with Batch B (mechanical rename). The full restructure is viable on
this codebase. dx 0.7's link-time machinery successfully wraps our full
production dep tree, manganis' `__LINK_SECTION` placeholder is rewritten to a
real `BundledAsset { bundled_path: "app-dxha1c368aca845a77f.css", ... }`, and
the SSR'd `<link>` href is correct without any `AssetRewriteLayer` involvement.
The fix is architectural and clean: dx owns the link, no middleware patches the
HTML.

## Evidence

### dx build tail

```
 16.28s  INFO Compiled [687/695]: crabcloud_fs
 16.87s  INFO Compiled [688/695]: crabcloud_users
 17.13s  INFO Compiled [689/695]: crabcloud_filecache
 17.19s  INFO Compiled [690/695]: sqlx_postgres
 17.59s  INFO Compiled [691/695]: reqwest
 22.58s  INFO Compiled [692/695]: dioxus_server
 31.77s  INFO Compiled [693/695]: crabcloud_http
 34.84s  INFO Compiled [694/695]: crabcloud_ui
 69.18s  INFO Compiled [695/695]: crabcloud-server
 69.21s  INFO Bundling app...
 69.74s  INFO Copying asset (1/1): C:\...\crates\crabcloud-ui\assets\app.css
 69.74s  INFO Client build completed successfully! 🚀 path="...\target\dx\crabcloud-server\release\web"
```

### Step 4 runtime checks (all passed)

```
$ server.exe --config config/smoke.toml migrate
... "message":"migrate complete" ...

$ echo hunter2 | server.exe --config config/smoke.toml user-add alice --password-stdin
... "message":"user created","uid":"alice","admin":false ...

$ curl -sI http://127.0.0.1:18765/status.php
HTTP/1.1 200 OK
content-type: application/json

$ curl -s http://127.0.0.1:18765/login | grep -o '<link rel="stylesheet"[^>]*>'
<link rel="stylesheet" href="/assets/app-dxha1c368aca845a77f.css" type="text/css"/>

$ curl -sI http://127.0.0.1:18765/assets/app-dxha1c368aca845a77f.css
HTTP/1.1 200 OK
content-type: text/css
accept-ranges: bytes
```
