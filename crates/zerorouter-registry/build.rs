//! Embeds `plugins/bundled/*.toml` as a static `(file name, source)` table.

use std::path::Path;
use std::{env, fs};

fn main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let dir = Path::new(&manifest).join("../../plugins/bundled");
    println!("cargo:rerun-if-changed={}", dir.display());

    let mut files: Vec<_> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .map(|entry| entry.expect("dir entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    files.sort();

    let mut out = String::from("pub static BUNDLED: &[(&str, &str)] = &[\n");
    for path in &files {
        let abs = path.canonicalize().expect("canonical path");
        let name = path.file_name().unwrap().to_string_lossy();
        out.push_str(&format!("    ({name:?}, include_str!({:?})),\n", abs.display().to_string()));
    }
    out.push_str("];\n");
    let dest = Path::new(&env::var("OUT_DIR").expect("OUT_DIR")).join("bundled_plugins.rs");
    fs::write(dest, out).expect("writing bundled_plugins.rs");
}
