//! `nullrouter adapters` (contracts/operator-cli.md): the third-party adapter store.
//!
//! Reviews belong to the server: the CLI never runs one, because its process would drop the queue
//! at exit. After a step that leaves a version `in_review` it asks the running server to review
//! it; with no server the review starts when `serve` starts (`Engine::resume_reviews`).

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use clap::Subcommand;
use nullrouter_adapters::alerts::{Alert, AlertKind, AlertLog};
use nullrouter_adapters::loader::Manifest;
use nullrouter_adapters::review::ReviewReport;
use nullrouter_adapters::store::{Index, ReviewConfig, Store, VersionEntry, VersionId, VersionState};
use nullrouter_adapters::{BUILD_TIMEOUT, BUILTIN, HarnessName, InstallError, InstallOptions, Installed};
use nullrouter_engine::keys::{self, Keys};
use nullrouter_engine::state::Engine;
use nullrouter_registry::OperatorHome;
use nullrouter_server::operator::{self, CallError};
use serde_json::{Value, json};

const REFUSED: u8 = 3;

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Each harness with an adapter, its versions and state. hermes shows as `built-in`.
    List,
    /// Gate, store and build a package (a directory or a `.tar.gz`); the review follows.
    Install { package: PathBuf },
    /// The manifest, state, report and decision of a version (default: the active one).
    Show { harness: String, version: Option<String> },
    /// Show the review report. `--retry` requeues a quarantined review.
    Review {
        harness: String,
        version: String,
        #[arg(long)]
        retry: bool,
    },
    /// Requeue a build that waited for the builder.
    Build {
        harness: String,
        version: String,
        #[arg(long)]
        retry: bool,
    },
    /// Approve a reported version; it becomes the active one.
    Approve {
        harness: String,
        version: String,
        #[arg(long)]
        note: Option<String>,
    },
    /// Reject a reported version.
    Reject {
        harness: String,
        version: String,
        #[arg(long)]
        note: Option<String>,
    },
    /// Clear a suspect version so it serves again. Lists its guardrail events first.
    Clear {
        harness: String,
        version: String,
        #[arg(long)]
        yes: bool,
    },
    /// Set or clear the model that reviews adapter source.
    ReviewSettings {
        /// A unified model id or `provider/model`.
        #[arg(long, requires = "budget", conflicts_with = "clear")]
        model: Option<String>,
        /// The most input tokens one review may send.
        #[arg(long, requires = "model")]
        budget: Option<u64>,
        /// Tokens kept free for the answer.
        #[arg(long, requires = "model")]
        reserve_output: Option<u32>,
        #[arg(long)]
        clear: bool,
    },
    /// Rebuild approved versions whose module was built for a kit the core no longer runs, from
    /// their reviewed source, in the foreground (what `serve` does at startup).
    Rebuild { harness: Option<String> },
    /// Remove a version, or the whole harness. Keys bound to it become plain clients.
    Remove {
        harness: String,
        version: Option<String>,
        /// Allow removing the active version.
        #[arg(long)]
        force: bool,
        #[arg(long)]
        yes: bool,
    },
}

fn fail(e: impl std::fmt::Display) -> ExitCode {
    eprintln!("{e}");
    ExitCode::from(1)
}

fn harness_of(s: &str) -> Result<HarnessName, ExitCode> {
    HarnessName::new(s).map_err(fail)
}

fn store_of(home: &OperatorHome) -> Result<Store, ExitCode> {
    Store::open(home.path()).map_err(fail)
}

/// The index, empty while there is no `adapters/` (a read creates nothing).
fn read_index(home: &OperatorHome) -> Result<Index, ExitCode> {
    if !home.path().join("adapters").is_dir() {
        return Ok(Index::default());
    }
    store_of(home)?.load_index().map_err(fail)
}

fn emit(as_json: bool, value: &Value, text: &str) {
    if as_json {
        println!("{value:#}");
    } else {
        println!("{text}");
    }
}

/// Asks the running server to reload. `applied`, or `saved; applies at next start`.
fn reload(home: &OperatorHome) -> Result<&'static str, ExitCode> {
    match operator::call(home, &json!({"op": "reload"})) {
        Ok(a) if a["ok"] == true => Ok("applied"),
        Ok(a) => Err(fail(a["error"].as_str().unwrap_or("the server refused the reload"))),
        Err(CallError::NoServer(_)) => Ok("saved; applies at next start"),
        Err(e) => Err(fail(e)),
    }
}

/// After a step that left a version `in_review`: reload, then have the server review it. With no
/// server, the review starts when `serve` does.
pub(crate) fn start_review(
    home: &OperatorHome,
    harness: &HarnessName,
    version: &VersionId,
) -> Result<&'static str, ExitCode> {
    if reload(home)? != "applied" {
        return Ok("the review starts when `serve` starts");
    }
    let req = json!({"op": "adapters.review", "harness": harness.as_str(), "version": version.as_str()});
    match operator::call(home, &req) {
        Ok(a) if a["ok"] == true => Ok("review queued"),
        Ok(a) => Err(fail(a["error"].as_str().unwrap_or("the server refused the review"))),
        Err(CallError::NoServer(_)) => Ok("the review starts when `serve` starts"),
        Err(e) => Err(fail(e)),
    }
}

fn runtime() -> Result<tokio::runtime::Runtime, ExitCode> {
    tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(fail)
}

pub(crate) fn run(home: Option<PathBuf>, cmd: Command, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    match cmd {
        Command::List => list(&home, as_json),
        Command::Install { package } => install(&home, &package, as_json),
        Command::Show { harness, version } => show(&home, &harness_of(&harness)?, version.as_deref(), as_json),
        Command::Review { harness, version, retry } => {
            review(&home, &harness_of(&harness)?, &VersionId::from_run(&version), retry, as_json)
        }
        Command::Build { harness, version, retry } => {
            if !retry {
                eprintln!("nullrouter adapters build: pass --retry to requeue the build");
                return Err(ExitCode::from(2));
            }
            build(&home, &harness_of(&harness)?, &VersionId::from_run(&version), as_json)
        }
        Command::Approve { harness, version, note } => {
            let (h, v) = (harness_of(&harness)?, VersionId::from_run(&version));
            nullrouter_adapters::approve(home.path(), &h, &v, note.as_deref()).map_err(fail)?;
            decided(&home, &h, &v, "approved", as_json)
        }
        Command::Reject { harness, version, note } => {
            let (h, v) = (harness_of(&harness)?, VersionId::from_run(&version));
            nullrouter_adapters::reject(home.path(), &h, &v, note.as_deref()).map_err(fail)?;
            decided(&home, &h, &v, "rejected", as_json)
        }
        Command::Clear { harness, version, yes } => {
            clear(&home, &harness_of(&harness)?, &VersionId::from_run(&version), yes, as_json)
        }
        Command::ReviewSettings { model, budget, reserve_output, clear } => {
            review_settings(&home, model, budget, reserve_output, clear, as_json)
        }
        Command::Rebuild { harness } => {
            let only = harness.as_deref().map(harness_of).transpose()?;
            rebuild(&home, only.as_ref(), as_json)
        }
        Command::Remove { harness, version, force, yes } => {
            let version = version.as_deref().map(VersionId::from_run);
            remove(&home, &harness_of(&harness)?, version.as_ref(), force, yes, as_json)
        }
    }
}

/// One version as a text line, without its indent: id, semver, state, then any rebuild flags.
fn version_line(v: &VersionEntry) -> String {
    let flags: Vec<&str> = [(v.rebuilding, "rebuilding"), (v.rebuild_failed, "rebuild_failed")]
        .into_iter()
        .filter(|(on, _)| *on)
        .map(|(_, flag)| flag)
        .collect();
    let flags = if flags.is_empty() { String::new() } else { format!(" {}", flags.join(" ")) };
    format!("{}  {}  {}{flags}", v.id, v.semver, v.state)
}

fn version_json(v: &VersionEntry) -> Value {
    json!({"id": v.id, "semver": v.semver, "state": v.state,
           "rebuilding": v.rebuilding, "rebuild_failed": v.rebuild_failed})
}

/// Every harness: hermes as `built-in`, then each third-party harness in the index with its
/// active version, the state of that version, its unacknowledged alerts and its versions. Reads
/// only: with no `adapters/` directory there is nothing to list.
fn list(home: &OperatorHome, as_json: bool) -> Result<ExitCode, ExitCode> {
    let index = read_index(home)?;
    let alerts: Vec<Alert> = if home.path().join("adapters").is_dir() {
        AlertLog::open(&store_of(home)?).list().map_err(fail)?
    } else {
        Vec::new()
    };
    let unacked = |name: &HarnessName| alerts.iter().filter(|a| a.acked.is_none() && &a.harness == name).count();

    let mut rows: Vec<Value> = BUILTIN.iter().map(|h| json!({"harness": h, "built_in": true})).collect();
    let mut lines: Vec<String> = BUILTIN.iter().map(|h| format!("{h}  built-in")).collect();
    for h in index.harnesses.iter().filter(|h| !h.name.is_builtin()) {
        let n = unacked(&h.name);
        let active_entry = h.active.as_ref().and_then(|id| h.versions.iter().find(|v| &v.id == id));
        let active = h.active.as_ref().map_or_else(|| "-".to_owned(), ToString::to_string);
        let active_state = active_entry.map_or_else(|| "-".to_owned(), |v| v.state.to_string());
        lines.push(format!("{}  {active}  {active_state}  {n} alerts", h.name));
        for v in &h.versions {
            lines.push(format!("  {}", version_line(v)));
        }
        rows.push(json!({"harness": h.name, "built_in": false, "active": h.active, "alerts": n,
                         "versions": h.versions.iter().map(version_json).collect::<Vec<Value>>()}));
    }
    emit(as_json, &Value::Array(rows), &lines.join("\n"));
    Ok(ExitCode::SUCCESS)
}

fn print_installed(done: &Installed, hint: Option<&str>, as_json: bool) {
    let mut text = format!("{} {}: {}", done.harness, done.version, done.state);
    if !done.reason.is_empty() {
        text = format!("{text} ({})", done.reason);
    }
    if let Some(h) = hint {
        text = format!("{text}\n{h}");
    }
    let value = json!({"harness": done.harness, "version": done.version, "state": done.state,
                       "reason": done.reason, "note": hint});
    emit(as_json, &value, &text);
}

/// Prints where an install or build ended, and the follow-up each end calls for.
pub(crate) fn finish(home: &OperatorHome, done: &Installed, as_json: bool) -> Result<ExitCode, ExitCode> {
    match done.state {
        VersionState::Refused => {
            print_installed(done, None, as_json);
            if !as_json {
                eprintln!("refused by the gate or by verification");
            }
            Ok(ExitCode::from(REFUSED))
        }
        VersionState::InReview => {
            let note = start_review(home, &done.harness, &done.version)?;
            print_installed(done, Some(note), as_json);
            Ok(ExitCode::SUCCESS)
        }
        _ => {
            let hint = format!(
                "install the builder, then run `nullrouter adapters build {} {} --retry`",
                done.harness, done.version
            );
            print_installed(done, Some(&hint), as_json);
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn install(home: &OperatorHome, package: &Path, as_json: bool) -> Result<ExitCode, ExitCode> {
    let (engine, _) = Engine::open(home.clone()).map_err(|e| fail(format!("startup failed:\n{e}")))?;
    let engine = Arc::new(engine);
    let rt = runtime()?;
    let builder = engine.snapshot().settings().adapters.builder.clone();
    let result = rt.block_on(engine.install_adapter_only(package, builder.as_deref()));
    match result {
        Ok(done) => finish(home, &done, as_json),
        Err(InstallError::Refused(reasons)) => {
            let lines: Vec<String> = reasons.iter().map(ToString::to_string).collect();
            if as_json {
                println!("{:#}", json!({"state": "refused", "reasons": lines}));
            } else {
                for l in &lines {
                    eprintln!("{l}");
                }
                eprintln!("refused by the gate or by verification");
            }
            Ok(ExitCode::from(REFUSED))
        }
        Err(e) => Err(fail(e)),
    }
}

fn build(home: &OperatorHome, harness: &HarnessName, version: &VersionId, as_json: bool) -> Result<ExitCode, ExitCode> {
    let (engine, _) = Engine::open(home.clone()).map_err(|e| fail(format!("startup failed:\n{e}")))?;
    let engine = Arc::new(engine);
    let st = engine.snapshot();
    let styles: Vec<&str> = st.styles.keys().map(String::as_str).collect();
    let opts = InstallOptions {
        styles: &styles,
        builder: st.settings().adapters.builder.as_deref(),
        origin: nullrouter_adapters::store::Origin::Local(String::new()),
        build_timeout: BUILD_TIMEOUT,
    };
    let done = runtime()?.block_on(nullrouter_adapters::build(home.path(), harness, version, &opts)).map_err(fail)?;
    finish(home, &done, as_json)
}

fn review(
    home: &OperatorHome,
    harness: &HarnessName,
    version: &VersionId,
    retry: bool,
    as_json: bool,
) -> Result<ExitCode, ExitCode> {
    if retry {
        let store = store_of(home)?;
        let mut index = store.load_index().map_err(fail)?;
        index.transition(harness, version, VersionState::InReview, "").map_err(fail)?;
        store.save_index(&index).map_err(fail)?;
        let note = start_review(home, harness, version)?;
        emit(
            as_json,
            &json!({"harness": harness, "version": version, "state": "in_review", "note": note}),
            &format!("{harness} {version}: in_review\n{note}"),
        );
        return Ok(ExitCode::SUCCESS);
    }
    let index = read_index(home)?;
    let entry = index
        .version(harness, version)
        .ok_or_else(|| fail(format!("no version {version} of {harness} in the index")))?;
    let report = read_report(home, harness, entry);
    if as_json {
        println!("{:#}", json!({"state": entry.state, "reason": entry.state_reason, "report": report}));
    } else if let Some(r) = &report {
        print_report(r);
    } else {
        println!("{harness} {version}: {}{}", entry.state, reason_suffix(&entry.state_reason));
        println!("no review report");
    }
    Ok(ExitCode::SUCCESS)
}

fn reason_suffix(reason: &str) -> String {
    if reason.is_empty() { String::new() } else { format!(" ({reason})") }
}

fn read_file(home: &OperatorHome, harness: &HarnessName, entry: &VersionEntry, name: &str) -> Option<String> {
    let dir = home.path().join("adapters").join(harness.as_str()).join(entry.id.as_str());
    std::fs::read_to_string(dir.join(name)).ok()
}

fn read_report(home: &OperatorHome, harness: &HarnessName, entry: &VersionEntry) -> Option<ReviewReport> {
    serde_json::from_str(&read_file(home, harness, entry, "review.json")?).ok()
}

fn print_report(r: &ReviewReport) {
    println!(
        "risk: {}",
        serde_json::to_value(r.risk).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
    );
    println!("{}", r.summary);
    for f in &r.findings {
        println!("  {}: {}", f.location, f.concern);
    }
    println!("reviewed by {} ({} tokens in, {} out)", r.model, r.tokens_in, r.tokens_out);
}

fn show(
    home: &OperatorHome,
    harness: &HarnessName,
    version: Option<&str>,
    as_json: bool,
) -> Result<ExitCode, ExitCode> {
    let index = read_index(home)?;
    let h = index.harness(harness).ok_or_else(|| fail(format!("no harness {harness} in the index")))?;
    let entry = match version {
        Some(v) => h.versions.iter().find(|e| e.id.as_str() == v),
        None => h
            .active
            .as_ref()
            .and_then(|a| h.versions.iter().find(|e| &e.id == a))
            .or_else(|| h.versions.iter().max_by(|a, b| a.submitted.cmp(&b.submitted))),
    }
    .ok_or_else(|| fail(format!("no such version of {harness}")))?;
    let manifest = read_file(home, harness, entry, "source/adapter.toml").and_then(|t| Manifest::parse(&t).ok());
    let report = read_report(home, harness, entry);
    let decision: Option<Value> =
        read_file(home, harness, entry, "decision.json").and_then(|t| serde_json::from_str(&t).ok());
    if as_json {
        let m = manifest.as_ref().map(|m| {
            json!({"style": m.style, "kit": m.kit, "summary": m.summary,
                   "request_selectors": m.request.selectors, "response_selectors": m.response.selectors})
        });
        let out = json!({"harness": harness, "version": entry.id, "semver": entry.semver, "state": entry.state,
            "reason": entry.state_reason, "active": h.active.as_ref() == Some(&entry.id), "origin": entry.origin,
            "source_fp": entry.source_fp.as_str(), "wasm_hash": entry.wasm_hash, "manifest": m,
            "report": report, "decision": decision});
        println!("{out:#}");
        return Ok(ExitCode::SUCCESS);
    }
    println!("{harness} {}: {}{}", entry.id, entry.state, reason_suffix(&entry.state_reason));
    println!("origin: {:?}", entry.origin);
    println!("source_fp: {}", entry.source_fp.as_str());
    println!("wasm_hash: {}", entry.wasm_hash.as_deref().unwrap_or("-"));
    if let Some(m) = &manifest {
        println!("style: {}  kit: {}", m.style, m.kit);
        if !m.summary.is_empty() {
            println!("{}", m.summary);
        }
        println!("request selectors: {}", m.request.selectors.join(", "));
        println!("response selectors: {}", m.response.selectors.join(", "));
    }
    if let Some(r) = &report {
        print_report(r);
    }
    if let Some(d) = &decision {
        println!("decision: {d}");
    }
    Ok(ExitCode::SUCCESS)
}

/// One guardrail event as a line: when, the record it came from, what happened.
fn event_line(a: &Alert) -> String {
    let record = a.record.as_deref().unwrap_or("-");
    let times = if a.count > 1 { format!("  (x{})", a.count) } else { String::new() };
    format!("{}  {record}  {}{times}", a.at, a.detail)
}

/// `suspect` → `approved`: the version serves again. Lists its guardrail events first, then asks
/// unless `--yes`.
fn clear(
    home: &OperatorHome,
    harness: &HarnessName,
    version: &VersionId,
    yes: bool,
    as_json: bool,
) -> Result<ExitCode, ExitCode> {
    if as_json && !yes {
        eprintln!("--json needs --yes");
        return Err(ExitCode::from(2));
    }
    let index = read_index(home)?;
    let state = index
        .version(harness, version)
        .ok_or_else(|| fail(format!("no version {version} of {harness} in the index")))?
        .state;
    if state != VersionState::Suspect {
        return Err(fail(format!("{harness} {version} is {state}, not suspect")));
    }
    let store = store_of(home)?;
    let events: Vec<Alert> = AlertLog::open(&store)
        .list()
        .map_err(fail)?
        .into_iter()
        .filter(|a| a.kind == AlertKind::Guardrail && &a.harness == harness && &a.version == version)
        .collect();
    if !as_json {
        for a in &events {
            println!("{}", event_line(a));
        }
    }
    if !yes {
        eprint!("clear {harness} {version}: it serves again from the next request [y/N] ");
        io::stderr().flush().map_err(fail)?;
        let mut line = String::new();
        io::stdin().read_line(&mut line).map_err(fail)?;
        if !matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("not cleared");
            return Err(ExitCode::from(1));
        }
    }
    let mut index = store.load_index().map_err(fail)?;
    index.transition(harness, version, VersionState::Approved, "cleared by the operator").map_err(fail)?;
    store.save_index(&index).map_err(fail)?;
    let status = reload(home)?;
    emit(
        as_json,
        &json!({"harness": harness, "version": version, "state": "approved", "status": status, "events": events}),
        &format!("{harness} {version}: approved: {status}"),
    );
    Ok(ExitCode::SUCCESS)
}

fn rebuild(home: &OperatorHome, only: Option<&HarnessName>, as_json: bool) -> Result<ExitCode, ExitCode> {
    if let Some(h) = only
        && read_index(home)?.harness(h).is_none()
    {
        return Err(fail(format!("no harness {h} in the index")));
    }
    let (engine, _) = Engine::open(home.clone()).map_err(|e| fail(format!("startup failed:\n{e}")))?;
    let builder = engine.snapshot().settings().adapters.builder.clone();
    let opts = InstallOptions {
        styles: &[],
        builder: builder.as_deref(),
        origin: nullrouter_adapters::store::Origin::Local("kit-upgrade".into()),
        build_timeout: BUILD_TIMEOUT,
    };
    // A reload that cannot reach a server is fine: the next start reads the store.
    let reload_server = || {
        let _ = reload(home);
    };
    let done = runtime()?
        .block_on(nullrouter_adapters::rebuilds_for(home.path(), &opts, &reload_server, only))
        .map_err(fail)?;
    let index = read_index(home)?;
    let failed: Vec<Value> = index
        .harnesses
        .iter()
        .filter(|h| only.is_none_or(|o| &h.name == o))
        .flat_map(|h| h.versions.iter().filter(|v| v.rebuild_failed).map(move |v| json!([h.name, v.id])))
        .collect();
    let rebuilt: Vec<Value> = done.iter().map(|(h, v)| json!([h, v])).collect();
    if as_json {
        println!("{:#}", json!({"rebuilt": rebuilt, "failed": failed}));
    } else if rebuilt.is_empty() && failed.is_empty() {
        println!("nothing to rebuild");
    } else {
        for (h, v) in &done {
            println!("{h} {v}: rebuilt");
        }
        for f in &failed {
            println!("{} {}: rebuild failed", f[0].as_str().unwrap_or(""), f[1].as_str().unwrap_or(""));
        }
    }
    Ok(if failed.is_empty() { ExitCode::SUCCESS } else { ExitCode::from(1) })
}

/// Names of the keys bound to `harness`, revoked ones included (they keep the binding).
fn bound_keys(home: &OperatorHome, harness: &HarnessName) -> Result<Vec<String>, ExitCode> {
    let keys = Keys::load(&home.path().join(keys::FILE)).map_err(fail)?;
    Ok(keys.iter().filter(|k| k.adapter.as_ref() == Some(harness)).map(|k| k.name.clone()).collect())
}

/// Removes a version, or the whole harness. Lists the bound keys first, then asks unless `--yes`.
/// The active version needs `--force`.
fn remove(
    home: &OperatorHome,
    harness: &HarnessName,
    version: Option<&VersionId>,
    force: bool,
    yes: bool,
    as_json: bool,
) -> Result<ExitCode, ExitCode> {
    if as_json && !yes {
        eprintln!("--json needs --yes");
        return Err(ExitCode::from(2));
    }
    let index = read_index(home)?;
    let entry = index.harness(harness).ok_or_else(|| fail(format!("no harness {harness} in the index")))?;
    if let Some(v) = version {
        if index.version(harness, v).is_none() {
            return Err(fail(format!("no version {v} of {harness} in the index")));
        }
        if entry.active.as_ref() == Some(v) && !force {
            return Err(fail(format!("{harness} {v} is the active version; pass --force to remove it")));
        }
    }
    // Removing the last version leaves the keys with nothing to run, as removing the harness does.
    let whole = version.is_none() || entry.versions.len() == 1;
    let bound = if whole { bound_keys(home, harness)? } else { Vec::new() };
    if !as_json {
        for k in &bound {
            println!("key {k} becomes a plain client");
        }
    }
    if !yes {
        let what = version.map_or_else(|| format!("{harness} and all its versions"), |v| format!("{harness} {v}"));
        eprint!("remove {what} [y/N] ");
        io::stderr().flush().map_err(fail)?;
        let mut line = String::new();
        io::stdin().read_line(&mut line).map_err(fail)?;
        if !matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("not removed");
            return Err(ExitCode::from(1));
        }
    }
    let done = nullrouter_adapters::remove(home.path(), harness, version, force).map_err(fail)?;
    let status = reload(home)?;
    let removed: Vec<&str> = done.versions.iter().map(VersionId::as_str).collect();
    emit(
        as_json,
        &json!({"harness": harness, "removed": removed, "keys": bound, "status": status}),
        &format!("{harness}: removed {} version(s): {status}", removed.len()),
    );
    Ok(ExitCode::SUCCESS)
}

fn decided(
    home: &OperatorHome,
    harness: &HarnessName,
    version: &VersionId,
    state: &str,
    as_json: bool,
) -> Result<ExitCode, ExitCode> {
    let status = reload(home)?;
    emit(
        as_json,
        &json!({"harness": harness, "version": version, "state": state, "status": status}),
        &format!("{harness} {version}: {state}: {status}"),
    );
    Ok(ExitCode::SUCCESS)
}

fn review_settings(
    home: &OperatorHome,
    model: Option<String>,
    budget: Option<u64>,
    reserve_output: Option<u32>,
    clear: bool,
    as_json: bool,
) -> Result<ExitCode, ExitCode> {
    let config = if clear {
        None
    } else {
        let (Some(model), Some(budget_tokens)) = (model, budget) else {
            return Err(fail("review-settings needs --model and --budget, or --clear"));
        };
        if model.trim().is_empty() || budget_tokens == 0 {
            return Err(fail("--model must be named and --budget must be above 0"));
        }
        let mut c: ReviewConfig =
            serde_json::from_value(json!({"model": model, "budget_tokens": budget_tokens})).map_err(fail)?;
        if let Some(r) = reserve_output {
            c.reserve_output = r;
        }
        Some(c)
    };
    let store = store_of(home)?;
    let mut index = store.load_index().map_err(fail)?;
    index.review.clone_from(&config);
    store.save_index(&index).map_err(fail)?;
    let status = reload(home)?;
    let text = match &config {
        Some(c) => format!("review: {} budget {} reserve {}: {status}", c.model, c.budget_tokens, c.reserve_output),
        None => format!("review settings cleared: {status}"),
    };
    emit(as_json, &json!({"review": config, "status": status}), &text);
    Ok(ExitCode::SUCCESS)
}
