//! Builds a static phf::Map<&'static str, &'static str> from
//! `data/mimetypes.txt` (TAB-separated `extension<TAB>mimetype` lines).
//! Generated module is included from `mimetype.rs` via include!.

use std::env;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=data/mimetypes.txt");

    let src = fs::read_to_string("data/mimetypes.txt").expect("read mimetypes.txt");
    let out_dir = env::var_os("OUT_DIR").expect("OUT_DIR");
    let out_path = Path::new(&out_dir).join("mimetype_map.rs");
    let out_file = fs::File::create(&out_path).expect("create mimetype_map.rs");
    let mut writer = BufWriter::new(out_file);

    let mut map = phf_codegen::Map::<&'static str>::new();
    let mut count: usize = 0;
    // Hold borrowed string references for the duration of map-building. The
    // closure `entry` returns &str references into `lines_owned`, so it has
    // to outlive the map builder.
    let lines_owned: Vec<(String, String)> = src
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let (ext, mime) = l.split_once('\t')?;
            Some((ext.trim().to_string(), mime.trim().to_string()))
        })
        .collect();

    let leaked: Vec<(&'static str, &'static str)> = lines_owned
        .iter()
        .map(|(e, m)| (e.as_str(), m.as_str()))
        .map(|(e, m)| {
            // Leak strings so phf_codegen receives 'static refs. Acceptable
            // for a build script: process is one-shot.
            let e: &'static str = Box::leak(e.to_string().into_boxed_str());
            let m: &'static str = Box::leak(m.to_string().into_boxed_str());
            (e, m)
        })
        .collect();

    // phf_codegen 0.13 requires the value arg to `entry` to outlive the call,
    // so format up-front and hold the strings in `quoted_values` for the
    // lifetime of the map builder.
    let quoted_values: Vec<String> = leaked
        .iter()
        .map(|(_, mime)| format!("\"{}\"", mime))
        .collect();
    for ((ext, _), quoted) in leaked.iter().zip(quoted_values.iter()) {
        map.entry(*ext, quoted);
        count += 1;
    }

    writeln!(
        &mut writer,
        "/// Auto-generated extension→mimetype map. Do not edit; regenerated\n\
         /// from `data/mimetypes.txt` by `build.rs`.\n\
         pub static EXTENSION_MIMETYPES: phf::Map<&'static str, &'static str> = {};",
        map.build()
    )
    .expect("write map");

    writeln!(
        &mut writer,
        "\n#[cfg(test)]\npub const EXTENSION_COUNT: usize = {};",
        count
    )
    .expect("write count");
}
