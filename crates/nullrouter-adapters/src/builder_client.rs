//! Runs the separate builder binary for one job (contracts/adapter-package.md, "After the
//! gate"). The core does not link the builder: the job and result JSON are mirrored here.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// The builder's name on `PATH` when `[adapters] builder` is not set.
pub const DEFAULT_BUILDER: &str = "nullrouter-builder";

const STDERR_TAIL_LINES: usize = 40;

/// One build job.
#[derive(Debug, Clone, Serialize)]
pub struct Job {
    pub source_dir: PathBuf,
    pub out_dir: PathBuf,
    pub kit: String,
    pub abi: u32,
}

/// Why the builder refused a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    Compile,
    Nondeterministic,
    Timeout,
    KitVersionUnavailable,
    ToolchainMissing,
}

impl Refusal {
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Compile => "compile",
            Self::Nondeterministic => "nondeterministic",
            Self::Timeout => "timeout",
            Self::KitVersionUnavailable => "kit_version_unavailable",
            Self::ToolchainMissing => "toolchain_missing",
        }
    }
}

#[derive(Debug, Deserialize)]
struct WireBuilt {
    #[allow(dead_code)]
    ok: bool,
    source_fp: String,
    wasm_hash: String,
    kit_abi: u32,
    toolchain: String,
}

#[derive(Debug, Deserialize)]
struct WireRefused {
    #[allow(dead_code)]
    ok: bool,
    error: Refusal,
    #[serde(default)]
    detail: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum WireResult {
    Built(WireBuilt),
    Refused(WireRefused),
}

/// What running the builder came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildOutcome {
    Built { source_fp: String, wasm_hash: String, kit_abi: u32, toolchain: String },
    /// The builder ran and refused the job.
    Refused { code: Refusal, detail: Option<String> },
    /// No builder binary at the configured path (`builder_not_installed`).
    NotInstalled,
    /// The wall-clock limit passed; the child was killed.
    TimedOut,
    /// Non-zero exit: the builder itself failed.
    BuilderFailed { status: Option<i32>, stderr_tail: String },
    /// Exit 0 but stdout was not a result.
    BadOutput { detail: String },
    /// Built, but for different source than the one that was gated.
    SourceMismatch { expected: String, got: String },
}

impl BuildOutcome {
    /// A stable snake_case code for records and alerts.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Built { .. } => "built",
            Self::Refused { code, .. } => code.code(),
            Self::NotInstalled => "builder_not_installed",
            Self::TimedOut => "builder_timeout",
            Self::BuilderFailed { .. } => "builder_failed",
            Self::BadOutput { .. } => "builder_bad_output",
            Self::SourceMismatch { .. } => "source_mismatch",
        }
    }
}

fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(STDERR_TAIL_LINES)..].join("\n")
}

/// Spawns `<builder> --home <home> build`, sends `job` on stdin and reads one result line.
/// `builder` is the configured path (`None`: `nullrouter-builder` on `PATH`).
pub async fn run_builder(
    builder: Option<&str>,
    home: &Path,
    job: &Job,
    expected_source_fp: &str,
    timeout: Duration,
) -> BuildOutcome {
    let program = builder.unwrap_or(DEFAULT_BUILDER);
    let mut child = match Command::new(program)
        .arg("--home")
        .arg(home)
        .arg("build")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return BuildOutcome::NotInstalled,
        Err(e) => {
            return BuildOutcome::BuilderFailed { status: None, stderr_tail: format!("spawn {program}: {e}") };
        }
    };
    let Ok(body) = serde_json::to_vec(job) else {
        return BuildOutcome::BadOutput { detail: "job did not serialise".to_owned() };
    };
    let stdin = child.stdin.take();
    let feed = async move {
        if let Some(mut stdin) = stdin {
            // A builder that exits early closes the pipe; its exit status tells the story.
            let _ = stdin.write_all(&body).await;
            let _ = stdin.shutdown().await;
        }
    };
    // `wait_with_output` consumes the child; dropping it on timeout kills it (`kill_on_drop`).
    let run = async {
        let (out, ()) = tokio::join!(child.wait_with_output(), feed);
        out
    };
    let output = match tokio::time::timeout(timeout, run).await {
        Err(_) => return BuildOutcome::TimedOut,
        Ok(Err(e)) => {
            return BuildOutcome::BuilderFailed { status: None, stderr_tail: format!("wait {program}: {e}") };
        }
        Ok(Ok(output)) => output,
    };
    if !output.status.success() {
        return BuildOutcome::BuilderFailed {
            status: output.status.code(),
            stderr_tail: tail(&String::from_utf8_lossy(&output.stderr)),
        };
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
    match serde_json::from_str::<WireResult>(line) {
        Err(e) => BuildOutcome::BadOutput { detail: e.to_string() },
        Ok(WireResult::Refused(r)) => BuildOutcome::Refused { code: r.error, detail: r.detail },
        Ok(WireResult::Built(b)) if b.source_fp != expected_source_fp => {
            BuildOutcome::SourceMismatch { expected: expected_source_fp.to_owned(), got: b.source_fp }
        }
        Ok(WireResult::Built(b)) => BuildOutcome::Built {
            source_fp: b.source_fp,
            wasm_hash: b.wasm_hash,
            kit_abi: b.kit_abi,
            toolchain: b.toolchain,
        },
    }
}
