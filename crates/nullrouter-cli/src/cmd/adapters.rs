//! `nullrouter adapters` (contracts/operator-cli.md): the third-party adapter store.
//!
//! Reviews belong to the server: the CLI never runs one, because its process would drop the queue
//! at exit. After a step that leaves a version `in_review` it asks the running server to review
//! it; with no server the review starts when `serve` starts (`Engine::resume_reviews`).

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;
use nullrouter_adapters::HarnessName;
use nullrouter_adapters::loader::Manifest;
use nullrouter_adapters::review::ReviewReport;
use nullrouter_adapters::store::{Index, Store, VersionEntry, VersionId};
use nullrouter_registry::OperatorHome;
use nullrouter_server::ops::accounts::Report;
use nullrouter_server::ops::adapters::{self as ops, Failure};
use nullrouter_server::ops::{Ctx, OpError, Status};
use nullrouter_server::views;
use serde_json::{Value, json};

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

/// Prints what an op returned, as the other commands do: the log on stderr, then the text, or
/// the JSON with `--json`. A change the running server did not take is an error.
pub(crate) fn print_report(log: &[String], r: Result<Report, OpError>, as_json: bool) -> Result<ExitCode, ExitCode> {
    for l in log {
        eprintln!("{l}");
    }
    let r = r.map_err(|e| fail_with(&e.text, e.exit))?;
    printed(&r, as_json)
}

fn printed(r: &Report, as_json: bool) -> Result<ExitCode, ExitCode> {
    if let Status::SavedNotApplied { reason } = &r.done.status {
        return Err(fail_with(reason, 1));
    }
    if as_json {
        println!("{:#}", r.json);
    } else {
        println!("{}", r.done.text);
    }
    Ok(ExitCode::SUCCESS)
}

/// [`print_report`] for the steps that can end in a refusal, which have their own shapes.
pub(crate) fn print_ended(
    log: &[String],
    r: Result<Report, Box<Failure>>,
    as_json: bool,
) -> Result<ExitCode, ExitCode> {
    for l in log {
        eprintln!("{l}");
    }
    match r {
        Ok(r) => printed(&r, as_json),
        Err(f) => {
            match (&f.json, &f.out) {
                (Some(j), _) if as_json => println!("{j:#}"),
                (_, Some(out)) => {
                    println!("{out}");
                    if let Some(t) = f.tail {
                        eprintln!("{t}");
                    }
                }
                _ => eprintln!("{}", f.error.text),
            }
            Err(ExitCode::from(f.error.exit))
        }
    }
}

fn fail_with(text: &str, code: u8) -> ExitCode {
    eprintln!("{text}");
    ExitCode::from(code)
}

/// Asks `question` on stderr; true on `y` or `yes`.
fn confirmed(question: &str) -> Result<bool, ExitCode> {
    eprint!("{question} [y/N] ");
    io::stderr().flush().map_err(fail)?;
    let mut line = String::new();
    io::stdin().read_line(&mut line).map_err(fail)?;
    Ok(matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

pub(crate) fn run(home: Option<PathBuf>, cmd: Command, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    let ctx = Ctx { home: home.clone(), engine: None };
    let mut log = Vec::new();
    match cmd {
        Command::List => list(&home, as_json),
        Command::Install { package } => {
            let r = ops::install_with(&ctx, &mut log, &package);
            print_ended(&log, r, as_json)
        }
        Command::Show { harness, version } => show(&home, &harness_of(&harness)?, version.as_deref(), as_json),
        Command::Review { harness, version, retry } => {
            if !retry {
                return review(&home, &harness_of(&harness)?, &VersionId::from_run(&version), as_json);
            }
            let r = ops::review_with(&ctx, &mut log, &harness, &version);
            print_ended(&log, r, as_json)
        }
        Command::Build { harness, version, retry } => {
            if !retry {
                eprintln!("nullrouter adapters build: pass --retry to requeue the build");
                return Err(ExitCode::from(2));
            }
            let r = ops::build_with(&ctx, &mut log, &harness, &version);
            print_ended(&log, r, as_json)
        }
        Command::Approve { harness, version, note } => {
            let r = ops::approve_with(&ctx, &mut log, &harness, &version, note.as_deref());
            print_report(&log, r, as_json)
        }
        Command::Reject { harness, version, note } => {
            let r = ops::reject_with(&ctx, &mut log, &harness, &version, note.as_deref());
            print_report(&log, r, as_json)
        }
        Command::Clear { harness, version, yes } => clear(&ctx, &harness, &version, yes, as_json),
        Command::ReviewSettings { model, budget, reserve_output, clear } => {
            let r = ops::review_settings_with(&ctx, &mut log, model.as_deref(), budget, reserve_output, clear);
            print_report(&log, r, as_json)
        }
        Command::Rebuild { harness } => {
            let r = ops::rebuild_with(&ctx, &mut log, harness.as_deref());
            print_ended(&log, r, as_json)
        }
        Command::Remove { harness, version, force, yes } => {
            remove(&ctx, &harness, version.as_deref(), force, yes, as_json)
        }
    }
}

/// One version as a text line, without its indent: id, semver, state, then any rebuild flags.
fn version_line(v: &Value) -> String {
    let flags: Vec<&str> = [("rebuilding", "rebuilding"), ("rebuild_failed", "rebuild_failed")]
        .into_iter()
        .filter(|(key, _)| v[*key] == true)
        .map(|(_, flag)| flag)
        .collect();
    let flags = if flags.is_empty() { String::new() } else { format!(" {}", flags.join(" ")) };
    let text = |k: &str| v[k].as_str().unwrap_or_default().to_owned();
    format!("{}  {}  {}{flags}", text("id"), text("semver"), text("state"))
}

/// Every harness: hermes as `built-in`, then each third-party harness in the index with its
/// active version, the state of that version, its unacknowledged alerts and its versions. The
/// read is the `adapters` view, which the dashboard reads too.
fn list(home: &OperatorHome, as_json: bool) -> Result<ExitCode, ExitCode> {
    let view = super::read(home, views::adapters::NEEDS, &json!({}), views::adapters::build)?;
    let mut lines: Vec<String> = Vec::new();
    for h in view.json.as_array().into_iter().flatten() {
        let name = h["harness"].as_str().unwrap_or_default();
        if h["built_in"] == true {
            lines.push(format!("{name}  built-in"));
            continue;
        }
        let versions = h["versions"].as_array().map_or(&[][..], Vec::as_slice);
        let active = h["active"].as_str();
        let active_state = active
            .and_then(|id| versions.iter().find(|v| v["id"] == id))
            .and_then(|v| v["state"].as_str())
            .unwrap_or("-");
        lines.push(format!("{name}  {}  {active_state}  {} alerts", active.unwrap_or("-"), h["alerts"]));
        lines.extend(versions.iter().map(|v| format!("  {}", version_line(v))));
    }
    emit(as_json, &view.json, &lines.join("\n"));
    Ok(ExitCode::SUCCESS)
}

/// `review` without `--retry`: the report of a version, read from the store.
fn review(
    home: &OperatorHome,
    harness: &HarnessName,
    version: &VersionId,
    as_json: bool,
) -> Result<ExitCode, ExitCode> {
    let index = read_index(home)?;
    let entry = index
        .version(harness, version)
        .ok_or_else(|| fail(format!("no version {version} of {harness} in the index")))?;
    let report = read_report(home, harness, entry);
    if as_json {
        println!("{:#}", json!({"state": entry.state, "reason": entry.state_reason, "report": report}));
    } else if let Some(r) = &report {
        print_report_text(r);
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

fn print_report_text(r: &ReviewReport) {
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
        print_report_text(r);
    }
    if let Some(d) = &decision {
        println!("decision: {d}");
    }
    Ok(ExitCode::SUCCESS)
}

/// `suspect` to `approved`: the version serves again. Lists its guardrail events first, then
/// asks unless `--yes`.
fn clear(ctx: &Ctx, harness: &str, version: &str, yes: bool, as_json: bool) -> Result<ExitCode, ExitCode> {
    if as_json && !yes {
        eprintln!("--json needs --yes");
        return Err(ExitCode::from(2));
    }
    let events = ops::clear_events(ctx, harness, version).map_err(|e| fail_with(&e.text, e.exit))?;
    if !as_json {
        for a in &events {
            println!("{}", ops::event_line(a));
        }
    }
    if !yes && !confirmed(&ops::clear_question(harness, version))? {
        println!("not cleared");
        return Err(ExitCode::from(1));
    }
    let mut log = Vec::new();
    let r = ops::clear_with(ctx, &mut log, harness, version);
    print_report(&log, r, as_json)
}

/// Removes a version, or the whole harness. Lists the bound keys first, then asks unless `--yes`.
/// The active version needs `--force`.
fn remove(
    ctx: &Ctx,
    harness: &str,
    version: Option<&str>,
    force: bool,
    yes: bool,
    as_json: bool,
) -> Result<ExitCode, ExitCode> {
    if as_json && !yes {
        eprintln!("--json needs --yes");
        return Err(ExitCode::from(2));
    }
    let preview = ops::remove_preview(ctx, harness, version, force).map_err(|e| fail_with(&e.text, e.exit))?;
    if !as_json {
        for k in &preview.bound {
            println!("key {k} becomes a plain client");
        }
    }
    if !yes && !confirmed(&ops::remove_question(harness, version))? {
        println!("not removed");
        return Err(ExitCode::from(1));
    }
    let mut log = Vec::new();
    let r = ops::remove_with(ctx, &mut log, harness, version, force);
    print_report(&log, r, as_json)
}
