//! `nullrouter behaviour` (contracts/operator-cli.md).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;
use nullrouter_registry::OperatorHome;
use nullrouter_server::ops::{self, Ctx, Status};
use nullrouter_server::views;
use serde_json::json;

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Show the operator's request-handling defaults and whether each is the default.
    Show,
    /// Operator default for a stream that breaks after output: `restart` or `error_event`.
    SetBreak { behaviour: String },
}

fn fail(e: impl std::fmt::Display) -> ExitCode {
    eprintln!("{e}");
    ExitCode::from(1)
}

pub(crate) fn run(home: Option<PathBuf>, cmd: Command, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    let behaviour = match cmd {
        Command::Show => return show(&home, as_json),
        Command::SetBreak { behaviour } => behaviour,
    };
    let ctx = Ctx { home, engine: None };
    let mut log = Vec::new();
    let result = ops::routing::set_break_default_with(&ctx, &mut log, &behaviour);
    for l in &log {
        eprintln!("{l}");
    }
    let r = result.map_err(|e| {
        eprintln!("{}", e.text);
        ExitCode::from(e.exit)
    })?;
    if let Status::SavedNotApplied { reason } = &r.done.status {
        return Err(fail(reason));
    }
    println!("{}", r.done.text);
    Ok(ExitCode::SUCCESS)
}

fn show(home: &OperatorHome, as_json: bool) -> Result<ExitCode, ExitCode> {
    let view = super::read(home, views::behaviour::NEEDS, &json!({}), views::behaviour::build)?;
    if as_json {
        println!("{:#}", view.json);
        return Ok(ExitCode::SUCCESS);
    }
    let b = &view.json["break_behaviour"];
    let tag = if b["default"] == true { "  (default)" } else { "" };
    println!("break_behaviour  {}{tag}", b["value"].as_str().unwrap_or_default());
    Ok(ExitCode::SUCCESS)
}
