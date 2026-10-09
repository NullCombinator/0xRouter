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

/// Whether this core's sandbox runs a module built for `abi`: the current major and the one
/// before it (the same rule as the sandbox's load gate).
fn abi_supported(abi: u32) -> bool {
    let current = nullrouter_adapter_kit::KIT_ABI;
    abi == current || (current > 1 && abi == current - 1)
}

/// The kit-upgrade step that runs before the adapters load (FR-032): every `approved` version
/// whose module was built for an ABI the sandbox no longer runs, or whose rebuild a stopped
/// server left unfinished, gets `rebuilding` set (and `rebuild_failed` cleared, so a failed one
/// is tried again). The state does not change. Returns the versions flagged.
pub fn flag_rebuilds(home: &Path) -> Result<Vec<(HarnessName, store::VersionId)>, InstallError> {
    flag_rebuilds_for(home, None)
}

/// [`flag_rebuilds`], limited to the harness `only` when given.
pub fn flag_rebuilds_for(
    home: &Path,
    only: Option<&HarnessName>,
) -> Result<Vec<(HarnessName, store::VersionId)>, InstallError> {
    let store = store::Store::open(home)?;
    let mut index = store.load_index()?;
    let mut targets = Vec::new();
    for h in index.harnesses.iter().filter(|h| only.is_none_or(|o| &h.name == o)) {
        for v in &h.versions {
            let stale = v.kit_abi.is_some_and(|a| !abi_supported(a));
            if v.state == store::VersionState::Approved && (v.rebuilding || stale) {
                targets.push((h.name.clone(), v.id.clone()));
            }
        }
    }
    for (name, id) in &targets {
        index.set_rebuild_flags(name, id, true, false)?;
    }
    if !targets.is_empty() {
        store.save_index(&index)?;
    }
    Ok(targets)
}

/// Rebuilds the approved versions built for an ABI that is no longer supported, from their
/// stored, reviewed source (FR-032, SC-013). Flags them first (see [`flag_rebuilds`]) and calls
/// `reload` so the flags take effect, then rebuilds one at a time through the builder, calling
/// `reload` after each. A rebuild must report the source's fingerprint: success replaces
/// `module.wasm` and `build.json` and clears the flag, with no state change and no review; any
/// failure sets `rebuild_failed` and raises a `rebuild_failed` alert, and the harness runs as a
/// plain client. Returns the versions that were rebuilt.
pub async fn startup_rebuilds(
    home: &Path,
    opts: &InstallOptions<'_>,
    reload: &(dyn Fn() + Sync),
) -> Result<Vec<(HarnessName, store::VersionId)>, InstallError> {
    rebuilds_for(home, opts, reload, None).await
}

/// [`startup_rebuilds`], limited to the harness `only` when given (`adapters rebuild <H>`).
pub async fn rebuilds_for(
    home: &Path,
    opts: &InstallOptions<'_>,
    reload: &(dyn Fn() + Sync),
    only: Option<&HarnessName>,
) -> Result<Vec<(HarnessName, store::VersionId)>, InstallError> {
    let targets = flag_rebuilds_for(home, only)?;
    if targets.is_empty() {
        return Ok(targets);
    }
    reload();
    let store = store::Store::open(home)?;
    let log = alerts::AlertLog::open(&store);
    let mut rebuilt = Vec::new();
    for (name, id) in targets {
        match rebuild_one(home, &store, &name, &id, opts).await {
            Ok(()) => rebuilt.push((name, id)),
            Err(codes) => {
                tracing::warn!("adapter {name} {id} not rebuilt for the new kit: {codes:?}");
                let mut index = store.load_index()?;
                index.set_rebuild_flags(&name, &id, false, true)?;
                store.save_index(&index)?;
                let alert = alerts::NewAlert {
                    kind: alerts::AlertKind::RebuildFailed,
                    harness: name.clone(),
                    version: id.clone(),
                    record: None,
                    message: "the rebuild for the new kit failed",
                    codes,
                };
                log.raise(alert, jiff::Timestamp::now())?;
            }
        }
        reload();
    }
    Ok(rebuilt)
}

/// One kit-upgrade rebuild. `Err` carries the alert codes.
async fn rebuild_one(
    home: &Path,
    store: &store::Store,
    name: &HarnessName,
    id: &store::VersionId,
    opts: &InstallOptions<'_>,
) -> Result<(), &'static [&'static str]> {
    const FAILED: &[&str] = &["rebuild_error"];
    let fail = |what: &str, e: &dyn fmt::Display| {
        tracing::error!("adapter {name} {id} rebuild: {what}: {e}");
        FAILED
    };
    let index = store.load_index().map_err(|e| fail("index", &e))?;
    let entry = index.version(name, id).ok_or(FAILED)?;
    // Only the stored, reviewed source is compiled: it must still be what was approved.
    if store.verify(name, entry).is_err() {
        return Err(&["source_mismatch"]);
    }
    let fp = entry.source_fp.clone();
    let source = store.version_dir(name, id).join("source");
    let text = std::fs::read_to_string(source.join("adapter.toml")).map_err(|e| fail("manifest", &e))?;
    let manifest = loader::Manifest::parse(&text).map_err(|e| fail("manifest", &e))?;
    let out = tempfile::tempdir().map_err(|e| fail("output directory", &e))?;
    let job = builder_client::Job {
        source_dir: source,
        out_dir: out.path().to_owned(),
        kit: manifest.kit,
        abi: nullrouter_adapter_kit::KIT_ABI,
    };
    let outcome = builder_client::run_builder(opts.builder, home, &job, fp.as_str(), opts.build_timeout).await;
    let builder_client::BuildOutcome::Built { wasm_hash, kit_abi, .. } = &outcome else {
        return Err(refusal_alert(&outcome).1);
    };
    if !abi_supported(*kit_abi) {
        return Err(&["kit_abi_unsupported"]);
    }
    let read = |file: &str| std::fs::read(out.path().join(file)).map_err(|e| fail(file, &e));
    let (wasm, build_json) = (read("module.wasm")?, read("build.json")?);
    if &nullrouter_sandbox::wasm_hash(&wasm) != wasm_hash {
        return Err(&["module_hash_mismatch"]);
    }
    store.write_version_file(name, id, "module.wasm", &wasm).map_err(|e| fail("module.wasm", &e))?;
    store.write_version_file(name, id, "build.json", &build_json).map_err(|e| fail("build.json", &e))?;
    let mut index = store.load_index().map_err(|e| fail("index", &e))?;
    index.set_built(name, id, wasm_hash, *kit_abi).map_err(|e| fail("index", &e))?;
    index.set_rebuild_flags(name, id, false, false).map_err(|e| fail("index", &e))?;
    store.save_index(&index).map_err(|e| fail("index", &e))?;
    Ok(())
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

/// What [`remove`] took out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removed {
    /// The versions deleted from the index and from disk.
    pub versions: Vec<store::VersionId>,
}

/// Removes one version of `harness`, or (`version` is `None`) all of them. The active version is
/// refused without `force`. A harness left without versions stays in the index as an empty
/// entry, so keys still bound to it are recorded `not_run{removed}` rather than
/// `no_approved_version`; installing a version again fills it. The index is saved before any
/// file is deleted. The caller reloads the engine afterwards.
pub fn remove(
    home: &Path,
    harness: &HarnessName,
    version: Option<&store::VersionId>,
    force: bool,
) -> Result<Removed, InstallError> {
    let store = store::Store::open(home)?;
    let mut index = store.load_index()?;
    let entry = index.harness(harness).ok_or_else(|| store::StoreError::UnknownHarness(harness.to_string()))?;
    let doomed: Vec<store::VersionId> = match version {
        Some(v) => {
            if index.version(harness, v).is_none() {
                return Err(store::StoreError::UnknownVersion {
                    harness: harness.to_string(),
                    version: v.to_string(),
                }
                .into());
            }
            if entry.active.as_ref() == Some(v) && !force {
                return Err(InstallError::Other(format!(
                    "{harness} {v} is the active version; pass --force to remove it"
                )));
            }
            vec![v.clone()]
        }
        None => entry.versions.iter().map(|e| e.id.clone()).collect(),
    };
    if let Some(h) = index.harnesses.iter_mut().find(|h| &h.name == harness) {
        h.versions.retain(|e| !doomed.contains(&e.id));
        if h.active.as_ref().is_some_and(|a| doomed.contains(a)) {
            h.active = None;
        }
    }
    store.save_index(&index)?;
    for id in &doomed {
        let dir = store.version_dir(harness, id);
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_err(&dir, e)),
        }
    }
    Ok(Removed { versions: doomed })
}
