//! `run_builder` against fake builders: shell scripts that print a fixed result.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use nullrouter_adapters::builder_client::{run_builder, BuildOutcome, Job, Refusal};

const FP: &str = "sha256:aaaa";
const BUILT: &str = r#"{"ok":true,"source_fp":"sha256:aaaa","wasm_hash":"sha256:bbbb","kit_abi":1,"toolchain":"1.93.1-wasm32-unknown-unknown"}"#;

fn fake(dir: &Path, name: &str, body: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.to_str().unwrap().to_owned()
}

fn job() -> Job {
    Job { source_dir: "/src".into(), out_dir: "/out".into(), kit: "1".into(), abi: 1 }
}

/// A script that records its stdin and arguments, then prints `line`.
fn recording(dir: &Path, line: &str) -> String {
    let body = format!(
        "cat > '{d}/stdin.json'\necho \"$@\" > '{d}/args.txt'\ncat <<'EOF'\n{line}\nEOF",
        d = dir.display()
    );
    fake(dir, "builder.sh", &body)
}

async fn run(dir: &Path, builder: &str, timeout: Duration) -> BuildOutcome {
    run_builder(Some(builder), &dir.join("home"), &job(), FP, timeout).await
}

#[tokio::test]
async fn built_and_receives_args_and_job() {
    let dir = tempfile::tempdir().unwrap();
    let b = recording(dir.path(), BUILT);
    let out = run(dir.path(), &b, Duration::from_secs(10)).await;
    assert_eq!(
        out,
        BuildOutcome::Built {
            source_fp: FP.into(),
            wasm_hash: "sha256:bbbb".into(),
            kit_abi: 1,
            toolchain: "1.93.1-wasm32-unknown-unknown".into(),
        }
    );
    let args = std::fs::read_to_string(dir.path().join("args.txt")).unwrap();
    assert_eq!(args.trim(), format!("--home {} build", dir.path().join("home").display()));
    let sent: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("stdin.json")).unwrap()).unwrap();
    assert_eq!(sent["source_dir"], "/src");
    assert_eq!(sent["out_dir"], "/out");
    assert_eq!(sent["kit"], "1");
    assert_eq!(sent["abi"], 1);
}

#[tokio::test]
async fn refused_compile() {
    let dir = tempfile::tempdir().unwrap();
    let b = recording(dir.path(), r#"{"ok":false,"error":"compile","detail":"E0308"}"#);
    let out = run(dir.path(), &b, Duration::from_secs(10)).await;
    assert_eq!(out, BuildOutcome::Refused { code: Refusal::Compile, detail: Some("E0308".into()) });
    assert_eq!(out.code(), "compile");
}

#[tokio::test]
async fn timeout_kills() {
    let dir = tempfile::tempdir().unwrap();
    let b = fake(dir.path(), "slow.sh", "sleep 30");
    let out = run(dir.path(), &b, Duration::from_millis(300)).await;
    assert_eq!(out, BuildOutcome::TimedOut);
}

#[tokio::test]
async fn nonzero_exit_reports_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let b = fake(dir.path(), "bad.sh", "echo boom >&2\nexit 1");
    let out = run(dir.path(), &b, Duration::from_secs(10)).await;
    assert_eq!(out, BuildOutcome::BuilderFailed { status: Some(1), stderr_tail: "boom".into() });
}

#[tokio::test]
async fn garbage_output() {
    let dir = tempfile::tempdir().unwrap();
    let b = fake(dir.path(), "garbage.sh", "echo not json");
    let out = run(dir.path(), &b, Duration::from_secs(10)).await;
    assert!(matches!(out, BuildOutcome::BadOutput { .. }), "{out:?}");
}

#[tokio::test]
async fn different_source_fp_is_a_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let b = recording(dir.path(), &BUILT.replace("aaaa", "cccc"));
    let out = run(dir.path(), &b, Duration::from_secs(10)).await;
    assert_eq!(out, BuildOutcome::SourceMismatch { expected: FP.into(), got: "sha256:cccc".into() });
    assert_eq!(out.code(), "source_mismatch");
}

#[tokio::test]
async fn missing_binary_is_not_installed() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope").to_str().unwrap().to_owned();
    let out = run(dir.path(), &missing, Duration::from_secs(1)).await;
    assert_eq!(out, BuildOutcome::NotInstalled);
    assert_eq!(out.code(), "builder_not_installed");
}
