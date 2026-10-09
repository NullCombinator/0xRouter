//! The builder against real fixtures. Each test needs the pinned toolchain and the wasm32 target,
//! so the tests skip with a message where the target is missing (CI installs it).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use nullrouter_builder::{BuildResult, Built, Job, KIT_ABI, Refusal, TARGET, TOOLCHAIN, build, builder_dir, kit_version, setup, source_fp};

static READY: OnceLock<Option<PathBuf>> = OnceLock::new();

fn wasm_target_installed() -> bool {
    let Ok(out) = Command::new("rustup").args(["target", "list", "--installed", "--toolchain", TOOLCHAIN]).output() else {
        return false;
    };
    out.status.success() && String::from_utf8_lossy(&out.stdout).lines().any(|l| l.trim() == TARGET)
}

/// The builder directory after one `setup`, shared by every test in this binary. `None` when the
/// wasm32 target is missing.
fn ready() -> Option<&'static Path> {
    READY
        .get_or_init(|| {
            if !wasm_target_installed() {
                eprintln!("skipping: wasm32-unknown-unknown for 1.93.1 is not installed");
                return None;
            }
            let home = tempfile::tempdir().expect("a temporary 0router home").keep();
            let dir = builder_dir(&home);
            setup(&dir).expect("the builder is set up");
            Some(dir)
        })
        .as_deref()
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(name)
}

fn noop() -> PathBuf {
    fixture("../nullrouter-adapters/tests/fixtures/noop")
}

fn job(source_dir: &Path, out_dir: &Path) -> Job {
    Job { source_dir: source_dir.to_path_buf(), out_dir: out_dir.to_path_buf(), kit: "1".into(), abi: KIT_ABI }
}

/// One build job with its own output directory.
fn run(dir: &Path, source: &Path) -> BuildResult {
    let out = tempfile::tempdir().expect("an output directory");
    build(dir, &job(source, out.path())).expect("the build runs")
}

#[test]
fn the_noop_fixture_builds() {
    let Some(dir) = ready() else { return };
    let source = noop();
    let out = tempfile::tempdir().expect("an output directory");
    let built: Built = match build(dir, &job(&source, out.path())).expect("the build runs") {
        BuildResult::Built(b) => b,
        other => panic!("the noop fixture was not built: {other:?}"),
    };
    assert!(built.ok);
    assert_eq!(built.kit_abi, KIT_ABI);
    assert_eq!(built.toolchain, "1.93.1-wasm32-unknown-unknown");
    assert_eq!(built.source_fp, source_fp(&source).expect("the source fingerprint"));
    assert!(out.path().join("module.wasm").is_file());
}

#[test]
fn two_builds_of_the_noop_fixture_give_the_same_wasm_hash() {
    let Some(dir) = ready() else { return };
    let source = noop();
    let first = match run(dir, &source) {
        BuildResult::Built(b) => b,
        other => panic!("the first build was not built: {other:?}"),
    };
    let second = match run(dir, &source) {
        BuildResult::Built(b) => b,
        other => panic!("the second build was not built: {other:?}"),
    };
    assert_eq!(first.wasm_hash, second.wasm_hash);
}

#[test]
fn a_broken_fixture_is_refused_with_a_short_compile_error() {
    let Some(dir) = ready() else { return };
    let refused = match run(dir, &fixture("tests/fixtures/broken")) {
        BuildResult::Refused(r) => r,
        other => panic!("the broken fixture was not refused: {other:?}"),
    };
    assert_eq!(refused.error, Refusal::Compile);
    let detail = refused.detail.as_deref().expect("the compiler output");
    assert!(detail.lines().count() <= 40, "the detail has {} lines", detail.lines().count());
    let json = serde_json::to_string(&refused).expect("the refusal as JSON");
    assert!(json.contains("\"ok\":false"), "{json}");
    assert!(json.contains("\"error\":\"compile\""), "{json}");
}

#[test]
fn an_unsafe_block_fails_to_compile() {
    let Some(dir) = ready() else { return };
    let refused = match run(dir, &fixture("tests/fixtures/unsafe_block")) {
        BuildResult::Refused(r) => r,
        other => panic!("the unsafe fixture was not refused: {other:?}"),
    };
    assert_eq!(refused.error, Refusal::Compile);
    let detail = refused.detail.as_deref().expect("the compiler output");
    assert!(detail.contains("unsafe"), "{detail}");
}

#[test]
fn the_kit_resolves_from_the_local_registry() {
    let Some(dir) = ready() else { return };
    let vendored = dir.join("vendor").join(format!("nullrouter-adapter-kit-{}", kit_version()));
    assert!(vendored.is_dir(), "{} is missing", vendored.display());

    let config = fs::read_to_string(dir.join("config.toml")).expect("config.toml");
    assert!(config.contains("replace-with = \"vendored\""), "{config}");

    let lock = fs::read_to_string(dir.join("Cargo.lock")).expect("Cargo.lock");
    let block = lock
        .split("[[package]]")
        .find(|b| b.lines().any(|l| l == "name = \"nullrouter-adapter-kit\""))
        .expect("the kit has a lock entry");
    assert!(block.contains("source = \"registry+https://github.com/rust-lang/crates.io-index\""), "{block}");
    assert!(block.contains("checksum = "), "{block}");
    assert!(!block.contains("path"), "{block}");
}
