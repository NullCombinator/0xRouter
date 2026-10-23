//! `nullrouter alerts` (contracts/operator-cli.md): the adapter alerts in `adapters/alerts.toml`.
//!
//! The CLI reads and acknowledges the file directly, as `adapters` does, and never calls the
//! operator socket. A read creates nothing: with no `adapters/` directory there are no alerts.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;

use super::adapters::print_report;
use nullrouter_adapters::alerts::Alert;
use nullrouter_registry::OperatorHome;
use nullrouter_server::ops::Ctx;
use nullrouter_server::ops::adapters as ops;
use nullrouter_server::views;
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
    let view = super::read(home, views::alerts::NEEDS, &json!({"all": all}), views::alerts::build)?;
    let shown: Vec<Alert> = serde_json::from_value(view.json.clone()).map_err(fail)?;
    let text: Vec<String> = shown.iter().map(alert_line).collect();
    emit(as_json, &view.json, &text.join("\n"));
    Ok(ExitCode::SUCCESS)
}

fn ack(home: &OperatorHome, id: Option<&str>, all: bool, as_json: bool) -> Result<ExitCode, ExitCode> {
    let ctx = Ctx { home: home.clone(), engine: None };
    let mut log = Vec::new();
    let r = ops::alerts_ack_with(&ctx, &mut log, id, all);
    print_report(&log, r, as_json)
}
