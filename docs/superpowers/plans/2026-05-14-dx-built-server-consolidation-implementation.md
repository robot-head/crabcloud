# dx-built server consolidation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Consolidate `crabcloud-server` into `crabcloud-app` (renamed from `crabcloud-ui`) so dx's link-time asset substitution runs over our actual production binary, eliminating `AssetRewriteLayer` and the cargo / dx hybrid that broke after dx 0.7.9 changed `is_bundled_app()`'s default.

**Architecture:** Rename the dx-app crate `crabcloud-ui` → `crabcloud-app`. Move `crabcloud-server`'s `main.rs` / `cli.rs` / `telemetry.rs` and its deps into `crabcloud-app`. Delete `crabcloud-server`. The single binary `crabcloud-app` is dx-built (`target/dx/crabcloud-app/release/web/server.exe`) and serves the full stack. Wire `dioxus_cli_config::fullstack_address_or_localhost()` so `dx serve` works for hot-reload dev.

**Tech Stack:** Rust 1.95, dx 0.7.9 (`dioxus-cli`), Dioxus 0.7 fullstack, axum, cargo workspaces. Builds on `crabcloud-app` (was `crabcloud-ui`), and folds in everything from `crabcloud-server`.

**Spec:** `docs/superpowers/specs/2026-05-14-dx-built-server-consolidation-design.md`

---

## Conventions for every batch

- **Branching:** Each batch is a separate PR off `origin/master`. At the start of each batch:
  ```bash
  cd "C:/Users/Matt Stone/git/crabcloud"
  git fetch origin master
  git switch -c app-consol/<batch-letter>-<slug> origin/master
  ```
  Slugs: `a-smoke`, `b-rename`, `c-fold`, `d-ci-cutover`, `e-dx-serve`.
- **Commit cadence:** Commit at every "Commit" step. Frequent, focused commits.
- **Pre-PR check (every batch except A):**
  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```
  All three must pass locally.
- **Open the PR:**
  ```bash
  git push -u origin app-consol/<batch-letter>-<slug>
  gh pr create --title "app-consol: batch <X> — <topic>" --body "$(cat <<'EOF'
  ## Summary
  - <one-line bullets>

  ## Test plan
  - [ ] CI green (fmt-and-clippy, build-wasm, test-sqlite, test-multidialect, e2e)
  - [ ] <batch-specific manual checks>

  🤖 Generated with [Claude Code](https://claude.com/claude-code)
  EOF
  )"
  ```
- **Merge:** After all checks pass: `gh pr merge --squash --delete-branch`.

---

## Batch A — Smoke test + go/no-go

**Branch:** `app-consol/a-smoke` off `origin/master`.
**Goal:** Validate the riskiest assumption from spec §6.1 — that dx's custom linker can wrap our full dep tree (sqlx, axum, tower, hyper, testcontainers, etc.). If `dx build --release` succeeds on a crate the size of the consolidated binary, the rest of the plan is mostly mechanical. If it fails, the project pivots back to the one-liner `CARGO_MANIFEST_DIR` fix.

**Time-boxed to 4 hours.** Spec §7 has the decision rule; this batch executes it.

### Task A1: Add dx-build metadata to `crabcloud-server`

**Files:**
- Modify: `crates/crabcloud-server/Cargo.toml`
- Create: `crates/crabcloud-server/Dioxus.toml`

`Cmd::Serve` already calls `dioxus::server::router(crabcloud_ui::App)` and merges it into `build_router`, so the runtime shape is already dx-compatible. We're only adding the dx *build metadata* it doesn't currently carry.

- [ ] **Step 1: Confirm the current binary already mounts the dioxus router**

```bash
grep -n "dioxus::server::router" crates/crabcloud-server/src/main.rs
```
Expected: a line in `Cmd::Serve` that looks like
```rust
let app_router = dioxus::server::router(crabcloud_ui::App);
let router = crabcloud_http::build_router(state.clone(), app_router);
```
If that's not present, STOP — the smoke test's premise is wrong and the spec needs revision.

- [ ] **Step 2: Add `dioxus` as a direct dep of `crabcloud-server`**

In `crates/crabcloud-server/Cargo.toml`, under `[dependencies]`, add:
```toml
dioxus = { workspace = true, features = ["fullstack"] }
```
(It's currently pulled in transitively through `crabcloud-ui`'s `server` feature; we need it as a direct dep so dx sees it during target detection — spec §7 step 2.)

- [ ] **Step 3: Create `Dioxus.toml`**

Create `crates/crabcloud-server/Dioxus.toml`:
```toml
[application]
name = "crabcloud-server"

[web.app]
default_platform = "web"
```

- [ ] **Step 4: `cargo check`**

```bash
cargo check -p crabcloud-server
```
Expected: PASS. (The Dioxus.toml file is dx CLI-only; cargo doesn't consume it.)

- [ ] **Step 5: Commit**

```bash
git add crates/crabcloud-server/Cargo.toml crates/crabcloud-server/Dioxus.toml
git commit -m "app-consol(a): smoke-test prep — add dx metadata to crabcloud-server"
```

### Task A2: Run `dx build --release` against `crabcloud-server`

- [ ] **Step 1: From the crate dir, invoke dx build**

```bash
cd crates/crabcloud-server
dx build --release
cd ../..
```

Time-box: 30 minutes. If the build is still running after 30 minutes (unlikely; SP6 wasm build took ~5 minutes), kill it — that's a finding worth recording.

**Possible outcomes:**

- **SUCCESS.** The build prints something like `Server build completed successfully! 🚀 path="…/target/dx/crabcloud-server/release/web"` and `target/dx/crabcloud-server/release/web/server.exe` exists. Proceed to Task A3.

- **FAILURE WITH LINKER ERROR.** dx's custom linker rejects something in our dep tree. Save the full output to `/tmp/dx-smoke.log` (or wherever convenient), summarize the error class in Task A4's marker note, and skip to Task A4 with the "no-go" decision.

- **FAILURE WITH COMPILE ERROR.** Something in our code doesn't compile under dx's harness (e.g. proc-macro context that dx doesn't pass through). Same as above — capture the error and skip to Task A4.

- **ANY OTHER FAILURE.** Treat as no-go; capture the error.

- [ ] **Step 2: Verify the artifacts exist**

(Only if Step 1 succeeded.)
```bash
test -f target/dx/crabcloud-server/release/web/server.exe && echo "binary exists"
test -d target/dx/crabcloud-server/release/web/public && echo "public dir exists"
test -f target/dx/crabcloud-server/release/web/public/assets/app.css 2>/dev/null \
  || ls target/dx/crabcloud-server/release/web/public/assets/ 2>&1 | head -5
```
Expected: binary exists; `public/` dir exists; a hashed CSS file under `public/assets/`.

- [ ] **Step 3: Save build output for the marker note**

Capture a short transcript of the dx output (warning lines + the success/failure summary), 20 lines max, into a temp file you can paste into the PR body in Task A4.

### Task A3: Verify the dx-built binary runs

(Only run if Task A2 succeeded.)

This task runs the dx-built binary against an existing test fixture and checks four specific behaviors per spec §7.

**Files (referenced, none created):**
- `config/e2e.toml` — must exist (CI creates it; for local smoke, you can re-use a previous local fixture or build a minimal one).

- [ ] **Step 1: Generate a fixture config if needed**

If `config/e2e.toml` doesn't exist locally (common — CI generates it on the fly), create a minimal one:
```bash
mkdir -p config
cat > config/e2e.toml <<'EOF'
instanceid     = "smoke"
secret         = "a-32-byte-or-longer-secret-key!"
passwordsalt   = "ps"
installed      = true
version        = "31.0.0.0"
versionstring  = "31.0.0"
dbtype         = "sqlite"
dbname         = "/tmp/smoke-e2e.db"
dbtableprefix  = "oc_"
datadirectory  = "/tmp/smoke-e2e-data"
trusted_domains = ["localhost", "127.0.0.1"]
loglevel       = "info"
bind_address   = "127.0.0.1:18765"
db_pool_max    = 4

[cache]
backend = "memory"
EOF
mkdir -p /tmp/smoke-e2e-data
```
On Windows, substitute `/tmp/` paths for `%TEMP%/` equivalents.

- [ ] **Step 2: Migrate**

```bash
./target/dx/crabcloud-server/release/web/server.exe --config config/e2e.toml migrate
```
Expected: exits 0 within ~5 seconds. Log line `migrate complete`.

- [ ] **Step 3: Create a user via the binary**

```bash
echo hunter2 | ./target/dx/crabcloud-server/release/web/server.exe --config config/e2e.toml user-add alice --password-stdin
```
Expected: exits 0. Log line `user created uid="alice" admin=false`.

If `user-add` doesn't accept `--password-stdin`, that's an SP7 batch F regression that needs investigating — the flag should already be there.

- [ ] **Step 4: Start the server in the background**

```bash
./target/dx/crabcloud-server/release/web/server.exe --config config/e2e.toml serve &
SERVER_PID=$!
sleep 3
```

- [ ] **Step 5: Check anonymous redirect**

```bash
curl -sI http://127.0.0.1:18765/apps/files/ | head -5
```
Expected: `HTTP/1.1 303 See Other` with a `Location:` header pointing at `/login?redirect_url=…` (per the SP7 redirect fix).

- [ ] **Step 6: Check SSR HTML for the stylesheet href**

```bash
curl -s http://127.0.0.1:18765/login 2>/dev/null | grep -o '<link rel="stylesheet" href="[^"]*"' | head -1
```
**Expected:** `<link rel="stylesheet" href="/assets/app-dxh<hash>.css"` — a real hashed URL.
**Failure modes:**
- `href="/assets/This should be replaced by dx as part of the build process. …"` → dx's link substitution didn't run on this binary. **NO-GO.**
- `href="C:\Users\…\app.css"` → `is_bundled_app()` returned false but the link section wasn't substituted. **NO-GO.**
- Anything else not matching the expected pattern → record and treat as NO-GO.

- [ ] **Step 7: Stop the server**

```bash
kill $SERVER_PID 2>/dev/null
wait $SERVER_PID 2>/dev/null
```
(On Windows, `taskkill /F /IM server.exe`.)

- [ ] **Step 8: Record the result**

If steps 2–6 all pass: result is **GO**. Capture the hashed CSS URL you saw — that's the marker PR's "evidence" line.

If any step failed: result is **NO-GO**. Capture the failure mode (which step, what was observed, full error text if applicable).

### Task A4: Write the marker PR + decision

**Files:**
- Create: `docs/superpowers/specs/2026-05-14-dx-built-server-consolidation-design.smoke-result.md`
- Modify (cleanup): `crates/crabcloud-server/Cargo.toml`, `crates/crabcloud-server/Dioxus.toml`

The dx-build metadata added in Task A1 was *for the smoke test only*. The full restructure will add those to `crabcloud-app`, not to `crabcloud-server` (which is getting deleted). Revert the smoke-test-only changes; the marker PR only ships the decision document.

- [ ] **Step 1: Revert the Task A1 changes**

```bash
git checkout origin/master -- crates/crabcloud-server/Cargo.toml
git rm crates/crabcloud-server/Dioxus.toml
```

- [ ] **Step 2: Write the result document**

Create `docs/superpowers/specs/2026-05-14-dx-built-server-consolidation-design.smoke-result.md`:

```markdown
# Smoke-test result — dx-built server consolidation

**Date:** 2026-05-<NN>
**Outcome:** GO  |  NO-GO

## What was tested

`dx build --release` invoked against `crates/crabcloud-server` with `dioxus = { features = ["fullstack"] }` added as a direct dep and a minimal `Dioxus.toml`. The runtime shape of `crabcloud-server` is already dx-compatible (`Cmd::Serve` mounts `dioxus::server::router(crabcloud_ui::App)` into `build_router`); only the build metadata was missing.

## What was observed

<one paragraph: did `dx build --release` succeed? if so, what does the binary do? the SSR'd `<link>` href format observed.>

## Decision

<GO: proceed with Batch B (mechanical rename). The full restructure is viable.

NO-GO: revert to the one-liner. Plan revised below.>

## Evidence

<paste-in evidence: success transcript or failure error text>
```

Fill in the outcome, the observed `<link>` href, and any captured error text. Keep it under one page.

- [ ] **Step 3: Commit + open PR**

```bash
git add docs/superpowers/specs/2026-05-14-dx-built-server-consolidation-design.smoke-result.md
git rm crates/crabcloud-server/Dioxus.toml
git add crates/crabcloud-server/Cargo.toml
git commit -m "app-consol(a): record smoke-test result (<GO|NO-GO>)"
git push -u origin app-consol/a-smoke
```

PR title (depending on outcome):
- GO: `app-consol: batch A — smoke test GO; proceed with rename`
- NO-GO: `app-consol: batch A — smoke test NO-GO; revising spec`

- [ ] **Step 4: Wait for review + merge**

If GO: merge, proceed to Batch B.
If NO-GO: STOP. The spec needs revision (likely back to the one-liner `CARGO_MANIFEST_DIR` fix). Open a discussion thread on the PR.

---

## Batch B — Mechanical rename `crabcloud-ui` → `crabcloud-app`

**Branch:** `app-consol/b-rename` off `origin/master`.
**Goal:** Rename the crate everywhere it appears, with no behavior change. After this batch, the workspace builds and tests identically to before; only paths and names have changed.

**Prerequisite:** Batch A merged with GO outcome.

### Task B1: Rename the directory

**Files:**
- Move: `crates/crabcloud-ui/` → `crates/crabcloud-app/`

- [ ] **Step 1: `git mv` the directory**

```bash
git mv crates/crabcloud-ui crates/crabcloud-app
```

- [ ] **Step 2: Verify**

```bash
ls crates/ | grep -E "crabcloud-(ui|app)"
```
Expected: `crabcloud-app` only.

### Task B2: Rename the crate in `crates/crabcloud-app/Cargo.toml`

**Files:**
- Modify: `crates/crabcloud-app/Cargo.toml`

- [ ] **Step 1: Change `name`**

```bash
sed -i 's/^name = "crabcloud-ui"$/name = "crabcloud-app"/' crates/crabcloud-app/Cargo.toml
```
(On Windows, use `-i ''` for sed or substitute with the equivalent PowerShell `(Get-Content … ) -replace … | Set-Content …`.)

Verify:
```bash
head -3 crates/crabcloud-app/Cargo.toml
```
Expected: `name = "crabcloud-app"`.

- [ ] **Step 2: Update the `[[bin]]` target**

In `crates/crabcloud-app/Cargo.toml`, the existing block reads:
```toml
[[bin]]
name = "crabcloud-ui"
path = "src/main.rs"
```
Change to:
```toml
[[bin]]
name = "crabcloud-app"
path = "src/main.rs"
```

### Task B3: Rename references in the workspace root `Cargo.toml`

**Files:**
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Update `members`**

In the root `Cargo.toml`, find the line `"crates/crabcloud-ui",` under `[workspace] members` and change it to `"crates/crabcloud-app",`. Re-sort alphabetically if necessary (between `crabcloud-sharing` and `crabcloud-storage` — `crabcloud-app` is actually earlier alphabetically, before `crabcloud-cache`).

- [ ] **Step 2: Update `[workspace.dependencies]`**

Find `crabcloud-ui = { path = "crates/crabcloud-ui" }` and change to `crabcloud-app = { path = "crates/crabcloud-app" }`. Re-sort alphabetically.

### Task B4: Update every other crate's `Cargo.toml`

**Files:**
- Modify: every `crates/*/Cargo.toml` that lists `crabcloud-ui` as a dep.

- [ ] **Step 1: Find call sites**

```bash
grep -rln 'crabcloud-ui' crates/ --include='Cargo.toml'
```
Expected to find: `crates/crabcloud-server/Cargo.toml` and possibly others.

- [ ] **Step 2: Replace each**

For each file the grep found, change `crabcloud-ui` → `crabcloud-app` in the dep declaration. The shape stays the same:
```toml
# before
crabcloud-ui = { workspace = true, features = ["server"] }
# after
crabcloud-app = { workspace = true, features = ["server"] }
```

### Task B5: Update Rust source `use` paths

**Files:**
- Modify: every `.rs` file containing `crabcloud_ui::` references.

- [ ] **Step 1: Find call sites**

```bash
grep -rln 'crabcloud_ui' crates/ --include='*.rs'
```

- [ ] **Step 2: Replace**

In each found file, search-and-replace `crabcloud_ui` → `crabcloud_app`. The replacement is mechanical; no semantic changes.

Examples of what changes:
- `use crabcloud_ui::App;` → `use crabcloud_app::App;`
- `crabcloud_ui::App` → `crabcloud_app::App`
- `dioxus::server::router(crabcloud_ui::App)` → `dioxus::server::router(crabcloud_app::App)`
- `use crabcloud_ui as _;` (lint anchors) → `use crabcloud_app as _;`

### Task B6: Update CI workflow

**Files:**
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Find references**

```bash
grep -n 'crabcloud-ui\|crabcloud_ui' .github/workflows/ci.yml
```

- [ ] **Step 2: Replace**

Each occurrence of `crabcloud-ui` → `crabcloud-app` (paths in `working-directory:`, artifact names, asset path env vars, etc.). For example:
- `working-directory: crates/crabcloud-ui` → `working-directory: crates/crabcloud-app`
- `path: target/dx/crabcloud-ui/release/web/public` → `path: target/dx/crabcloud-app/release/web/public`
- `DIOXUS_PUBLIC_PATH: …/target/dx/crabcloud-ui/release/web/public` → same with `-app`

### Task B7: Update doc + README references

**Files:**
- Modify: `README.md` (if it references `crabcloud-ui`)
- Modify: `e2e/README.md` and similar
- Modify: docs under `docs/superpowers/specs/` and `docs/superpowers/plans/` that mention `crabcloud-ui` (only do current-project docs; don't touch historical changelogs)

- [ ] **Step 1: Find references**

```bash
grep -rln 'crabcloud-ui\|crabcloud_ui' README.md e2e/ docs/ 2>/dev/null
```

- [ ] **Step 2: Replace in current docs only**

For each file:
- `README.md`, `e2e/README.md`, similar live docs: replace `crabcloud-ui` → `crabcloud-app`.
- Spec / plan markdown files: replace ONLY in files that describe currently-relevant designs (the carryforward spec & plan from this sub-project, and any current followup-files). Historical SP1-SP7 specs document past state; leave them alone (the historical `crabcloud-ui` references are part of the project record).

A pragmatic rule: replace in files dated 2026-05-14 or later. Leave anything dated earlier alone unless it's a changelog being actively updated.

### Task B8: Build + test + commit + PR

- [ ] **Step 1: Verify the workspace builds**

```bash
cargo check --workspace --all-targets
```
Expected: PASS. If anything fails, you missed a rename somewhere — re-run `grep -r 'crabcloud[_-]ui' crates/ .github/` (note the regex covers both `_` and `-`) and fix.

- [ ] **Step 2: Run the full test suite**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
All three must pass.

- [ ] **Step 3: Verify dx still builds the WASM bundle**

```bash
cd crates/crabcloud-app
dx build --release
cd ../..
test -f target/dx/crabcloud-app/release/web/public/assets/crabcloud-app_bg-*.wasm \
  || ls target/dx/crabcloud-app/release/web/public/assets/ | head -5
```
Expected: a wasm file under the new path.

- [ ] **Step 4: Commit + push + PR**

```bash
git add -A
git commit -m "app-consol(b): rename crabcloud-ui to crabcloud-app"
git push -u origin app-consol/b-rename
gh pr create --title "app-consol: batch B — rename crabcloud-ui to crabcloud-app" --body "Mechanical rename per the consolidation spec. No behavior change."
```

- [ ] **Step 5: Merge after CI green**

---

## Batch C — Fold `crabcloud-server` into `crabcloud-app`

**Branch:** `app-consol/c-fold` off `origin/master`.
**Goal:** `crates/crabcloud-server/` no longer exists. All its contents (main.rs, cli.rs, telemetry.rs, deps) live in `crates/crabcloud-app/`. The single binary `crabcloud-app` serves the full stack.

**Prerequisite:** Batch B merged.

### Task C1: Move `crabcloud-server`'s source files

**Files:**
- Move: `crates/crabcloud-server/src/main.rs` → `crates/crabcloud-app/src/main.rs` (REPLACES the existing UI-only `main.rs`)
- Move: `crates/crabcloud-server/src/cli.rs` → `crates/crabcloud-app/src/cli.rs`
- Move: `crates/crabcloud-server/src/telemetry.rs` → `crates/crabcloud-app/src/telemetry.rs`

- [ ] **Step 1: Inspect the existing `crabcloud-app/src/main.rs`**

```bash
cat crates/crabcloud-app/src/main.rs
```
This is currently the small "WASM entrypoint" main (the no-op stub when building for native, the dioxus launch when building for wasm32). After the move, this file becomes the binary entrypoint with clap CLI + subcommands.

- [ ] **Step 2: Save the existing WASM-entrypoint content**

The current `main.rs` looks roughly like:
```rust
#[cfg(target_arch = "wasm32")]
fn main() {
    crabcloud_app::install_csrf_fetch_interceptor();
    dioxus::launch(crabcloud_app::App);
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}  // server binary's main lives in crabcloud-server
```
The `#[cfg(target_arch = "wasm32")]` block needs to merge into the new combined `main.rs`. The `not(wasm32)` block gets replaced with crabcloud-server's real main.

Note its exact shape so you can merge correctly in Task C3.

- [ ] **Step 3: `git mv` the three files**

```bash
git mv crates/crabcloud-server/src/cli.rs crates/crabcloud-app/src/cli.rs
git mv crates/crabcloud-server/src/telemetry.rs crates/crabcloud-app/src/telemetry.rs
# main.rs is the merge case — handled in Task C3, not a simple move.
```

### Task C2: Merge dependencies in `crabcloud-app/Cargo.toml`

**Files:**
- Modify: `crates/crabcloud-app/Cargo.toml` — gains every dep `crabcloud-server` declared.

- [ ] **Step 1: Dump `crabcloud-server`'s deps**

```bash
sed -n '/^\[dependencies\]/,/^\[/p' crates/crabcloud-server/Cargo.toml
```

- [ ] **Step 2: Merge them into `crabcloud-app/Cargo.toml`**

Open `crates/crabcloud-app/Cargo.toml`. Under `[dependencies]`, add every entry from `crabcloud-server`'s `[dependencies]` that isn't already present. `crabcloud-app` should already pull most of these in via its `server` feature, but the new main.rs uses them directly so they need to be unconditional deps.

Likely additions (verify by reading the actual file): `anyhow`, `clap`, `crabcloud-config`, `rpassword`, `tokio` (with the same features `crabcloud-server` used), `tracing`, `tracing-subscriber`, and the workspace-internal `crabcloud-core`, `crabcloud-http`, `crabcloud-users` etc. that the CLI subcommands touch.

- [ ] **Step 3: Move dev-deps**

If `crabcloud-server/Cargo.toml` has `[dev-dependencies]`, merge those into `crabcloud-app/Cargo.toml`'s `[dev-dependencies]` similarly.

### Task C3: Rewrite `crabcloud-app/src/main.rs`

**Files:**
- Modify: `crates/crabcloud-app/src/main.rs` — new content combines the WASM bootstrap (from the old file) with crabcloud-server's full main.

- [ ] **Step 1: Examine `crates/crabcloud-server/src/main.rs`**

```bash
cat crates/crabcloud-server/src/main.rs
```
Note the structure: imports, helper functions (`prompt_password`, `read_password_from_stdin`, `shutdown_signal`), `#[tokio::main]` async fn, the `match cli.command` block dispatching all subcommands.

- [ ] **Step 2: Write the new combined `main.rs`**

The combined file looks like (paste the body of `crabcloud-server/src/main.rs` and prepend the WASM block):

```rust
//! Crabcloud server binary. On wasm32 this compiles to the dioxus client
//! entrypoint that boots WASM hydration; on native targets this is the full
//! axum server + CLI subcommands.

#[cfg(target_arch = "wasm32")]
fn main() {
    // WASM client entrypoint — installs the CSRF fetch interceptor then
    // hands control to dioxus::launch.
    crabcloud_app::install_csrf_fetch_interceptor();
    dioxus::launch(crabcloud_app::App);
}

#[cfg(not(target_arch = "wasm32"))]
mod cli;

#[cfg(not(target_arch = "wasm32"))]
mod telemetry;

#[cfg(not(target_arch = "wasm32"))]
use cli::{Cli, Cmd, FilesCmd};
// … all other native-only imports from crabcloud-server's main.rs …

#[cfg(not(target_arch = "wasm32"))]
fn prompt_password(prompt: &str) -> anyhow::Result<String> {
    let pw = rpassword::prompt_password(prompt)?;
    Ok(pw)
}

#[cfg(not(target_arch = "wasm32"))]
fn read_password_from_stdin() -> anyhow::Result<String> {
    use std::io::BufRead;
    let stdin = std::io::stdin();
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    let pw = line.trim_end_matches(['\r', '\n']).to_string();
    if pw.is_empty() {
        anyhow::bail!("empty password");
    }
    Ok(pw)
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    // … the entire body of crabcloud-server's `main` function, with
    // `crabcloud_ui::App` already renamed to `crabcloud_app::App` from Batch B.
}

#[cfg(not(target_arch = "wasm32"))]
async fn shutdown_signal() {
    // … crabcloud-server's existing shutdown_signal body …
}
```

**Important: the body is verbatim from `crates/crabcloud-server/src/main.rs`**, just wrapped in `#[cfg(not(target_arch = "wasm32"))]` where the WASM target wouldn't compile it. Don't reinvent any logic — copy-paste the function bodies.

- [ ] **Step 3: Apply the same cfg to cli.rs and telemetry.rs**

In `crates/crabcloud-app/src/cli.rs`, prepend:
```rust
#![cfg(not(target_arch = "wasm32"))]
```
Same for `crates/crabcloud-app/src/telemetry.rs`. These modules are unconditionally native; the wasm target should skip them.

(If the module declarations in main.rs use `#[cfg(not(target_arch = "wasm32"))] mod cli;`, that alone is sufficient and the inner `#![cfg(...)]` is belt-and-braces. Either works; pick one and be consistent.)

### Task C4: Delete the old `crabcloud-server` crate

**Files:**
- Delete: `crates/crabcloud-server/` entirely.
- Modify: workspace root `Cargo.toml` (remove from `members` and `[workspace.dependencies]`)

- [ ] **Step 1: Remove the directory**

```bash
git rm -r crates/crabcloud-server
```

- [ ] **Step 2: Update workspace `Cargo.toml`**

- Remove `"crates/crabcloud-server",` from the `[workspace] members` list.
- Remove `crabcloud-server = { path = "crates/crabcloud-server" }` from `[workspace.dependencies]`.

- [ ] **Step 3: Check for orphaned references**

```bash
grep -rln 'crabcloud-server\|crabcloud_server' crates/ docs/ .github/ README.md e2e/ 2>/dev/null
```
Update each — most should be path references in CI / docs that now point at `crabcloud-app`. Historical / changelog references (pre-2026-05-14) stay.

### Task C5: Verify cargo can still build everything

- [ ] **Step 1: Workspace build**

```bash
cargo check --workspace --all-targets
```
Expected: PASS. If anything fails, look for missed `crabcloud_server` references.

- [ ] **Step 2: Build the binary**

```bash
cargo build --release -p crabcloud-app
```
Expected: produces `target/release/crabcloud-app` (or `.exe` on Windows). This is the cargo-built fallback binary; the dx-built one comes later.

- [ ] **Step 3: Smoke-test CLI subcommands**

Use the fixture config you set up for Batch A's Task A3:
```bash
./target/release/crabcloud-app --config config/e2e.toml migrate
echo hunter2 | ./target/release/crabcloud-app --config config/e2e.toml user-add alice --password-stdin
./target/release/crabcloud-app --config config/e2e.toml serve &
sleep 3
curl -sI http://127.0.0.1:18765/status.php | head -1
kill $!
```
Expected: each subcommand exits 0 (or for `serve`, returns 200 on the status check before being killed).

Note: at this point the SSR'd HTML still has the placeholder href issue (the binary is cargo-built, not dx-built). That gets resolved in Batch D when we switch to `dx build --release` for production.

### Task C6: Run the full test suite + commit + PR

- [ ] **Step 1: Tests + clippy + fmt**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
All three must pass.

- [ ] **Step 2: Commit + push + PR**

```bash
git add -A
git commit -m "app-consol(c): fold crabcloud-server into crabcloud-app"
git push -u origin app-consol/c-fold
gh pr create --title "app-consol: batch C — fold crabcloud-server into crabcloud-app"
```

- [ ] **Step 3: Merge after CI green**

---

## Batch D — CI cutover + remove `AssetRewriteLayer`

**Branch:** `app-consol/d-ci-cutover` off `origin/master`.
**Goal:** CI's `e2e` job uses the dx-built binary instead of the cargo-built one. `AssetRewriteLayer` deleted (no longer needed; dx fills in the link section). New regression test asserts the SSR `<link>` href is hashed, not the placeholder.

**Prerequisite:** Batch C merged.

### Task D1: Add the regression test for SSR asset href

**Files:**
- Create or extend: `crates/crabcloud-http/tests/asset_render_regression.rs` (new) — or fold into an existing tests file.

This test asserts that against a running app, the SSR'd `<link rel="stylesheet">` href is a hashed `/assets/<…>.css` URL — NOT the manganis placeholder. It's the regression guard for the specific bug this whole sub-project addresses.

- [ ] **Step 1: Write the test**

Create `crates/crabcloud-http/tests/asset_render_regression.rs`:

```rust
//! Regression guard for the dx 0.7.9 placeholder-leak bug. The SSR'd HTML's
//! `<link rel="stylesheet">` href must be a hashed `/assets/<hash>.css` URL
//! produced by dx's link-time substitution — never the manganis placeholder
//! ("This should be replaced by dx as part of the build process. …") or an
//! absolute source path.

#![allow(unused_crate_dependencies)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use crabcloud_config::test_support::minimal_sqlite_config;
use crabcloud_core::AppStateBuilder;
use dioxus::server::{DioxusRouterExt, FullstackState};
use tempfile::tempdir;
use tower::ServiceExt;

#[tokio::test]
async fn ssr_stylesheet_href_is_a_hashed_assets_path() {
    let dir = tempdir().unwrap();
    let data = tempdir().unwrap();
    let mut cfg = minimal_sqlite_config(dir.path().join("h.db"));
    cfg.datadirectory = data.path().to_path_buf();
    cfg.filecache.enabled = false;
    let state = AppStateBuilder::new(cfg).build().await.unwrap();

    let dioxus_router = axum::Router::new()
        .register_server_functions()
        .with_state(FullstackState::headless());
    let app = crabcloud_http::build_router(state, dioxus_router);

    let req = Request::builder()
        .method("GET")
        .uri("/login")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 256 * 1024).await.unwrap();
    let html = std::str::from_utf8(&body).unwrap();

    // The link tag should reference a hashed CSS file under /assets/.
    let placeholder_marker = "This should be replaced by dx";
    assert!(
        !html.contains(placeholder_marker),
        "SSR'd HTML still contains the manganis placeholder — dx link substitution didn't run. Body excerpt: {}",
        &html[..html.len().min(2000)]
    );

    // The expected shape is `<link rel="stylesheet" href="/assets/app-dxh<hash>.css"`.
    // Match permissively on the dx hash prefix + .css suffix to avoid hard-
    // coding the hash.
    let link_pattern = regex::Regex::new(r#"<link rel="stylesheet" href="/assets/[A-Za-z0-9_-]+\.css""#).unwrap();
    assert!(
        link_pattern.is_match(html),
        "no hashed stylesheet href in SSR'd HTML. Body excerpt: {}",
        &html[..html.len().min(2000)]
    );
}
```

If the `regex` crate isn't already a dev-dep of `crabcloud-http`, add it:
```toml
[dev-dependencies]
regex = "1"
```

- [ ] **Step 2: Run it (expected: FAIL today, because the cargo-built test binary hits the placeholder)**

```bash
cargo test -p crabcloud-http --test asset_render_regression
```

**This test will fail when run as part of `cargo test --workspace`** — that's the cargo-built path. The test is a SUCCESS criterion for the dx-built binary, which is what CI's `e2e` job exercises after Batch D's CI changes.

Mark the test as `#[ignore = "asserts dx-link-section substitution; only true when binary built via dx"]` so `cargo test --workspace` skips it. The CI `e2e` job (after the cutover in Task D3) explicitly runs it.

Final test attribute block:
```rust
#[tokio::test]
#[ignore = "asserts dx-link-section substitution; only valid against dx-built binary"]
async fn ssr_stylesheet_href_is_a_hashed_assets_path() {
    // …
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/crabcloud-http/tests/asset_render_regression.rs crates/crabcloud-http/Cargo.toml
git commit -m "app-consol(d): regression test for SSR asset href"
```

### Task D2: Delete `AssetRewriteLayer`

**Files:**
- Delete: `crates/crabcloud-http/src/middleware/asset_rewrite.rs`
- Modify: `crates/crabcloud-http/src/middleware/mod.rs` (remove `pub mod asset_rewrite;`)
- Modify: `crates/crabcloud-http/src/router.rs` (remove the layer wiring)

- [ ] **Step 1: Find all references**

```bash
grep -rln 'AssetRewriteLayer\|asset_rewrite' crates/
```

- [ ] **Step 2: Delete the module**

```bash
git rm crates/crabcloud-http/src/middleware/asset_rewrite.rs
```

In `crates/crabcloud-http/src/middleware/mod.rs`, remove the line `pub mod asset_rewrite;`.

- [ ] **Step 3: Remove the layer from the router**

In `crates/crabcloud-http/src/router.rs` (or wherever `build_router` is defined), find the `.layer(AssetRewriteLayer::new())` (or analogous) call inside the layer stack. Delete that line. Also remove the `use crate::middleware::asset_rewrite::AssetRewriteLayer;` import at the top of the file.

- [ ] **Step 4: Check build**

```bash
cargo check --workspace
```
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "app-consol(d): remove AssetRewriteLayer (dx now fills in asset hrefs)"
```

### Task D3: Update CI workflow to use the dx-built binary

**Files:**
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Delete the `build-wasm` job**

In `.github/workflows/ci.yml`, find the entire `build-wasm:` job and remove it. It produces an artifact that the new e2e workflow won't need (e2e runs `dx build --release` itself).

- [ ] **Step 2: Rewrite the `e2e` job**

Replace the `e2e:` job's body with (preserving the env / triggers / `needs`):

```yaml
  e2e:
    runs-on: ubuntu-latest
    needs: fmt-and-clippy
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v6
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: wasm32-unknown-unknown
      - uses: Swatinem/rust-cache@v2
      - name: Install dioxus-cli
        run: cargo install dioxus-cli --version "^0.7" --locked
      - name: dx build --release
        run: |
          cd crates/crabcloud-app
          dx build --release
      - uses: actions/setup-node@v6
        with:
          node-version: "24"
          cache: "npm"
          cache-dependency-path: e2e/package-lock.json
      - name: Install Playwright deps
        working-directory: e2e
        run: |
          npm ci
          npx playwright install --with-deps chromium
      - name: Generate bcrypt hash for fixture
        id: bcrypt
        run: |
          python3 -m pip install bcrypt
          HASH=$(python3 -c "import bcrypt; print(bcrypt.hashpw(b'hunter2', bcrypt.gensalt(12)).decode())")
          echo "hash=$HASH" >> "$GITHUB_OUTPUT"
      - name: Write fixture config
        env:
          BCRYPT_HASH: ${{ steps.bcrypt.outputs.hash }}
        run: |
          mkdir -p config
          cat > config/e2e.toml <<'EOF'
          instanceid     = "e2e"
          secret         = "a-32-byte-or-longer-secret-key!"
          passwordsalt   = "ps"
          installed      = true
          version        = "31.0.0.0"
          versionstring  = "31.0.0"
          dbtype         = "sqlite"
          dbname         = "__WS__/e2e.db"
          dbtableprefix  = "oc_"
          datadirectory  = "__WS__/data"
          trusted_domains = ["localhost", "127.0.0.1"]
          loglevel       = "info"
          bind_address   = "127.0.0.1:18765"
          db_pool_max    = 4

          [cache]
          backend = "memory"

          [bootstrap_admin]
          username      = "admin"
          password_hash = "__HASH__"
          EOF
          sed -i "s|__WS__|$GITHUB_WORKSPACE|g" config/e2e.toml
          python3 -c "import os, pathlib; p = pathlib.Path('config/e2e.toml'); p.write_text(p.read_text().replace('__HASH__', os.environ['BCRYPT_HASH']))"
      - name: Migrate
        run: ./target/dx/crabcloud-app/release/web/server.exe --config config/e2e.toml migrate
      - name: Bootstrap sharing-test users
        run: |
          for u in sharealice sharebob; do
            echo 'hunter2' | ./target/dx/crabcloud-app/release/web/server.exe --config config/e2e.toml user-add "$u" --password-stdin
          done
      - name: Start server
        run: |
          ./target/dx/crabcloud-app/release/web/server.exe --config config/e2e.toml serve &
          echo "SERVER_PID=$!" >> "$GITHUB_ENV"
          for i in $(seq 1 30); do
            if curl -sf http://127.0.0.1:18765/status.php >/dev/null; then
              echo "Server ready"
              exit 0
            fi
            sleep 1
          done
          echo "Server did not become ready"
          exit 1
      - name: Run Playwright tests
        working-directory: e2e
        env:
          CRABCLOUD_E2E_URL: http://127.0.0.1:18765
        run: npm test
      - name: Run dx-built regression tests
        run: |
          # The asset_render_regression test is `#[ignore]`'d in the normal
          # test suite because it only passes against a dx-built binary.
          # Here, we run it against a binary that came through dx's linker.
          cargo test -p crabcloud-http --test asset_render_regression -- --ignored
      - name: Stop server
        if: always()
        run: |
          if [ -n "$SERVER_PID" ]; then
            kill "$SERVER_PID" 2>/dev/null || true
          fi
      - name: Upload Playwright report on failure
        if: failure()
        uses: actions/upload-artifact@v7
        with:
          name: playwright-report
          path: e2e/playwright-report
          retention-days: 7
      - name: Upload Playwright test-results on failure
        if: failure()
        uses: actions/upload-artifact@v7
        with:
          name: playwright-test-results
          path: e2e/test-results
          retention-days: 7
```

Notes:
- The Ubuntu runner produces `server.exe` only if dx names it that way on Linux. Earlier dx source showed the server-bundle binary is named `server` on Linux and `server.exe` on Windows. Adjust the path: on Linux runners use `target/dx/crabcloud-app/release/web/server`.
- Verify the path by running `dx build --release` locally first and looking at the artifact name. If Linux produces `server`, change all `server.exe` references in the YAML to `server`.

- [ ] **Step 3: Verify the YAML parses**

```bash
python3 -c "import yaml, pathlib; yaml.safe_load(pathlib.Path('.github/workflows/ci.yml').read_text())" && echo OK
```
Expected: `OK`.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "app-consol(d): CI uses dx-built binary; drop build-wasm job"
```

### Task D4: Pre-PR sweep + PR

- [ ] **Step 1: Pre-PR checks**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
All three must pass.

- [ ] **Step 2: Open PR**

```bash
git push -u origin app-consol/d-ci-cutover
gh pr create --title "app-consol: batch D — CI uses dx-built binary; remove AssetRewriteLayer"
```

- [ ] **Step 3: Watch CI carefully**

The first CI run on this branch is the moment of truth. Watch the `e2e` job specifically:
- Does `dx build --release` succeed in the GitHub Actions environment?
- Does the dx-built binary's `--config X migrate` work?
- Do the Playwright tests pass against the dx-built server?
- Does the `asset_render_regression` test pass with `--ignored`?

If anything fails, debug from the captured logs. The most likely failure modes:
- Dependency that built locally but doesn't build under Linux CI (unlikely if Batch A's smoke succeeded).
- `dx build --release` taking longer than the 30-minute timeout (raise it; consider a runner with more cores).
- Server start race (the existing 30-second retry loop should cover it).

- [ ] **Step 4: Merge after CI green**

---

## Batch E — `dx serve` wiring + dev docs

**Branch:** `app-consol/e-dx-serve` off `origin/master`.
**Goal:** `dx serve` (run from `crates/crabcloud-app/`) launches the dev server with hot-reload. README documents the dev + prod commands.

**Prerequisite:** Batch D merged.

### Task E1: Wire `dioxus_cli_config::fullstack_address_or_localhost()` into `Cmd::Serve`

**Files:**
- Modify: `crates/crabcloud-app/src/main.rs`
- Modify: `crates/crabcloud-app/Cargo.toml` (add `dioxus-cli-config` as a direct dep)

- [ ] **Step 1: Add the dep**

In `crates/crabcloud-app/Cargo.toml`, under `[dependencies]`:
```toml
dioxus-cli-config = { version = "0.7", optional = true }
```
And under the `server` feature in `[features]`:
```toml
server = [
    # … existing entries …
    "dep:dioxus-cli-config",
]
```

(If the feature gate setup is different — e.g. all native deps are unconditional — adapt to match. The intent is: dioxus-cli-config is only needed for native server builds.)

- [ ] **Step 2: Modify `Cmd::Serve`**

In `crates/crabcloud-app/src/main.rs`, find the `Cmd::Serve` arm. Insert the env-var override before the `axum::serve` call:

```rust
Cmd::Serve => {
    let mut config = crabcloud_config::load(&cli.config, &[])?;
    // dx serve sets `IP` + `PORT` env vars before launching the binary.
    // Honor them when present so the HMR websocket / asset reload reach
    // the right address. Otherwise stick with what the config file said —
    // production environments don't set these.
    if std::env::var("PORT").is_ok() || std::env::var("IP").is_ok() {
        config.bind_address = dioxus_cli_config::fullstack_address_or_localhost();
    }
    let bind = config.bind_address;
    info!(
        dbtype = %config.dbtype.as_str(),
        bind = %bind,
        "starting Crabcloud server"
    );
    // … rest of the existing serve body, unchanged …
}
```

- [ ] **Step 3: Build + test**

```bash
cargo build --release -p crabcloud-app
cargo test --workspace
```
Expected: PASS.

- [ ] **Step 4: Smoke test the override locally**

```bash
PORT=18900 IP=127.0.0.1 ./target/release/crabcloud-app --config config/e2e.toml serve &
sleep 3
curl -sI http://127.0.0.1:18900/status.php | head -1
kill $!
```
Expected: `HTTP/1.1 200 OK`. (If the binary ignored the env vars and bound to 18765 instead, the curl will fail — that's a regression.)

- [ ] **Step 5: Commit**

```bash
git add crates/crabcloud-app/Cargo.toml crates/crabcloud-app/src/main.rs
git commit -m "app-consol(e): honor PORT/IP env vars from dx serve in Cmd::Serve"
```

### Task E2: Verify `dx serve` end-to-end

This is a manual verification step; not a code change.

- [ ] **Step 1: Run `dx serve`**

```bash
cd crates/crabcloud-app
dx serve --release
```

Expected: dx builds the binary + WASM bundle, launches the server, opens a browser at the dev URL (usually `http://localhost:8080`). Browser console shouldn't show the placeholder error.

- [ ] **Step 2: Click around**

- Navigate to `/apps/files/`. Expect a 303 redirect to `/login` (anonymous user).
- Click "Log in", enter admin credentials. Expect login to succeed.
- Verify the SSR'd HTML has a hashed CSS href (Open the page source via Ctrl+U).

- [ ] **Step 3: Test hot-reload**

While the dx serve session is running:
- Edit a string in `crates/crabcloud-app/src/pages/files/mod.rs` (e.g. change a placeholder string in the empty-folder state).
- Save the file.
- The browser should auto-refresh and show the new string.

Document the result in the PR description (success or specific failure modes).

If `dx serve` doesn't work as expected, that's worth flagging in the PR description; it's not a blocker for the rest of the batch but worth investigation.

### Task E3: Update README + dev docs

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add / update a "Development" section**

Open `README.md`. Find the existing build / run instructions (search for `dx build` or `cargo run`). Replace them with a section like:

```markdown
## Development

### Hot-reload dev server

From the workspace root or from `crates/crabcloud-app/`:

```bash
dx serve --release
```

This builds the WASM bundle + the server binary and launches both with file-watch hot-reload. The dev server proxies to the binary at the URL dx prints (typically `http://localhost:8080`). Code changes to `crates/crabcloud-app/src/**` trigger automatic rebuilds.

### Release build

```bash
cd crates/crabcloud-app
dx build --release
```

Produces the server binary at `target/dx/crabcloud-app/release/web/server` (Linux/macOS) or `server.exe` (Windows), with the asset bundle at `target/dx/crabcloud-app/release/web/public/`. Run it via:

```bash
./target/dx/crabcloud-app/release/web/server --config config/config.toml serve
```

### Cargo-only fallback (no asset substitution)

`cargo run --release -p crabcloud-app -- <subcommand>` works for the CLI subcommands (`migrate`, `user-add`, etc.) and for running the server without dx. Note: a cargo-built server's SSR'd HTML will contain manganis placeholder strings for stylesheet hrefs — use the dx-built binary for production / serving real users. The cargo path is convenient for scripted CLI tasks.
```

Adjust to fit the existing README's tone / structure.

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "app-consol(e): document dev + prod commands after consolidation"
```

### Task E4: Pre-PR sweep + PR

- [ ] **Step 1: Pre-PR checks**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
All three must pass.

- [ ] **Step 2: Open PR**

```bash
git push -u origin app-consol/e-dx-serve
gh pr create --title "app-consol: batch E — dx serve wiring + dev docs"
```

- [ ] **Step 3: Merge after CI green**

---

## Closing checklist

After Batch E merges:

- [ ] Every acceptance criterion from spec §9 passes on master:
  - `dx build --release` produces the binary at `target/dx/crabcloud-app/release/web/server.exe`.
  - That binary serves the full stack.
  - SSR HTML's `<link>` href is `/assets/<hash>.css`.
  - `AssetRewriteLayer` is gone (grep confirms).
  - `crates/crabcloud-server/` is gone (grep confirms).
  - Tests + clippy + e2e all green.
  - `dx serve` works for dev (manual).
  - README documents dev + prod commands.
- [ ] The two remaining SP7 carryforwards (filecache poisoning, `SqlUserStore::lookup` decode bug) stay queued for follow-up.
- [ ] SP8 (public links) is unblocked: clean local screenshots are now achievable via `dx build` or `dx serve`.
