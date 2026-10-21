//! The kit, embedded from the live sources of `crates/nullrouter-adapter-kit` so the builder
//! can never drift from it. `setup` writes it out as a directory-source package: the manifest
//! is normalised (no workspace inheritance, no lints table) and a `.cargo-checksum.json` lists
//! every file.

use std::collections::BTreeMap;

use serde::Serialize;
use sha2::{Digest, Sha256};

/// The ABI major this builder's kit speaks (checked against the kit crate in the tests).
pub const KIT_ABI: u32 = 1;

const MANIFEST: &str = include_str!("../../nullrouter-adapter-kit/Cargo.toml");

/// Every file under the kit's `src/`. A test fails if the directory gains a file not listed.
pub(crate) const SOURCES: &[(&str, &str)] = &[
    ("src/abi.rs", include_str!("../../nullrouter-adapter-kit/src/abi.rs")),
    ("src/context.rs", include_str!("../../nullrouter-adapter-kit/src/context.rs")),
    ("src/edit.rs", include_str!("../../nullrouter-adapter-kit/src/edit.rs")),
    ("src/input.rs", include_str!("../../nullrouter-adapter-kit/src/input.rs")),
    ("src/lib.rs", include_str!("../../nullrouter-adapter-kit/src/lib.rs")),
];

/// The dependency lines of the normalised manifest. They mirror the workspace's `serde` and
/// `serde_json` entries; a test compares them with the root `Cargo.toml`.
pub(crate) const DEPENDENCIES: &str = r#"serde = { version = "1", features = ["derive"] }
serde_json = { version = "1", features = ["preserve_order", "raw_value"] }
"#;

fn manifest_value() -> toml::Table {
    // The embedded file is checked by the tests, so a parse failure is a build-time defect.
    MANIFEST.parse().unwrap_or_default()
}

/// The kit's own `package.version`.
#[must_use]
pub fn kit_version() -> String {
    manifest_value()
        .get("package")
        .and_then(|p| p.get("version"))
        .and_then(toml::Value::as_str)
        .unwrap_or("0.0.0")
        .to_owned()
}

fn description() -> String {
    manifest_value()
        .get("package")
        .and_then(|p| p.get("description"))
        .and_then(toml::Value::as_str)
        .unwrap_or("")
        .to_owned()
}

/// The directory name under `vendor/`.
pub(crate) fn dir_name() -> String {
    format!("nullrouter-adapter-kit-{}", kit_version())
}

pub(crate) fn normalised_manifest() -> String {
    format!(
        "[package]\nname = \"nullrouter-adapter-kit\"\nversion = {:?}\nedition = \"2024\"\nrust-version = \"1.93\"\nlicense = \"MIT\"\ndescription = {:?}\n\n[dependencies]\n{DEPENDENCIES}",
        kit_version(),
        description()
    )
}

/// The package's files, `Cargo.toml` first, as `(relative path, bytes)`.
pub(crate) fn files() -> Vec<(String, Vec<u8>)> {
    let mut out = vec![("Cargo.toml".to_owned(), normalised_manifest().into_bytes())];
    out.extend(SOURCES.iter().map(|(p, s)| ((*p).to_owned(), s.as_bytes().to_vec())));
    out
}

fn hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Serialize)]
struct Checksums {
    files: BTreeMap<String, String>,
    package: String,
}

/// The `.cargo-checksum.json` of the kit directory, and the `package` value. Cargo copies
/// `package` into the lock file's `checksum`, so setup writes the same value there.
pub(crate) fn checksum_json() -> (String, String) {
    let files = files();
    let package = {
        let mut sorted: Vec<&(String, Vec<u8>)> = files.iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        let mut h = Sha256::new();
        for (path, bytes) in sorted {
            h.update(path.as_bytes());
            h.update([0]);
            h.update(bytes.len().to_string().as_bytes());
            h.update([0]);
            h.update(bytes);
        }
        format!("{:x}", h.finalize())
    };
    let listed = files.iter().map(|(p, b)| (p.clone(), hex(b))).collect();
    let json = serde_json::to_string(&Checksums { files: listed, package: package.clone() }).unwrap_or_default();
    (json, package)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kit_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../nullrouter-adapter-kit")
    }

    #[test]
    fn abi_matches_the_kit() {
        assert_eq!(KIT_ABI, nullrouter_adapter_kit::KIT_ABI);
    }

    #[test]
    fn every_kit_source_is_embedded() {
        let mut on_disk: Vec<String> = std::fs::read_dir(kit_dir().join("src"))
            .unwrap()
            .map(|e| format!("src/{}", e.unwrap().file_name().to_str().unwrap()))
            .collect();
        on_disk.sort();
        let mut embedded: Vec<String> = SOURCES.iter().map(|(p, _)| (*p).to_owned()).collect();
        embedded.sort();
        assert_eq!(on_disk, embedded);
    }

    #[test]
    fn the_normalised_dependencies_mirror_the_workspace() {
        let root: toml::Table = std::fs::read_to_string(kit_dir().join("../../Cargo.toml")).unwrap().parse().unwrap();
        let ws = &root["workspace"]["dependencies"];
        let mine: toml::Table = DEPENDENCIES.parse().unwrap();
        assert_eq!(ws["serde"], mine["serde"]);
        assert_eq!(ws["serde_json"], mine["serde_json"]);
    }

    #[test]
    fn the_normalised_manifest_parses_and_has_no_inheritance() {
        let text = normalised_manifest();
        assert!(!text.contains("workspace"));
        let parsed: toml::Table = text.parse().unwrap();
        assert_eq!(parsed["package"]["version"].as_str(), Some(kit_version().as_str()));
    }
}
