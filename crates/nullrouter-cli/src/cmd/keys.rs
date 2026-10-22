//! `nullrouter keys` (contracts/operator-cli.md).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;
use nullrouter_registry::OperatorHome;
use nullrouter_server::ops::accounts::Report;
use nullrouter_server::ops::{self, Ctx, OpError, Status};
use nullrouter_server::views;
use serde_json::{Value, json};

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Issue a key. It is printed once; only its digest is stored.
    Issue {
        name: String,
        #[arg(long = "break", value_name = "BEHAVIOUR")]
        break_behaviour: Option<String>,
        /// Which harness the key's client is (1 to 32 characters); shown, never acted on.
        #[arg(long)]
        harness: Option<String>,
        /// The harness adapter this key's requests run through.
        #[arg(long, value_name = "HARNESS")]
        adapter: Option<String>,
    },
    List,
    /// Revoke a key by name or id.
    Revoke {
        key: String,
    },
    /// Set, replace or remove (`--clear`) the harness tag of a key, by name or id.
    Tag {
        key: String,
        text: Option<String>,
        #[arg(long)]
        clear: bool,
    },
    /// Per-key break behaviour: `restart`, `error_event`, or `default`.
    SetBreak {
        key: String,
        behaviour: String,
    },
    /// Bind a key to a harness's adapter, or unbind it with `--clear`.
    SetAdapter {
        key: String,
        #[arg(required_unless_present = "clear", conflicts_with = "clear")]
        harness: Option<String>,
        #[arg(long)]
        clear: bool,
    },
}

fn fail(e: impl std::fmt::Display) -> ExitCode {
    eprintln!("{e}");
    ExitCode::from(1)
}

fn report(r: Result<Report, OpError>, log: &[String], as_json: bool, issued: bool) -> Result<ExitCode, ExitCode> {
    for l in log {
        eprintln!("{l}");
    }
    let r = r.map_err(|e| {
        eprintln!("{}", e.text);
        ExitCode::from(e.exit)
    })?;
    if let Status::SavedNotApplied { reason } = &r.done.status {
        return Err(fail(reason));
    }
    if as_json {
        println!("{}", r.json);
    } else if let (true, Some(once)) = (issued, &r.done.once) {
        // The only time the key is shown: stdout carries just the key, for scripts.
        eprintln!("{}", r.done.text);
        once.value.with_exposed(|k| println!("{k}"));
    } else {
        println!("{}", r.done.text);
    }
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn run(home: Option<PathBuf>, cmd: Command, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    if matches!(cmd, Command::List) {
        let view = super::read(&home, views::keys::NEEDS, &json!({}), views::keys::build)?;
        print_list(&view.json, as_json);
        return Ok(ExitCode::SUCCESS);
    }
    let ctx = Ctx { home, engine: None };
    let mut log = Vec::new();
    let log = &mut log;
    let mut issued = false;
    let result = match cmd {
        Command::List => unreachable!("handled above"),
        Command::Issue { name, break_behaviour, harness, adapter } => {
            issued = true;
            ops::keys::issue_with(&ctx, log, &name, break_behaviour.as_deref(), harness.as_deref(), adapter.as_deref())
        }
        Command::Revoke { key } => ops::keys::revoke_with(&ctx, log, &key),
        Command::Tag { key, text, clear } => ops::keys::tag_with(&ctx, log, &key, text.as_deref(), clear),
        Command::SetBreak { key, behaviour } => ops::keys::set_break_with(&ctx, log, &key, &behaviour),
        Command::SetAdapter { key, harness, clear } => {
            ops::keys::set_adapter_with(&ctx, log, &key, harness.as_deref(), clear)
        }
    };
    report(result, log, as_json, issued)
}

fn print_list(rows: &Value, as_json: bool) {
    if as_json {
        println!("{rows:#}");
        return;
    }
    // The adapter column appears once any key is bound to one.
    let any_adapter = rows.as_array().into_iter().flatten().any(|r| r["adapter"].is_string());
    for r in rows.as_array().into_iter().flatten() {
        let s = |k: &str| r[k].as_str().unwrap_or_default().to_owned();
        let adapter =
            if any_adapter { format!("{:<14} ", r["adapter"].as_str().unwrap_or("-")) } else { String::new() };
        println!(
            "{:<12} {:<20} {:<14} {:<8} {:<26} {:<26} {:<11} {adapter}{}",
            s("id"),
            s("name"),
            r["harness"].as_str().unwrap_or("-"),
            s("key"),
            s("created"),
            r["revoked"].as_str().unwrap_or("-"),
            r["break"].as_str().unwrap_or("default"),
            r["last_used"].as_str().unwrap_or("never")
        );
    }
}
