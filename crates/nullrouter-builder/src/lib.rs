//! The adapter builder: compiles reviewed adapter source to WASM, offline, against the kit
//! embedded in this crate (research R8). `setup` prepares the home once (network), `build`
//! runs one job. No unsafe code: the resource limits come from `prlimit(1)`.

mod build;
mod fingerprint;
mod kit;
mod lock;
mod setup;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use build::build;
pub use fingerprint::{source_fp, source_fp_of_files};
pub use kit::{KIT_ABI, kit_version};
pub use setup::setup;

/// The pinned compiler.
pub const TOOLCHAIN: &str = "1.93.1";
/// The only compile target.
pub const TARGET: &str = "wasm32-unknown-unknown";

/// The toolchain id recorded in `build.json`.
#[must_use]
pub fn toolchain_id() -> String {
    format!("{TOOLCHAIN}-{TARGET}")
}

/// `$NULLROUTER_HOME`, else `~/.0router`.
#[must_use]
pub fn default_home() -> PathBuf {
    if let Some(home) = std::env::var_os("NULLROUTER_HOME") {
        return PathBuf::from(home);
    }
    std::env::var_os("HOME").map_or_else(|| PathBuf::from(".0router"), |h| PathBuf::from(h).join(".0router"))
}

/// The builder's own directory inside a 0router home. It is also the build's `CARGO_HOME`.
#[must_use]
pub fn builder_dir(home: &Path) -> PathBuf {
    home.join("builder")
}

/// One build job (contracts/adapter-package.md, "After the gate").
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Job {
    pub source_dir: PathBuf,
    /// Receives `module.wasm` and `build.json` on success.
    pub out_dir: PathBuf,
    /// The kit requirement the package declares, such as `"1"`.
    pub kit: String,
    /// The ABI the caller wants.
    pub abi: u32,
}

/// Why a build produced no module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    Compile,
    Nondeterministic,
    Timeout,
    KitVersionUnavailable,
    ToolchainMissing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Built {
    pub ok: bool,
    pub source_fp: String,
    pub wasm_hash: String,
    pub kit_abi: u32,
    pub toolchain: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refused {
    pub ok: bool,
    pub error: Refusal,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// The JSON written to stdout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BuildResult {
    Built(Built),
    Refused(Refused),
}

impl BuildResult {
    pub(crate) fn refused(error: Refusal, detail: impl Into<Option<String>>) -> Self {
        Self::Refused(Refused { ok: false, error, detail: detail.into() })
    }
}

/// A failure of the builder itself, as opposed to a refusal of the job.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("{context}: {source}")]
    Io { context: String, source: std::io::Error },
    #[error("bad JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Source(String),
    #[error("setup failed: {0}")]
    Setup(String),
}

impl BuildError {
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io { context: context.into(), source }
    }
}
