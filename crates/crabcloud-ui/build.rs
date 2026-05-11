//! Ensure the `target/dx/crabcloud-ui-web/release/web/public/` directory exists
//! before `rust-embed`'s proc macro inspects it. On a fresh checkout that
//! never ran `dx build`, the directory wouldn't exist and `#[derive(RustEmbed)]`
//! would error at compile time. Creating an empty placeholder lets the macro
//! produce an empty asset set; the asset handler then returns 404 for every
//! path until `cargo xtask build` (Task 10) populates the directory.
//!
//! We also crack open the dx-emitted `index.html` (when present) and extract
//! the `<script>` tag dx injects for the WASM bundle. dx 0.7 hashes the bundle
//! filename in release mode (e.g. `assets/<hash>.js`), so a hard-coded path no
//! longer works — instead we re-emit the tag dx wrote into our SSR document.

use std::path::PathBuf;

fn main() {
    let manifest_dir =
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set by cargo");
    let mut dir = PathBuf::from(&manifest_dir);
    dir.push("..");
    dir.push("..");
    dir.push("target");
    dir.push("dx");
    dir.push("crabcloud-ui-web");
    dir.push("release");
    dir.push("web");
    dir.push("public");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        println!(
            "cargo:warning=crabcloud-ui: could not create {}: {}",
            dir.display(),
            e
        );
    }

    // Extract dx's injected `<script>` tag from index.html (if present) and
    // emit it as `wasm_script_tag.txt` in OUT_DIR for ssr.rs to `include_str!`.
    // The hash in release-mode bundle paths changes per build, so the SSR side
    // can't hard-code the filename.
    let out_dir = std::env::var_os("OUT_DIR").expect("OUT_DIR must be set by cargo");
    let script_tag_out = PathBuf::from(&out_dir).join("wasm_script_tag.txt");
    let index_html = dir.join("index.html");
    println!("cargo:rerun-if-changed={}", index_html.display());
    let tag = std::fs::read_to_string(&index_html)
        .ok()
        .and_then(|html| extract_wasm_script_tag(&html));
    let tag = tag.unwrap_or_default();
    if tag.is_empty() {
        println!(
            "cargo:warning=crabcloud-ui: no <script type=\"module\"> bundle tag found in {} — run `dx build --release --platform web` from crates/crabcloud-ui first",
            index_html.display()
        );
    }
    std::fs::write(&script_tag_out, tag).expect("write wasm_script_tag.txt to OUT_DIR");

    println!("cargo:rerun-if-changed=build.rs");
}

/// Return the verbatim `<script ... src="...">` opening tag (followed by its
/// closing `</script>`) that dx injects to load the WASM bundle. We look for
/// the first `<script` whose attributes include both `type="module"` and a
/// `src=`, since dx's `inject_loading_scripts` emits exactly that shape.
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
