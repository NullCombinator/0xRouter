//! Harness adapters: the built-in hermes adapter and the pipeline for third-party ones.

pub mod alerts;
pub mod apply;
pub mod builder_client;
pub mod builtin;
pub mod catalogue;
pub mod fingerprint;
pub mod gate;
pub mod guard;
pub mod loader;
pub mod record;
pub mod review;
pub mod runner;
pub mod scramble;
pub mod selector;
pub mod store;
#[cfg(feature = "testkit")]
pub mod testkit;
pub mod unpack;

use std::fmt;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Adapters that ship in the core.
pub const BUILTIN: &[&str] = &["hermes"];
/// Names kept for harnesses that are handled elsewhere or are not ours to adapt.
pub const RESERVED: &[&str] = &["opencode", "grok-build", "zcode"];

/// The name of a harness a key is bound to: `^[a-z][a-z0-9-]{1,31}$`, and not reserved.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct HarnessName(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HarnessNameError {
    #[error("harness names are 2-32 characters: a lowercase letter, then lowercase letters, digits or '-' ({0:?})")]
    Malformed(String),
    #[error("{0} is a reserved harness name")]
    Reserved(String),
}

impl HarnessName {
    pub fn new(s: &str) -> Result<Self, HarnessNameError> {
        let b = s.as_bytes();
        let ok = (2..=32).contains(&b.len())
            && b[0].is_ascii_lowercase()
            && b[1..].iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-');
        if !ok {
            return Err(HarnessNameError::Malformed(s.to_owned()));
        }
        if RESERVED.contains(&s) {
            return Err(HarnessNameError::Reserved(s.to_owned()));
        }
        Ok(Self(s.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_builtin(&self) -> bool {
        BUILTIN.contains(&self.0.as_str())
    }
}

impl fmt::Display for HarnessName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for HarnessName {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::new(&s).map_err(serde::de::Error::custom)
    }
}

/// How long the builder has for one job.
pub const BUILD_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_REASON_CHARS: usize = 1_000;

/// What `install` needs besides the package.
pub struct InstallOptions<'a> {
    /// The loaded client style ids the gate checks `adapter.toml` against.
    pub styles: &'a [&'a str],
    /// The builder's path (`None`: `nullrouter-builder` on `PATH`).
    pub builder: Option<&'a str>,
    pub origin: store::Origin,
    pub build_timeout: Duration,
}

/// Where an install ended. When `state` is `in_review` the caller enqueues the review: this crate
/// makes no model request, so the queue (the engine's) is the caller's to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub harness: HarnessName,
    pub version: store::VersionId,
    pub state: store::VersionState,
    /// Why it is not further along: the refusal reasons, or `builder_not_installed`.
    pub reason: String,
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error(transparent)]
    Unpack(#[from] unpack::UnpackError),
    #[error(transparent)]
    Store(#[from] store::StoreError),
    #[error(transparent)]
    Alert(#[from] alerts::AlertError),
    #[error(transparent)]
    Harness(#[from] HarnessNameError),
    /// The gate refused and the package named no harness or version to record the refusal under.
    #[error("refused: {}", join_reasons(.0))]
    Refused(Vec<gate::Reason>),
    #[error("{0}")]
    Other(String),
}

fn join_reasons(reasons: &[gate::Reason]) -> String {
    let text = reasons.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ");
    text.chars().take(MAX_REASON_CHARS).collect()
}

fn io_err(path: &Path, source: std::io::Error) -> InstallError {
    InstallError::Store(store::StoreError::Io { path: path.to_owned(), source })
}

/// Installs the package at `input` (a directory or a `.tar.gz`) under `home`: unpack, gate,
/// fingerprint, store the source, build, and leave the version `in_review`. A refusal is a
/// recorded `refused` version and a `refused` alert, and returns `Ok` with that state. A missing
/// builder leaves the version `queued`.
pub async fn install(home: &Path, input: &Path, opts: &InstallOptions<'_>) -> Result<Installed, InstallError> {
    use alerts::{AlertKind, AlertLog, NewAlert};
    use store::{Store, VersionEntry, VersionId, VersionState};

    let files = unpack::read_package(input)?;
    // The gate reads a private copy of exactly the bytes that are fingerprinted and stored.
    let staging = tempfile::tempdir().map_err(|e| io_err(Path::new("staging"), e))?;
    unpack::write_tree(&files, staging.path())?;
    let fp = fingerprint::of_files(&files);
    let store = Store::open(home)?;
    let log = AlertLog::open(&store);
    let submitted = jiff::Timestamp::now().to_string();

    let gated = match gate::check(staging.path(), opts.styles) {
        Ok(g) => g,
        Err(reasons) => {
            let Some((name, semver)) = identity(&files) else { return Err(InstallError::Refused(reasons)) };
            let id = VersionId::new(&semver, &fp);
            let mut index = store.load_index()?;
            let entry = VersionEntry {
                id: id.clone(),
                semver: semver.to_string(),
                state: VersionState::Queued,
                source_fp: fp,
                wasm_hash: None,
                kit_abi: None,
                submitted,
                state_reason: String::new(),
                rebuilding: false,
                rebuild_failed: false,
                origin: opts.origin.clone(),
            };
            index.submit(&name, entry)?;
            let reason = join_reasons(&reasons);
            index.transition(&name, &id, VersionState::Refused, &reason)?;
            store.save_index(&index)?;
            let alert = NewAlert {
                kind: AlertKind::Refused,
                harness: name.clone(),
                version: id.clone(),
                record: None,
                message: "the gate refused the package",
                codes: &["gate_refused"],
            };
            log.raise(alert, jiff::Timestamp::now())?;
            return Ok(Installed { harness: name, version: id, state: VersionState::Refused, reason });
        }
    };

    let name = HarnessName::new(&gated.manifest.harness)?;
    let semver = semver::Version::parse(&gated.package_version)
        .map_err(|e| InstallError::Other(format!("package version {:?}: {e}", gated.package_version)))?;
    let id = VersionId::new(&semver, &fp);
    for (path, bytes) in &files {
        store.write_version_file(&name, &id, &format!("source/{path}"), bytes)?;
    }
    let mut index = store.load_index()?;
    let entry = VersionEntry {
        id: id.clone(),
        semver: semver.to_string(),
        state: VersionState::Queued,
        source_fp: fp.clone(),
        wasm_hash: None,
        kit_abi: None,
        submitted,
        state_reason: String::new(),
        rebuilding: false,
        rebuild_failed: false,
        origin: opts.origin.clone(),
    };
    index.submit(&name, entry)?;

    store.save_index(&index)?;
    build(home, &name, &id, opts).await
}

/// Builds a stored `queued` version: builder check, `building`, the build, the hash check, the
/// module stored, `in_review`. `install` ends here; `adapters build --retry` calls it once a
/// missing builder is installed. Only a `queued` version can be built.
pub async fn build(
    home: &Path,
    name: &HarnessName,
    id: &store::VersionId,
    opts: &InstallOptions<'_>,
) -> Result<Installed, InstallError> {
    use alerts::{AlertLog, NewAlert};
    use store::{Store, VersionState};

    let store = Store::open(home)?;
    let log = AlertLog::open(&store);
    let mut index = store.load_index()?;
    let entry = index
        .version(name, id)
        .ok_or_else(|| store::StoreError::UnknownVersion { harness: name.to_string(), version: id.to_string() })?;
    if entry.state != VersionState::Queued {
        return Err(store::StoreError::Transition { from: entry.state, to: VersionState::Building }.into());
    }
    let fp = entry.source_fp.clone();
    let source = store.version_dir(name, id).join("source");
    let manifest_path = source.join("adapter.toml");
    let text = std::fs::read_to_string(&manifest_path).map_err(|e| io_err(&manifest_path, e))?;
    let manifest = loader::Manifest::parse(&text).map_err(InstallError::Other)?;
    let (name, id) = (name.clone(), id.clone());

    let done = |state, reason: &str| Installed {
        harness: name.clone(),
        version: id.clone(),
        state,
        reason: reason.to_owned(),
    };
    if !builder_present(opts.builder) {
        index.set_reason(&name, &id, "builder_not_installed")?;
        store.save_index(&index)?;
        return Ok(done(VersionState::Queued, "builder_not_installed"));
    }
    index.transition(&name, &id, VersionState::Building, "")?;
    store.save_index(&index)?;

    let out = tempfile::tempdir().map_err(|e| io_err(Path::new("build output"), e))?;
    let job = builder_client::Job {
        source_dir: source,
        out_dir: out.path().to_owned(),
        kit: manifest.kit,
        abi: nullrouter_adapter_kit::KIT_ABI,
    };
    let outcome = builder_client::run_builder(opts.builder, home, &job, fp.as_str(), opts.build_timeout).await;
    let builder_client::BuildOutcome::Built { wasm_hash, kit_abi, .. } = &outcome else {
        let mut reason = outcome.code().to_owned();
        if let builder_client::BuildOutcome::Refused { detail: Some(d), .. } = &outcome {
            reason = format!("{reason}: {d}");
        }
        let reason: String = reason.chars().take(MAX_REASON_CHARS).collect();
        index = store.load_index()?;
        index.transition(&name, &id, VersionState::Refused, &reason)?;
        store.save_index(&index)?;
        let (kind, codes) = refusal_alert(&outcome);
        let alert = NewAlert {
            kind,
            harness: name.clone(),
            version: id.clone(),
            record: None,
            message: "the build was refused",
            codes,
        };
        log.raise(alert, jiff::Timestamp::now())?;
        return Ok(done(VersionState::Refused, &reason));
    };

    // The module is stored only if it is the one the builder named.
    let read = |file: &str| std::fs::read(out.path().join(file)).map_err(|e| io_err(&out.path().join(file), e));
    let (wasm, build_json) = (read("module.wasm")?, read("build.json")?);
    if &nullrouter_sandbox::wasm_hash(&wasm) != wasm_hash {
        index = store.load_index()?;
        index.transition(&name, &id, VersionState::Refused, "module_hash_mismatch")?;
        store.save_index(&index)?;
        return Ok(done(VersionState::Refused, "module_hash_mismatch"));
    }
    store.write_version_file(&name, &id, "module.wasm", &wasm)?;
    store.write_version_file(&name, &id, "build.json", &build_json)?;
    index = store.load_index()?;
    index.set_built(&name, &id, wasm_hash, *kit_abi)?;
    index.transition(&name, &id, VersionState::InReview, "")?;
    store.save_index(&index)?;
    Ok(done(VersionState::InReview, ""))
}

/// The harness and package version a package names, read leniently, so a refused package can
/// still be recorded. `None`: it names neither validly.
fn identity(files: &unpack::Files) -> Option<(HarnessName, semver::Version)> {
    let text = |path: &str| {
        let (_, bytes) = files.iter().find(|(p, _)| p == path)?;
        std::str::from_utf8(bytes).ok()
    };
    let manifest = loader::Manifest::parse(text("adapter.toml")?).ok()?;
    let cargo: toml::Table = toml::from_str(text("Cargo.toml")?).ok()?;
    let version = cargo.get("package")?.get("version")?.as_str()?;
    Some((HarnessName::new(&manifest.harness).ok()?, semver::Version::parse(version).ok()?))
}

fn refusal_alert(outcome: &builder_client::BuildOutcome) -> (alerts::AlertKind, &'static [&'static str]) {
    use alerts::AlertKind::{Refused, SourceMismatch};
    use builder_client::{BuildOutcome as O, Refusal as R};
    match outcome {
        O::SourceMismatch { .. } => (SourceMismatch, &["source_mismatch"]),
        O::Refused { code: R::Compile, .. } => (Refused, &["compile"]),
        O::Refused { code: R::Nondeterministic, .. } => (Refused, &["nondeterministic"]),
        O::Refused { code: R::Timeout, .. } | O::TimedOut => (Refused, &["builder_timeout"]),
        O::Refused { code: R::KitVersionUnavailable, .. } => (Refused, &["kit_version_unavailable"]),
        O::Refused { code: R::ToolchainMissing, .. } => (Refused, &["toolchain_missing"]),
        O::NotInstalled => (Refused, &["builder_not_installed"]),
        O::BadOutput { .. } => (Refused, &["builder_bad_output"]),
        O::Built { .. } | O::BuilderFailed { .. } => (Refused, &["builder_failed"]),
    }
}

/// Whether the builder can be found: a path that is a file, or a name on `PATH`.
fn builder_present(builder: Option<&str>) -> bool {
    let program = builder.unwrap_or(builder_client::DEFAULT_BUILDER);
    if program.contains('/') {
        return Path::new(program).is_file();
    }
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(program).is_file()))
}

/// The operator's approval of a `reported` version: writes `decision.json`, makes the version
/// the active one and supersedes the one that was. The caller reloads the engine afterwards.
pub fn approve(
    home: &Path,
    harness: &HarnessName,
    version: &store::VersionId,
    note: Option<&str>,
) -> Result<(), InstallError> {
    let store = store::Store::open(home)?;
    let mut index = store.load_index()?;
    // Fail on a wrong state before anything is written.
    let entry = index.version(harness, version).ok_or_else(|| store::StoreError::UnknownVersion {
        harness: harness.to_string(),
        version: version.to_string(),
    })?;
    if !entry.state.can_become(store::VersionState::Approved) {
        return Err(store::StoreError::Transition { from: entry.state, to: store::VersionState::Approved }.into());
    }
    store.verify(harness, entry)?;
    let mut decision = serde_json::json!({"decision": "approve", "at": jiff::Timestamp::now().to_string()});
    if let Some(note) = note {
        decision["note"] = note.into();
    }
    let bytes = serde_json::to_vec_pretty(&decision).map_err(|e| InstallError::Other(e.to_string()))?;
    store.write_version_file(harness, version, "decision.json", &bytes)?;
    index.approve(harness, version)?;
    store.save_index(&index)?;
    Ok(())
}

/// The operator's rejection of a `reported` version: writes `decision.json` and moves it to
/// `rejected`. The caller reloads the engine afterwards.
pub fn reject(
    home: &Path,
    harness: &HarnessName,
    version: &store::VersionId,
    note: Option<&str>,
) -> Result<(), InstallError> {
    let store = store::Store::open(home)?;
    let mut index = store.load_index()?;
    let entry = index.version(harness, version).ok_or_else(|| store::StoreError::UnknownVersion {
        harness: harness.to_string(),
        version: version.to_string(),
    })?;
    if !entry.state.can_become(store::VersionState::Rejected) {
        return Err(store::StoreError::Transition { from: entry.state, to: store::VersionState::Rejected }.into());
    }
    let mut decision = serde_json::json!({"decision": "reject", "at": jiff::Timestamp::now().to_string()});
    if let Some(note) = note {
        decision["note"] = note.into();
    }
    let bytes = serde_json::to_vec_pretty(&decision).map_err(|e| InstallError::Other(e.to_string()))?;
    store.write_version_file(harness, version, "decision.json", &bytes)?;
    index.transition(harness, version, store::VersionState::Rejected, "rejected by the operator")?;
    store.save_index(&index)?;
    Ok(())
}
