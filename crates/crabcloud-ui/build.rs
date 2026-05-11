//! Two jobs at compile time:
//!
//! 1. Ensure `target/dx/crabcloud-ui-web/release/web/public/` exists before
//!    `rust-embed`'s proc macro inspects it. On a fresh checkout that never
//!    ran `dx build`, the directory wouldn't exist and `#[derive(RustEmbed)]`
//!    would error. Creating an empty placeholder lets the macro produce an
//!    empty asset set; the asset handler then returns 404 for every path
//!    until `dx build --release --platform web` populates the directory.
//!
//! 2. Extract the `<script type="module" ... src="...">` tag dx 0.7 injects
//!    into its emitted `index.html`. Release-mode dx hashes the bundle
//!    filename (e.g. `assets/crabcloud-ui-web-dx<hash>.js`), so the SSR shell
//!    can't hard-code the path. We parse `index.html`, find the bundle
//!    script tag, and write the verbatim opening + closing tag pair to
//!    `OUT_DIR/wasm_script_tag.txt` for `ssr.rs` to `include_str!`. When
//!    dx hasn't been run (e.g. unit tests on a fresh checkout), the file is
//!    empty and SSR omits the bundle tag — handlers still SSR fine, just
//!    without client hydration.
//!
//! Both jobs are idempotent and re-key on the index.html mtime via
//! `cargo:rerun-if-changed`.

use std::path::PathBuf;

fn main() {
    let manifest_dir =
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set by cargo");
    let mut public = PathBuf::from(&manifest_dir);
    public.push("..");
    public.push("..");
    public.push("target");
    public.push("dx");
    public.push("crabcloud-ui-web");
    public.push("release");
    public.push("web");
    public.push("public");
    if let Err(e) = std::fs::create_dir_all(&public) {
        println!(
            "cargo:warning=crabcloud-ui: could not create {}: {}",
            public.display(),
            e
        );
    }

    let out_dir = std::env::var_os("OUT_DIR").expect("OUT_DIR must be set by cargo");
    let tag_out = PathBuf::from(&out_dir).join("wasm_script_tag.txt");
    let index_html = public.join("index.html");
    println!("cargo:rerun-if-changed={}", index_html.display());
    let tag = std::fs::read_to_string(&index_html)
        .ok()
        .and_then(|html| extract_wasm_script_tag(&html))
        .unwrap_or_default();
    if tag.is_empty() {
        println!(
            "cargo:warning=crabcloud-ui: no module-script bundle tag found in {} \u{2014} run `dx build --release --platform web` from crates/crabcloud-ui first",
            index_html.display()
        );
    }
    std::fs::write(&tag_out, tag).expect("write wasm_script_tag.txt to OUT_DIR");

    println!("cargo:rerun-if-changed=build.rs");
}

/// Find the first `<script>` opening tag in `html` whose attributes include
/// both `type="module"` and `src=`, and return that tag plus its closing
/// `</script>`. Returns `None` if no matching tag is present.
///
/// dx 0.7's `inject_loading_scripts` emits exactly one such tag, so the
/// first match is what we want.
fn extract_wasm_script_tag(html: &str) -> Option<String> {
    let mut rest = html;
    while let Some(open_idx) = rest.find("<script") {
        let after_open = &rest[open_idx..];
        let close_idx = after_open.find('>')?;
        let opening = &after_open[..=close_idx];
        let lower = opening.to_ascii_lowercase();
        if lower.contains("type=\"module\"") && lower.contains(" src=") {
            return Some(format!("{opening}</script>"));
        }
        rest = &after_open[close_idx + 1..];
    }
    None
}
