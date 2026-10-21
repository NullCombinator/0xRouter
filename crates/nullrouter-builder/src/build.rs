//! `build`: one job, two cold builds, one comparison.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use std::os::unix::process::ExitStatusExt;

use sha2::{Digest, Sha256};

use crate::{BuildError, BuildResult, Built, Job, Refusal, TARGET, TOOLCHAIN, fingerprint, kit, lock, toolchain_id};

const KIT_NAME: &str = "nullrouter-adapter-kit";
const CPU_SECONDS: u64 = 120;
const ADDRESS_SPACE: u64 = 2 * 1024 * 1024 * 1024;
/// A backstop against a build that sleeps instead of computing, which the CPU limit never sees.
const WALL: Duration = Duration::from_secs(600);
const DETAIL_LINES: usize = 40;

enum Outcome {
    Wasm(Vec<u8>),
    Refused(Refusal, String),
}

fn hash(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn copy_tree(files: &[(String, Vec<u8>)], dest: &Path) -> Result<(), BuildError> {
    for (path, bytes) in files {
        let to = dest.join(path);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(|e| BuildError::io(parent.display().to_string(), e))?;
        }
        fs::write(&to, bytes).map_err(|e| BuildError::io(to.display().to_string(), e))?;
    }
    Ok(())
}

fn first_lines(text: &str) -> String {
    text.lines().take(DETAIL_LINES).collect::<Vec<_>>().join("\n")
}

/// Runs one `cargo build` of `project` into `target_dir`, under `prlimit`.
fn compile(dir: &Path, project: &Path, target_dir: &Path, lib_name: &str, logs: &Path) -> Result<Outcome, BuildError> {
    let vendor = dir.join("vendor");
    let flags = [
        "-F".to_owned(),
        "unsafe_code".to_owned(),
        format!("--remap-path-prefix={}=/src", project.display()),
        format!("--remap-path-prefix={}=/vendor", vendor.display()),
        "-C".to_owned(),
        "debuginfo=0".to_owned(),
        "-C".to_owned(),
        "strip=symbols".to_owned(),
    ]
    .join("\u{1f}");

    let out_log = logs.join("stdout.log");
    let err_log = logs.join("stderr.log");
    let open = |p: &Path| File::create(p).map_err(|e| BuildError::io(p.display().to_string(), e));

    let mut cmd = Command::new("prlimit");
    cmd.arg(format!("--cpu={CPU_SECONDS}"))
        .arg(format!("--as={ADDRESS_SPACE}"))
        .args(["--", "cargo", "build", "--release", "--offline", "--locked", "--target", TARGET])
        .current_dir(project)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("CARGO_HOME", dir)
        .env("CARGO_TARGET_DIR", target_dir)
        .env("CARGO_ENCODED_RUSTFLAGS", flags)
        .env("CARGO_TERM_COLOR", "never")
        // Selects the pinned compiler (env_clear leaves rustup no other hint); RUSTUP_HOME below.
        .env("RUSTUP_TOOLCHAIN", TOOLCHAIN)
        .stdin(Stdio::null())
        .stdout(Stdio::from(open(&out_log)?))
        .stderr(Stdio::from(open(&err_log)?));
    if let Some(home) = rustup_home() {
        cmd.env("RUSTUP_HOME", home);
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Outcome::Refused(Refusal::ToolchainMissing, "prlimit or cargo was not found on PATH".into()));
        }
        Err(e) => return Err(BuildError::io("starting cargo", e)),
    };
    let started = Instant::now();
    let status = loop {
        match child.try_wait().map_err(|e| BuildError::io("waiting for cargo", e))? {
            Some(s) => break s,
            None if started.elapsed() > WALL => {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(Outcome::Refused(Refusal::Timeout, "the build ran past its wall-time cap".into()));
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    };
    let stderr = fs::read_to_string(&err_log).unwrap_or_default();

    if status.success() {
        let wasm = target_dir.join(TARGET).join("release").join(format!("{lib_name}.wasm"));
        return fs::read(&wasm).map(Outcome::Wasm).map_err(|e| BuildError::io(wasm.display().to_string(), e));
    }
    // SIGXCPU is the CPU limit; SIGKILL follows it when the process ignores that. Cargo also
    // reports a child's signal in its own message.
    let limited = matches!(status.signal(), Some(24 | 9))
        || stderr.contains("SIGXCPU")
        || stderr.contains("signal: 9")
        || stderr.contains("Cannot allocate memory")
        || stderr.contains("memory allocation of");
    if limited {
        return Ok(Outcome::Refused(Refusal::Timeout, first_lines(&stderr)));
    }
    if stderr.contains("target may not be installed")
        || stderr.contains("is not installed")
        || stderr.contains("rustup could not choose")
    {
        return Ok(Outcome::Refused(Refusal::ToolchainMissing, first_lines(&stderr)));
    }
    Ok(Outcome::Refused(Refusal::Compile, first_lines(&stderr)))
}

fn rustup_home() -> Option<PathBuf> {
    if let Some(h) = std::env::var_os("RUSTUP_HOME") {
        return Some(PathBuf::from(h));
    }
    let default = PathBuf::from(std::env::var_os("HOME")?).join(".rustup");
    default.is_dir().then_some(default)
}

fn package_identity(manifest: &[u8]) -> Result<(String, String), BuildError> {
    let text = std::str::from_utf8(manifest).map_err(|_| BuildError::Source("Cargo.toml is not UTF-8".into()))?;
    let table: toml::Table = text.parse().map_err(|e| BuildError::Source(format!("Cargo.toml: {e}")))?;
    let field = |k: &str| {
        table
            .get("package")
            .and_then(|p| p.get(k))
            .and_then(toml::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| BuildError::Source(format!("Cargo.toml has no package.{k}")))
    };
    Ok((field("name")?, field("version")?))
}

/// Runs `job` with the builder prepared in `dir` (see [`crate::setup`]).
pub fn build(dir: &Path, job: &Job) -> Result<BuildResult, BuildError> {
    let kit_version = kit::kit_version();
    let major = kit_version.split('.').next().unwrap_or("");
    if job.kit != major || job.abi != kit::KIT_ABI {
        return Ok(BuildResult::refused(
            Refusal::KitVersionUnavailable,
            format!("this builder carries kit {kit_version} (ABI {}); the job wants kit {} ABI {}", kit::KIT_ABI, job.kit, job.abi),
        ));
    }
    let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let ready = ["rust-toolchain.toml", "config.toml", "Cargo.lock"].iter().all(|f| dir.join(f).is_file())
        && dir.join("vendor").join(kit::dir_name()).is_dir();
    if !ready {
        return Ok(BuildResult::refused(Refusal::ToolchainMissing, "the builder is not set up; run `nullrouter-builder setup`".to_owned()));
    }

    let files = fingerprint::read_tree(&job.source_dir)?;
    let source_fp = fingerprint::source_fp_of_files(&files);
    let manifest = files
        .iter()
        .find(|(p, _)| p == "Cargo.toml")
        .ok_or_else(|| BuildError::Source("the source has no Cargo.toml".into()))?;
    let (name, version) = package_identity(&manifest.1)?;

    let work_root = dir.join("work");
    fs::create_dir_all(&work_root).map_err(|e| BuildError::io(work_root.display().to_string(), e))?;
    let work = tempfile::Builder::new()
        .prefix("job-")
        .tempdir_in(&work_root)
        .map_err(|e| BuildError::io("creating the work directory", e))?;
    let project = work.path().join("src");
    let own: Vec<(String, Vec<u8>)> = files.into_iter().filter(|(p, _)| p != "Cargo.lock").collect();
    copy_tree(&own, &project)?;

    // The author may not set `crate-type`; the builder does, through a generated [lib].
    let mut cargo_toml = fs::read_to_string(project.join("Cargo.toml")).map_err(|e| BuildError::io("Cargo.toml", e))?;
    cargo_toml.push_str("\n[lib]\ncrate-type = [\"cdylib\"]\n");
    fs::write(project.join("Cargo.toml"), cargo_toml).map_err(|e| BuildError::io("Cargo.toml", e))?;
    let pinned = fs::read_to_string(dir.join("Cargo.lock")).map_err(|e| BuildError::io("Cargo.lock", e))?;
    fs::write(project.join("Cargo.lock"), lock::with_root(&pinned, &name, &version, KIT_NAME))
        .map_err(|e| BuildError::io("Cargo.lock", e))?;

    let lib_name = name.replace('-', "_");
    let logs = work.path().join("logs");
    fs::create_dir_all(&logs).map_err(|e| BuildError::io("logs", e))?;
    let mut hashes = Vec::new();
    let mut module = Vec::new();
    for round in ["target-1", "target-2"] {
        match compile(&dir, &project, &work.path().join(round), &lib_name, &logs)? {
            Outcome::Wasm(bytes) => {
                hashes.push(hash(&bytes));
                module = bytes;
            }
            Outcome::Refused(why, detail) => return Ok(BuildResult::refused(why, detail)),
        }
    }
    if hashes[0] != hashes[1] {
        return Ok(BuildResult::refused(Refusal::Nondeterministic, format!("{} then {}", hashes[0], hashes[1])));
    }

    let built = Built {
        ok: true,
        source_fp,
        wasm_hash: hashes.remove(0),
        kit_abi: kit::KIT_ABI,
        toolchain: toolchain_id(),
    };
    fs::create_dir_all(&job.out_dir).map_err(|e| BuildError::io(job.out_dir.display().to_string(), e))?;
    fs::write(job.out_dir.join("module.wasm"), &module).map_err(|e| BuildError::io("module.wasm", e))?;
    let record = serde_json::json!({
        "source_fp": built.source_fp,
        "wasm_hash": built.wasm_hash,
        "kit_abi": built.kit_abi,
        "toolchain": built.toolchain,
        "built": jiff::Timestamp::now().to_string(),
    });
    fs::write(job.out_dir.join("build.json"), serde_json::to_vec_pretty(&record)?)
        .map_err(|e| BuildError::io("build.json", e))?;
    Ok(BuildResult::Built(built))
}
