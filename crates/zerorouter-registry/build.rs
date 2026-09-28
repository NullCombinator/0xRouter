//! Embeds `plugins/bundled/*.toml` and `styles/bundled/*.toml` as static
//! `(file name, source)` tables.

use std::path::Path;
use std::{env, fs, io};

fn main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let root = Path::new(&manifest).join("../..");
    embed(&root.join("plugins/bundled"), "BUNDLED", "bundled_plugins.rs");
    embed(&root.join("styles/bundled"), "BUNDLED_STYLES", "bundled_styles.rs");
}

/// A missing directory embeds an empty table.
fn embed(dir: &Path, name: &str, out_file: &str) {
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files: Vec<_> = match fs::read_dir(dir) {
        Ok(entries) => entries
            .map(|entry| entry.expect("dir entry").path())
            .filter(|p| p.extension().is_some_and(|e| e == "toml"))
            .collect(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => panic!("reading {}: {e}", dir.display()),
    };
    files.sort();

    let mut out = format!("pub static {name}: &[(&str, &str)] = &[\n");
    for path in &files {
        let abs = path.canonicalize().expect("canonical path");
        let file = path.file_name().unwrap().to_string_lossy();
        out.push_str(&format!("    ({file:?}, include_str!({:?})),\n", abs.display().to_string()));
    }
    out.push_str("];\n");
    let dest = Path::new(&env::var("OUT_DIR").expect("OUT_DIR")).join(out_file);
    fs::write(dest, out).expect("writing embedded table");
}
