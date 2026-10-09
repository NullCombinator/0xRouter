//! `setup`: the builder's one network use, run by the operator.
//!
//! Layout under `builder/` afterwards:
//! - `rust-toolchain.toml`: rustc 1.93.1 with `wasm32-unknown-unknown`;
//! - `vendor/`: `serde`, `serde_json` and their closure (from `cargo vendor`, with Cargo's own
//!   `.cargo-checksum.json`), plus the kit written from the embedded sources;
//! - `config.toml`: replaces crates-io with `vendor/` (the builds' `CARGO_HOME` is this dir);
//! - `Cargo.lock`: the pinned lock every build starts from.
//!
//! The closure is resolved by `cargo vendor` in a scratch "seed" project that depends on the
//! kit by path and starts from the workspace lock file, embedded at compile time, so every
//! version is pinned by a committed file.

use std::fs;
use std::path::Path;
use std::process::Command;

use crate::{BuildError, TARGET, TOOLCHAIN, kit, lock};

const SEED_NAME: &str = "nr-builder-seed";
const SEED_LOCK: &str = include_str!("../../../Cargo.lock");

fn remove(path: &Path) -> Result<(), BuildError> {
    let result = if path.is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) };
    match result {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(BuildError::io(path.display().to_string(), e)),
        _ => Ok(()),
    }
}

fn write(path: &Path, bytes: impl AsRef<[u8]>) -> Result<(), BuildError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| BuildError::io(parent.display().to_string(), e))?;
    }
    fs::write(path, bytes).map_err(|e| BuildError::io(path.display().to_string(), e))
}

fn run(mut cmd: Command, what: &str) -> Result<(), BuildError> {
    let out = cmd.output().map_err(|e| BuildError::Setup(format!("{what}: {e}")))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let tail: Vec<&str> = stderr.lines().rev().take(20).collect();
    let tail: Vec<&str> = tail.into_iter().rev().collect();
    Err(BuildError::Setup(format!("{what} failed ({}):\n{}", out.status, tail.join("\n"))))
}

/// Prepares `dir` (the builder directory). Safe to run again: it rebuilds everything.
pub fn setup(dir: &Path) -> Result<(), BuildError> {
    fs::create_dir_all(dir).map_err(|e| BuildError::io(dir.display().to_string(), e))?;
    let dir = dir.canonicalize().map_err(|e| BuildError::io(dir.display().to_string(), e))?;
    let vendor = dir.join("vendor");
    let seed = dir.join("seed");

    // A config left by an earlier run would redirect the vendoring below to the old directory.
    remove(&dir.join("config.toml"))?;
    remove(&vendor)?;
    remove(&seed)?;
    remove(&dir.join("Cargo.lock"))?;

    write(
        &dir.join("rust-toolchain.toml"),
        format!("[toolchain]\nchannel = \"{TOOLCHAIN}\"\ntargets = [\"{TARGET}\"]\nprofile = \"minimal\"\n"),
    )?;
    let mut rustup = Command::new("rustup");
    rustup.args(["toolchain", "install", TOOLCHAIN, "--profile", "minimal", "--target", TARGET]);
    run(rustup, "rustup toolchain install")?;

    // The seed: the kit by path, resolved from the committed lock.
    for (path, bytes) in kit::files() {
        write(&seed.join("kit").join(path), bytes)?;
    }
    write(
        &seed.join("Cargo.toml"),
        format!("[package]\nname = \"{SEED_NAME}\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[dependencies]\nnullrouter-adapter-kit = {{ path = \"kit\" }}\n"),
    )?;
    write(&seed.join("src/lib.rs"), "")?;
    write(&seed.join("Cargo.lock"), SEED_LOCK)?;

    let mut vendoring = Command::new("cargo");
    vendoring
        .current_dir(&seed)
        .env("CARGO_HOME", &dir)
        .args(["vendor", "--versioned-dirs"])
        .arg(&vendor);
    run(vendoring, "cargo vendor")?;

    // The kit joins the vendored crates.
    let (checksums, package) = kit::checksum_json();
    let kit_dir = vendor.join(kit::dir_name());
    for (path, bytes) in kit::files() {
        write(&kit_dir.join(path), bytes)?;
    }
    write(&kit_dir.join(".cargo-checksum.json"), checksums)?;

    let resolved = fs::read_to_string(seed.join("Cargo.lock")).map_err(|e| BuildError::io("seed Cargo.lock", e))?;
    write(&dir.join("Cargo.lock"), lock::pin(&resolved, SEED_NAME, "nullrouter-adapter-kit", &package))?;
    remove(&seed)?;

    let vendor_path = toml::Value::String(vendor.display().to_string());
    write(
        &dir.join("config.toml"),
        format!("[source.crates-io]\nreplace-with = \"vendored\"\n\n[source.vendored]\ndirectory = {vendor_path}\n"),
    )
}
