//! `nullrouter catalogue` (contracts/operator-cli.md): the adapter catalogue. Only this module
//! names `catalogue::`; `serve` never reaches it. Installs go through the same `finish` as
//! `adapters install`, so a refusal exits 3 and an `in_review` version starts its review.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;
use nullrouter_adapters::catalogue::{self, CatalogueError, Client};
use nullrouter_adapters::store::{Index, Origin, Store};
use nullrouter_adapters::{BUILD_TIMEOUT, InstallError, InstallOptions};
use nullrouter_engine::state::Engine;
use nullrouter_registry::OperatorHome;
use serde_json::{Value, json};

use super::adapters::finish;

const REFUSED: u8 = 3;

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Each harness in the catalogue, its newest version, and whether it is installed.
    List,
    /// Every version of a harness in the catalogue, with source URL and fingerprints.
    Show { harness: String },
    /// Fetch, verify, gate and queue a version (the newest unless one is named).
    Install {
        harness: String,
        /// A semver version. The newest one if omitted.
        semver: Option<String>,
    },
    /// Newer catalogue versions of installed harnesses. Installs nothing.
    Check,
}

fn fail(e: impl std::fmt::Display) -> ExitCode {
    eprintln!("{e}");
    ExitCode::from(1)
}

fn runtime() -> Result<tokio::runtime::Runtime, ExitCode> {
    tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(fail)
}

/// The catalogue URL from the settings. Opening the engine is the lightest way to read them.
fn catalogue_url(home: &OperatorHome) -> Result<String, ExitCode> {
    let (engine, _) = Engine::open(home.clone()).map_err(|e| fail(format!("startup failed:\n{e}")))?;
    let url = engine.snapshot().settings().adapters.catalogue_url.clone();
    Ok(url)
}

/// The index, empty while there is no `adapters/` (a read creates nothing).
fn read_index(home: &OperatorHome) -> Result<Index, ExitCode> {
    if !home.path().join("adapters").is_dir() {
        return Ok(Index::default());
    }
    Store::open(home.path()).map_err(fail)?.load_index().map_err(fail)
}

fn is_installed(index: &Index, harness: &str) -> bool {
    index.harnesses.iter().any(|h| h.name.as_str() == harness && !h.versions.is_empty())
}

/// Prints a catalogue error with its code and returns the exit code it maps to. Refusals also
/// print the gate lines.
fn report(e: &CatalogueError, as_json: bool) -> ExitCode {
    let code = e.code();
    let reasons: Option<Vec<String>> = match e {
        CatalogueError::Install(InstallError::Refused(r)) => Some(r.iter().map(ToString::to_string).collect()),
        _ => None,
    };
    if as_json {
        println!("{:#}", json!({"error": code, "message": e.to_string(), "reasons": reasons}));
    } else {
        eprintln!("error[{code}]: {e}");
        for line in reasons.iter().flatten() {
            eprintln!("{line}");
        }
        if reasons.is_some() {
            eprintln!("refused by the gate or by verification");
        }
    }
    ExitCode::from(exit_of(e))
}

fn exit_of(e: &CatalogueError) -> u8 {
    match e {
        CatalogueError::Fetch { .. } | CatalogueError::NotHttps(_) | CatalogueError::TooLarge { .. } => 5,
        CatalogueError::HashMismatch { .. }
        | CatalogueError::FpMismatch { .. }
        | CatalogueError::Unpack(_)
        | CatalogueError::Install(InstallError::Refused(_)) => REFUSED,
        _ => 1,
    }
}

/// The versions of `entry` newer than every installed one: the intersection of each installed
/// version's `newer_than`. The highest installed version has the smallest such set, so the
/// intersection is exactly its set.
fn newer_than_all<'a>(entry: &'a catalogue::Entry, installed: &[&str]) -> Vec<&'a catalogue::Version> {
    let Some((first, rest)) = installed.split_first() else { return Vec::new() };
    let mut out = entry.newer_than(*first);
    for s in rest {
        let other = entry.newer_than(*s);
        out.retain(|v| other.iter().any(|o| o.semver == v.semver));
    }
    out
}

pub(crate) fn run(home: Option<PathBuf>, cmd: Command, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    match cmd {
        Command::List => list(&home, as_json),
        Command::Show { harness } => show(&home, &harness, as_json),
        Command::Install { harness, semver } => install(&home, &harness, semver.as_deref(), as_json),
        Command::Check => check(&home, as_json),
    }
}

fn list(home: &OperatorHome, as_json: bool) -> Result<ExitCode, ExitCode> {
    let url = catalogue_url(home)?;
    let client = Client::new().map_err(|e| report(&e, as_json))?;
    let cat = runtime()?.block_on(client.fetch_index(&url)).map_err(|e| report(&e, as_json))?;
    let installed = read_index(home)?;
    let mut rows = Vec::new();
    for e in &cat.entry {
        let newest = e.pick(None).map(|v| v.semver.clone());
        let inst = is_installed(&installed, &e.harness);
        if as_json {
            rows.push(json!({"harness": e.harness, "summary": e.summary, "newest": newest, "installed": inst}));
        } else {
            let yes = if inst { "yes" } else { "no" };
            println!("{}  {}  installed: {yes}  {}", e.harness, newest.as_deref().unwrap_or("-"), e.summary);
        }
    }
    if as_json {
        println!("{}", Value::Array(rows));
    }
    Ok(ExitCode::SUCCESS)
}

fn show(home: &OperatorHome, harness: &str, as_json: bool) -> Result<ExitCode, ExitCode> {
    let url = catalogue_url(home)?;
    let client = Client::new().map_err(|e| report(&e, as_json))?;
    let cat = runtime()?.block_on(client.fetch_index(&url)).map_err(|e| report(&e, as_json))?;
    let entry =
        cat.entry(harness).ok_or_else(|| report(&CatalogueError::UnknownHarness(harness.to_owned()), as_json))?;
    if as_json {
        let versions: Vec<Value> = entry
            .version
            .iter()
            .map(|v| json!({"semver": v.semver, "source": v.source, "sha256": v.sha256, "source_fp": v.source_fp}))
            .collect();
        println!(
            "{:#}",
            json!({"harness": entry.harness, "summary": entry.summary, "homepage": entry.homepage,
                   "style": entry.style, "versions": versions})
        );
    } else {
        println!("{}: {}", entry.harness, entry.summary);
        if let Some(h) = &entry.homepage {
            println!("homepage: {h}");
        }
        for v in &entry.version {
            println!("{}\n  source {}\n  sha256 {}\n  source_fp {}", v.semver, v.source, v.sha256, v.source_fp);
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn install(home: &OperatorHome, harness: &str, semver: Option<&str>, as_json: bool) -> Result<ExitCode, ExitCode> {
    let (engine, _) = Engine::open(home.clone()).map_err(|e| fail(format!("startup failed:\n{e}")))?;
    let st = engine.snapshot();
    let styles: Vec<&str> = st.styles.keys().map(String::as_str).collect();
    let url = st.settings().adapters.catalogue_url.clone();
    let opts = InstallOptions {
        styles: &styles,
        builder: st.settings().adapters.builder.as_deref(),
        origin: Origin::Local(String::new()),
        build_timeout: BUILD_TIMEOUT,
    };
    let client = Client::new().map_err(|e| report(&e, as_json))?;
    let result = runtime()?.block_on(client.install(home.path(), &url, harness, semver, opts));
    match result {
        Ok(done) => finish(home, &done, as_json),
        Err(e) => Err(report(&e, as_json)),
    }
}

fn check(home: &OperatorHome, as_json: bool) -> Result<ExitCode, ExitCode> {
    let url = catalogue_url(home)?;
    let client = Client::new().map_err(|e| report(&e, as_json))?;
    let cat = runtime()?.block_on(client.fetch_index(&url)).map_err(|e| report(&e, as_json))?;
    let installed = read_index(home)?;
    let mut rows: Vec<(String, Vec<String>)> = Vec::new();
    for h in &installed.harnesses {
        if h.versions.is_empty() {
            continue;
        }
        let versions: Vec<&str> = h.versions.iter().map(|v| v.semver.as_str()).collect();
        let newer: Vec<String> = match cat.entry(h.name.as_str()) {
            Some(entry) => newer_than_all(entry, &versions).iter().map(|v| v.semver.clone()).collect(),
            None => Vec::new(),
        };
        rows.push((h.name.as_str().to_owned(), newer));
    }
    if as_json {
        let out: Vec<Value> = rows.iter().map(|(h, n)| json!({"harness": h, "newer": n})).collect();
        println!("{}", Value::Array(out));
    } else {
        for (h, newer) in &rows {
            if newer.is_empty() {
                println!("{h}: no newer version");
            } else {
                println!("{h}: newer {}", newer.join(", "));
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
