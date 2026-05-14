# dx-built server consolidation — Design

**Status:** spec — design only, no implementation.
**Date:** 2026-05-14
**Sub-project:** infrastructure refactor between SP7 and SP8 (carryforward #1).
**Context:** During SP7 Batch F we discovered dx 0.7.9 changed `Asset::resolve()`'s default behavior such that non-dx-built server binaries render the manganis placeholder string in SSR HTML instead of either the absolute source path or the bundled URL. The existing `AssetRewriteLayer` was designed against the older "source path" behavior; it can't disambiguate the new placeholder. After investigation, the architecturally correct fix is to consolidate `crabcloud-server` into the dx-built crate so dx's link-time asset substitution runs over our actual production binary.

## 1. Goal

After this sub-project, `dx build --release` produces a single binary at `target/dx/crabcloud-app/release/web/server.exe` that runs the full Crabcloud stack (Dioxus SSR + server fns + OCS REST + WebDAV + CLI subcommands), with dx's link-time asset substitution filling in proper hashed URLs in the SSR'd HTML directly. `dx serve` provides hot-reload dev. `AssetRewriteLayer` and `cargo build -p crabcloud-server` go away.

**In scope:**

- Rename `crates/crabcloud-ui` → `crates/crabcloud-app`.
- Delete `crates/crabcloud-server`; fold its `main.rs`, `cli.rs`, `telemetry.rs` and deps into `crabcloud-app`.
- Wire `dioxus_cli_config::fullstack_address_or_localhost()` into `Cmd::Serve` so `dx serve` sets the bind address.
- Remove `crates/crabcloud-http/src/middleware/asset_rewrite.rs` and its wiring.
- Update CI: drop the standalone `build-wasm` job; the `e2e` job uses `dx build --release` then the dx-built binary.
- Verify `dx serve` works end-to-end and document the dev workflow.

**Explicitly out of scope:**

- The other two SP7 carryforwards (filecache poisoning in `SharedSubrootStorage`; `SqlUserStore::lookup` decode bug on mysql/postgres). They stay carried forward; tackled after this lands.
- SP8 (public links). This consolidation unblocks clean screenshots in SP8 but is its own sub-project.
- Internal changes to library crates (`crabcloud-core`, `crabcloud-http`, `crabcloud-fs`, etc.) — only their `crabcloud-ui` → `crabcloud-app` references update.
- Workspace cleanup unrelated to the rename (dep audits, module reshuffles, etc.).

## 2. Load-bearing decisions (brainstormed)

| # | Decision | Rationale |
|---|---|---|
| 1 | **Rename `crabcloud-ui` → `crabcloud-app`.** | The crate's role expands from "UI components" to "the whole binary entrypoint plus UI". The name should reflect the new responsibility. Cleaner long-term than leaving the misleading name behind. |
| 2 | **Delete `crates/crabcloud-server`** entirely; absorb its contents into `crabcloud-app`. | Single crate = single dx target. dx 0.7's model is one package, two outputs (WASM + native). Keeping a separate `crabcloud-server` would require either an unsupported dx mode or duplicate code. |
| 3 | **Library crates unchanged.** `crabcloud-core`, `crabcloud-http`, `crabcloud-fs`, `crabcloud-sharing`, etc. only get their `crabcloud-ui` → `crabcloud-app` dep + use-path renames. | They aren't the dx target; their layout is fine. Limits blast radius. |
| 4 | **`Cmd::Serve` reads bind address from `dioxus_cli_config::fullstack_address_or_localhost()` when `PORT` or `IP` is set in the env**; otherwise falls back to the config-file `bind_address`. | Makes `dx serve` work without breaking the production path (env vars not set → config wins, identical to today). |
| 5 | **Remove `AssetRewriteLayer` outright** once the consolidation lands. No deprecation shim. | Its purpose was to translate source paths → hashed URLs in non-dx-built binaries. After consolidation, dx fills in the proper URL at link time. Keeping the layer would be dead code that confuses future readers. |
| 6 | **CI's `build-wasm` job folds into `e2e`.** | `dx build --release` produces both the WASM bundle and the server binary in one invocation; a separate WASM job is redundant. |
| 7 | **Workspace + library tests unchanged.** `cargo test --workspace` still runs everything. Only `crabcloud_ui::*` references in tests get the rename. | The consolidation is structural, not behavioral. Library APIs don't change. |
| 8 | **Production path is the dx-built binary.** `target/dx/crabcloud-app/release/web/server.exe`. `cargo build --release -p crabcloud-app` still works (it just produces a binary without the proper asset link section, useful for `cargo run` scripted scenarios but not for production). | Avoids forcing dx into every operational workflow. Pragmatic dual-mode. |

## 3. File structure after

```
crates/crabcloud-app/        (was crabcloud-ui; absorbs crabcloud-server's contents)
├── Cargo.toml               — merged deps; gains all of crabcloud-server's deps
├── Dioxus.toml              — unchanged
├── src/
│   ├── main.rs              — was crates/crabcloud-server/src/main.rs
│   ├── cli.rs               — was crates/crabcloud-server/src/cli.rs
│   ├── telemetry.rs         — was crates/crabcloud-server/src/telemetry.rs
│   ├── lib.rs               — was crates/crabcloud-ui/src/lib.rs; re-exports for WASM target + tests
│   ├── app.rs               — Dioxus root + routes (unchanged from crabcloud-ui)
│   ├── pages/               — UI components (unchanged)
│   ├── server_fns.rs        — server fns (unchanged)
│   └── …                    — everything else from crates/crabcloud-ui/src/
├── assets/
│   └── app.css              — unchanged
└── tests/                   — was crates/crabcloud-ui/tests/; targets renamed to crabcloud_app::*

crates/crabcloud-server/     — DELETED
```

Other workspace crates' `Cargo.toml` entries change `crabcloud-ui` → `crabcloud-app`; every `use crabcloud_ui::*` becomes `use crabcloud_app::*`. The workspace root `Cargo.toml` updates the `members` list and the `[workspace.dependencies]` alias.

## 4. dx-serve wiring

`Cmd::Serve` in `crates/crabcloud-app/src/main.rs`:

```rust
Cmd::Serve => {
    let mut config = crabcloud_config::load(&cli.config, &[])?;
    // dx serve sets `IP` + `PORT` env vars before launching the binary. Honor
    // them when present so HMR ws / asset reload works against the right
    // address; otherwise stick with what the config file declared. This keeps
    // production (no env vars) untouched.
    if std::env::var("PORT").is_ok() || std::env::var("IP").is_ok() {
        config.bind_address = dioxus_cli_config::fullstack_address_or_localhost();
    }
    // …rest of the existing serve body
}
```

dx serve's hot-module-reload WebSocket and its asset reload requests both arrive at the same axum router we already build, which serves the public dir and the SSR + server-fn routes. No additional plumbing needed.

## 5. CI changes

Current e2e workflow:

```yaml
build-wasm:                # standalone WASM bundle job
  - cargo install dioxus-cli ...
  - dx build --release --platform web
  - upload artifact

e2e:
  needs: build-wasm
  - cargo build --release -p crabcloud-server
  - download dx-public artifact
  - DIOXUS_PUBLIC_PATH=…/public ./target/release/crabcloud-server serve
```

After consolidation:

```yaml
e2e:
  - cargo install dioxus-cli --version "^0.7" --locked
  - dx build --release   # one invocation; produces target/dx/crabcloud-app/release/web/{server.exe,public/}
  - <bcrypt + fixture config steps, unchanged>
  - target/dx/crabcloud-app/release/web/server.exe --config config/e2e.toml migrate
  - target/dx/crabcloud-app/release/web/server.exe …user-add --password-stdin   (unchanged shape)
  - target/dx/crabcloud-app/release/web/server.exe --config config/e2e.toml serve &
  - <playwright steps>
```

The `build-wasm` job is removed; its concerns fold into the e2e job. The `DIOXUS_PUBLIC_PATH` env var goes away — the dx-built server knows where its assets live.

## 6. Risks + open questions

1. **dx-built binaries with our dep tree** — sqlx, axum, tower, hyper, testcontainers, the whole stack. dx 0.7's custom linker has been validated mostly on smaller demo projects. Concrete risk: some proc-macro / build-script combination might not survive dx's link wrapper. **Mitigation:** the §7 smoke test gates the full restructure.
2. **`dx serve`'s relationship with non-Dioxus routes.** dx serve launches the binary and the binary owns HTTP routing; this should be transparent for our custom OCS / WebDAV / DAV routes. But the dev server intercepts some requests (HMR ws, asset hot-swap) and behavior at the edges (e.g. our `OCS-APIRequest` header bypass) is unverified. **Mitigation:** verify in Batch E with explicit `dx serve` smoke checks; if HMR ws collides with our CSP, document a workaround.
3. **`cargo test` for `crabcloud-app`.** The crate has WASM-target-only and native-target-only code paths. The current setup uses feature gates (`server` vs `web`); the consolidation might surface a code path that wasn't previously native-built (e.g., the new `Cmd::Serve` body depends on `crabcloud-core`, but the rest of the binary code was already there). **Mitigation:** `cargo test --workspace` + `cargo check -p crabcloud-app --target wasm32-unknown-unknown --features web` both run in CI.
4. **Local dev fallback.** `cargo run -p crabcloud-app -- serve --config X` should still work for non-dx workflows (CI bootstrap, scripted runs, debugging in IDEs). It won't have the proper asset link section — same caveat as today's `cargo run -p crabcloud-server`. **Mitigation:** documented as expected behavior; production / e2e always use the dx-built binary.
5. **Workspace lint anchors.** `use foo as _;` patterns sprinkled across crates anchor deps that aren't directly used per target. Some of these will move or change with the consolidation. **Mitigation:** `cargo clippy --workspace --all-targets -- -D warnings` is the gate; fix as flagged.
6. **`cargo install dioxus-cli` lockfile.** CI uses `--locked` to pin transitive deps. dx 0.7.x patch releases could still introduce regressions (we just saw one). **Mitigation:** out of scope; same risk we have today.

## 7. Spec-driven smoke test (gate before the full restructure)

Before committing to the full restructure, validate the riskiest assumption: that `dx build --release` actually succeeds on a crate the size of `crabcloud-app` with the full workspace dep tree.

**Procedure (throwaway branch, time-boxed to 4 hours):**

1. Branch `sp-app-consolidation/smoke` off `origin/master`.
2. Make `crabcloud-server` itself a candidate for dx by adding the dx app entry-point shape to it minimally. `Cmd::Serve` already merges `dioxus::server::router(crabcloud_ui::App)` into `build_router`, so the binary's runtime shape is already dx-compatible; what's missing is the dx build metadata:
   - Add `dioxus = { workspace = true, features = ["fullstack"] }` to `crates/crabcloud-server/Cargo.toml` as a direct dep (it currently pulls it in transitively through `crabcloud-ui`).
   - Add a minimal `Dioxus.toml` at `crates/crabcloud-server/Dioxus.toml` declaring `[application] name = "crabcloud-server"` and `[web.app] default_platform = "web"`.
3. From `crates/crabcloud-server/`: run `dx build --release`. Observe whether dx successfully wraps the linker against our dep tree.
4. If the build completes, run the resulting binary: `target/dx/crabcloud-server/release/web/server.exe --config config/e2e.toml serve` (with an e2e-shaped config). Confirm:
   - `curl /apps/files/` returns a 303 to `/login` (anonymous redirect from SP7).
   - `curl /login` returns SSR HTML.
   - The `<link rel="stylesheet">` href in that HTML is `/assets/<hash>.css` (NOT the placeholder string).
   - `target/dx/.../server.exe --config X migrate` exits cleanly.
   - `echo pw | target/dx/.../server.exe --config X user-add foo --password-stdin` creates a user.

**Decision rule:**

- **All four checks pass** → proceed with the full restructure (§8 batches). Document the smoke-test result in the marker PR for Batch A.
- **Any check fails** → revise this spec. Likely fallback is the one-liner `CARGO_MANIFEST_DIR` fix in `crabcloud-server::main()` plus a regression test that asserts the SSR `<link href>` matches `/assets/<hash>.css`. The architectural debt stays logged for a future attempt.

The smoke test PR itself is thrown away after the decision is recorded; only the marker PR's text persists.

## 8. Batches

Five batches. Concrete tasks live in the implementation plan (writing-plans is the next step); the table here is the shape.

- **Batch A — Smoke test + go/no-go.** Execute §7. Land a small marker PR (probably under 50 lines, mostly a CHANGELOG-style note) recording the outcome. If the smoke fails, this is also where the spec gets revised and the project pivots back to the one-liner fix.
- **Batch B — Mechanical rename `crabcloud-ui` → `crabcloud-app`.** No behavior change. Touches every workspace crate's Cargo.toml + every `use crabcloud_ui::*`. Single commit; squash-merge.
- **Batch C — Fold `crabcloud-server` into `crabcloud-app`.** Move `main.rs`, `cli.rs`, `telemetry.rs`; merge deps; delete the old crate. After this batch the workspace has one binary crate (was two). Verify `cargo run -p crabcloud-app -- migrate` / `user-add` / `serve` all work.
- **Batch D — CI cutover + remove `AssetRewriteLayer`.** Update `.github/workflows/ci.yml`: drop `build-wasm` job, e2e uses `dx build --release` + dx-built binary path. Delete `crates/crabcloud-http/src/middleware/asset_rewrite.rs` and its wiring in `routes/mod.rs`. Confirm CI green. This is the batch where the asset-rendering issue is fundamentally resolved.
- **Batch E — `dx serve` + dev docs.** Wire `dioxus_cli_config::fullstack_address_or_localhost()` into `Cmd::Serve`. Update `README.md` with the new dev workflow (`dx serve` for hot-reload, `dx build --release` for prod). Optionally add an `xtask dev` shortcut. Manual verification that `dx serve` works.

## 9. Acceptance criteria

- `dx build --release` from the workspace root produces `target/dx/crabcloud-app/release/web/server.exe` and `target/dx/crabcloud-app/release/web/public/` with hashed assets.
- That binary serves the full stack: Dioxus SSR, server fns, OCS REST, WebDAV, all CLI subcommands.
- SSR'd HTML's `<link rel="stylesheet">` href is `/assets/<hash>.css` — verified by a regression test in `crates/crabcloud-http/tests/` or similar that lives on in the test suite.
- `AssetRewriteLayer` is gone (`grep -r "AssetRewriteLayer" crates/` returns no hits).
- `crates/crabcloud-server/` no longer exists; nothing in the workspace references it.
- `cargo test --workspace` passes.
- `cargo clippy --workspace --all-targets -- -D warnings` passes.
- CI (fmt-and-clippy, test-sqlite, test-multidialect, e2e) is green. The `build-wasm` job is removed.
- `dx serve` from the workspace root launches the dev server with hot-reload; clicking a folder in the Files UI works end-to-end. Manually verified.
- README documents the dev + prod commands.

## 10. Carry-forward

- The other two SP7 carryforwards (filecache poisoning in `SharedSubrootStorage`; `SqlUserStore::lookup` decode bug). They stay queued.
- SP8 (public links) — proceed after this lands; should benefit from clean local screenshots once `AssetRewriteLayer` is gone and `dx serve` works.
