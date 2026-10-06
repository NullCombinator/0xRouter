//! Embeds `plugins/bundled/*.toml`, `plugins/community/*.toml` and `styles/bundled/*.toml` as static
//! `(file name, source)` tables, and `plugins/{bundled,community}/logos/*.png` as
//! `(file name, bytes)` tables (spec 009 research R10).

use std::path::{Path, PathBuf};
use std::{env, fs, io};

fn main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let root = Path::new(&manifest).join("../..");
    embed(&root.join("plugins/bundled"), "BUNDLED", "bundled_plugins.rs");
    embed(&root.join("styles/bundled"), "BUNDLED_STYLES", "bundled_styles.rs");
    embed(&root.join("plugins/community"), "COMMUNITY", "community_plugins.rs");
    let logos = [
        ("BUNDLED_LOGOS", root.join("plugins/bundled/logos")),
        ("COMMUNITY_LOGOS", root.join("plugins/community/logos")),
    ];
    let mut out = String::new();
    for (name, dir) in &logos {
        out.push_str(&format!("pub(crate) static {name}: &[(&str, &[u8])] = &[\n"));
        for path in files(dir, "png") {
            let abs = path.canonicalize().expect("canonical path");
            let file = path.file_name().unwrap().to_string_lossy();
            out.push_str(&format!("    ({file:?}, include_bytes!({:?})),\n", abs.display().to_string()));
        }
        out.push_str("];\n");
    }
    write(&out, "logos.rs");
}

/// A missing directory embeds an empty table.
fn embed(dir: &Path, name: &str, out_file: &str) {
    let mut out = format!("pub static {name}: &[(&str, &str)] = &[\n");
    for path in files(dir, "toml") {
        let abs = path.canonicalize().expect("canonical path");
        let file = path.file_name().unwrap().to_string_lossy();
        out.push_str(&format!("    ({file:?}, include_str!({:?})),\n", abs.display().to_string()));
    }
    out.push_str("];\n");
    write(&out, out_file);
}

/// The files in `dir` with extension `ext`, sorted; none when `dir` is missing.
fn files(dir: &Path, ext: &str) -> Vec<PathBuf> {
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files: Vec<_> = match fs::read_dir(dir) {
        Ok(entries) => entries
            .map(|entry| entry.expect("dir entry").path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == ext))
            .collect(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => panic!("reading {}: {e}", dir.display()),
    };
    files.sort();
    files
}

fn write(out: &str, out_file: &str) {
    let dest = Path::new(&env::var("OUT_DIR").expect("OUT_DIR")).join(out_file);
    fs::write(dest, out).expect("writing embedded table");
}
