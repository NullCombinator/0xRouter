//! `nullrouter alerts` (contracts/operator-cli.md): the adapter alerts in `adapters/alerts.toml`.
//!
//! The CLI reads and acknowledges the file directly, as `adapters` does, and never calls the
//! operator socket. A read creates nothing: with no `adapters/` directory there are no alerts.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;
use jiff::Timestamp;
use nullrouter_adapters::alerts::{Alert, AlertId, AlertLog};
use nullrouter_adapters::store::Store;
use nullrouter_registry::OperatorHome;
use serde_json::{Value, json};

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Unacknowledged alerts, newest first. `--all` includes acknowledged ones.
    List {
        #[arg(long)]
        all: bool,
    },
    /// Acknowledge one alert by id, or every open one with `--all`. Give exactly one.
    Ack {
        /// An alert id: `al_` and 10 lowercase letters or digits.
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        id: Option<String>,
        #[arg(long)]
        all: bool,
    },
}

fn fail(e: impl std::fmt::Display) -> ExitCode {
    eprintln!("{e}");
    ExitCode::from(1)
}

fn emit(as_json: bool, value: &Value, text: &str) {
    if as_json {
        println!("{value:#}");
    } else {
        println!("{text}");
    }
}

/// The store, or `None` while there is no `adapters/` (a read creates nothing).
fn existing_store(home: &OperatorHome) -> Result<Option<Store>, ExitCode> {
    if !home.path().join("adapters").is_dir() {
        return Ok(None);
    }
    Store::open(home.path()).map(Some).map_err(fail)
}

pub(crate) fn run(home: Option<PathBuf>, cmd: Command, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    match cmd {
        Command::List { all } => list(&home, all, as_json),
        Command::Ack { id, all } => ack(&home, id.as_deref(), all, as_json),
    }
}

/// One alert as a line: id, kind, harness@version, record, detail, time, then `(xN)` if folded.
fn alert_line(a: &Alert) -> String {
    let record = a.record.as_deref().unwrap_or("-");
    let times = if a.count > 1 { format!("  (x{})", a.count) } else { String::new() };
    format!("{}  {}  {}@{}  {record}  {}  {}{times}", a.id, a.kind, a.harness, a.version, a.detail, a.at)
}

fn list(home: &OperatorHome, all: bool, as_json: bool) -> Result<ExitCode, ExitCode> {
    let stored = match existing_store(home)? {
        Some(store) => AlertLog::open(&store).list().map_err(fail)?,
        None => Vec::new(),
    };
    // The log is kept oldest first; the listing is newest first.
    let shown: Vec<&Alert> = stored.iter().rev().filter(|a| all || a.acked.is_none()).collect();
    let value = serde_json::to_value(&shown).map_err(fail)?;
    let text: Vec<String> = shown.iter().copied().map(alert_line).collect();
    emit(as_json, &value, &text.join("\n"));
    Ok(ExitCode::SUCCESS)
}

fn ack(home: &OperatorHome, id: Option<&str>, all: bool, as_json: bool) -> Result<ExitCode, ExitCode> {
    let store = existing_store(home)?;
    if all {
        let n = match &store {
            Some(s) => AlertLog::open(s).ack_all(Timestamp::now()).map_err(fail)?,
            None => 0,
        };
        emit(as_json, &json!({"acknowledged": n}), &format!("acknowledged {n}"));
        return Ok(ExitCode::SUCCESS);
    }
    let Some(raw) = id else {
        eprintln!("nullrouter alerts ack: give an alert id, or --all");
        return Err(ExitCode::from(2));
    };
    let unknown = || fail(format!("no open alert {raw}"));
    let Some(id) = AlertId::parse(raw) else { return Err(unknown()) };
    let Some(store) = store else { return Err(unknown()) };
    if !AlertLog::open(&store).ack(&id, Timestamp::now()).map_err(fail)? {
        return Err(unknown());
    }
    emit(as_json, &json!({"acknowledged": 1, "id": id}), &format!("acknowledged {id}"));
    Ok(ExitCode::SUCCESS)
}
