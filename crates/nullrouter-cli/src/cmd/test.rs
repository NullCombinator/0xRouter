//! `nullrouter test` (spec 011 contracts/cli.md § `nullrouter test`): real, billed calls through
//! the running server, one per pair, each giving PASS, BROKEN or UNKNOWN.
//!
//! `test.plan` names the pairs and the calls first; more than one call asks before it runs.
//! `test.run` then streams one result per pair as it finishes. Ctrl-C closes the socket: the
//! server sends no more calls, and the finished results stay saved.
//!
//! A combo's test is one call through the combo; its answer is one nested result, printed as a
//! tree of the members the walk reached.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::SystemTime;

use clap::Args as ClapArgs;
use nullrouter_registry::OperatorHome;
use nullrouter_server::ops::model_tests::{self, Event};
use nullrouter_server::ops::{Ctx, OpError};
use serde_json::json;

#[derive(ClapArgs)]
pub(crate) struct Args {
    /// `provider/model`, a unified model, or a combo.
    #[arg(required_unless_present = "all", conflicts_with = "all")]
    target: Option<String>,
    /// Only this account of the model's provider.
    #[arg(long, value_name = "NAME")]
    account: Option<String>,
    /// Every pair a unified model or combo can reach.
    #[arg(long)]
    all: bool,
    /// Run without asking first.
    #[arg(long)]
    yes: bool,
}

fn fail(e: impl std::fmt::Display) -> ExitCode {
    eprintln!("{e}");
    ExitCode::from(1)
}

fn op_fail(e: OpError) -> ExitCode {
    eprintln!("{}", e.text);
    ExitCode::from(e.exit)
}

pub(crate) fn run(home: Option<PathBuf>, args: Args, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    let ctx = Ctx { home, engine: None };
    let (target, account) = (args.target.as_deref(), args.account.as_deref());
    let plan = model_tests::plan(&ctx, target, account).map_err(op_fail)?;
    if plan.needs_confirm() && !args.yes && !confirm(&plan.question()) {
        return Err(fail("not run"));
    }

    let mut results = Vec::new();
    let mut combo = None;
    let done = model_tests::run_with(&ctx, target, account, |event| match event {
        Event::Combo(r) => combo = Some(r.clone()),
        Event::Result(r) => {
            if !as_json {
                println!("{}", model_tests::result_line(r, SystemTime::now()));
            }
            results.push(r.clone());
        }
    })
    .map_err(op_fail)?;
    if let Some(c) = combo {
        if as_json {
            println!("{}", json!({"combo": c, "done": done}));
        } else {
            for l in model_tests::combo_lines(&c, SystemTime::now()) {
                println!("{l}");
            }
        }
    } else if as_json {
        println!("{}", json!({"results": results, "done": done}));
    } else {
        println!("{}", model_tests::summary_line(results.len(), &done));
    }
    Ok(ExitCode::SUCCESS)
}

/// `12 billed calls: 8 text, 2 embeddings, … Continue? [y/N]`, on stderr so `--json` stays clean.
fn confirm(question: &str) -> bool {
    eprint!("{question} [y/N] ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}
