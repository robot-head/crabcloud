# Sub-project 6 follow-up — open notes feeding SP7 (Sharing)

Surfaced during SP6 implementation.

## Seams left open for sharing

- Sidebar reserves space for "Favorites", "Recent", "Shared with you", "Trash". Strings + routing land in SP7+.
- `FileEntry` DTO has no sharing fields yet. When sharing lands, extend with `shared_with: Option<Vec<String>>` + indicator flags rather than changing existing fields — both the WASM `FileList`/`FileRow` and the server fn's `dir_entry_to_dto` are tolerant of additive changes.
- The row ⋯ menu has two fixed entries (Rename, Delete). A "Share" entry plugs in as a third item; the menu is rendered as plain markup inside `FileRow`, so adding it is a one-place change.
- The new `require_user()` helper in `server_fns.rs` reads `AuthContext` (not session-gated). Sharing endpoints can reuse it for the same Bearer-or-cookie behavior, and additionally gate on `ctx.method == AuthMethod::Session` if a share action must be browser-only (mirroring `list_app_passwords`).

## Working primitives sharing can reuse

- The drop overlay's positioning + activation pattern can drive a "drag-to-share" flow if that lands in sharing.
- The clipboard chip pattern (sticky, navigation-preserving toolbar chip) can become a "share clipboard" for bulk-share UX.
- The `DeleteModal` is a clean template for the share-confirmation modal — same backdrop + body + destructive button shape.

## Plan deviations actually landed (worth knowing for SP7)

- Server fns use `POST /api/files/<op>` + JSON body, not `GET /api/files/list?path=...` as the design originally proposed. The plan literal was wrong against Dioxus 0.7's `#[get]` macro semantics. SP7's share endpoints should default to the `#[server(endpoint = "...", prefix = "")]` (POST + JSON) pattern unless there's a specific reason to use `#[get]`.
- Auth in server fns reads `crabcloud_http::AuthContext` from `FullstackContext::extension`, not the `SessionHandle` snapshot. The design called for the snapshot but it's cookie-only. AuthContext works for all three auth methods.
- The chunked upload's commit `MOVE` request is built via `web_sys::Request` + `window.fetch()` because `gloo-net` doesn't expose arbitrary HTTP methods. Re-usable pattern for any future WebDAV verb the browser needs to issue.

## Known limitations carried forward

- **No client-side pagination.** `list_dir` returns every entry in the folder. Shared folders with many entries may need server-side pagination (offset + limit) in SP7 — flag this when the share UX has lots of items.
- **Upload retry is UI-only.** A failed upload's Retry button resets the job to Queued but cannot re-spawn the network task without prompting the user to re-pick the file (browser security: blob references are not persistent across page reloads or unrelated to a fresh user gesture). Acceptable MVP behavior per spec §17.
- **No outside-click dismiss on the ⋯ overflow menu** or on the inline rename input's parent area. Polish item for a later cycle.
- **No file-type icons.** All files render as `📄`, all dirs as `📁`. Mime-aware icons land later.
