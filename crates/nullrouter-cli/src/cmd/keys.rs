//! `nullrouter keys` (contracts/operator-cli.md).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;
use nullrouter_engine::keys::{self, BreakBehaviour, HarnessName, Keys};
use nullrouter_registry::OperatorHome;
use nullrouter_server::views;
use serde_json::{Value, json};

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Issue a key. It is printed once; only its digest is stored.
    Issue {
        name: String,
        #[arg(long = "break", value_name = "BEHAVIOUR")]
        break_behaviour: Option<String>,
        /// The client harness this key's requests come from; its adapter runs on them.
        #[arg(long, value_name = "HARNESS")]
        harness: Option<String>,
    },
    List,
    /// Revoke a key by name or id.
    Revoke {
        key: String,
    },
    /// Per-key break behaviour: `restart`, `error_event`, or `default`.
    SetBreak {
        key: String,
        behaviour: String,
    },
    /// Bind a key to a harness, or unbind it with `--clear`.
    SetHarness {
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

fn harness_name(s: &str) -> Result<HarnessName, ExitCode> {
    HarnessName::new(s).map_err(fail)
}

fn behaviour(s: &str) -> Result<BreakBehaviour, ExitCode> {
    BreakBehaviour::parse(s)
        .ok_or_else(|| fail(format!("unknown break behaviour {s:?}; allowed: restart, error_event")))
}

pub(crate) fn run(home: Option<PathBuf>, cmd: Command, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    if matches!(cmd, Command::List) {
        let view = super::read(&home, views::keys::NEEDS, &json!({}), views::keys::build)?;
        print_list(&view.json, as_json);
        return Ok(ExitCode::SUCCESS);
    }
    let mut list = Keys::load(&home.path().join(keys::FILE)).map_err(fail)?;
    let done = match cmd {
        Command::List => unreachable!("handled above"),
        Command::Issue { name, break_behaviour, harness } => {
            let b = break_behaviour.as_deref().map(behaviour).transpose()?;
            let h = harness.as_deref().map(harness_name).transpose()?;
            let (key, rec) = list.issue(&name, b).map_err(fail)?;
            let (id, name) = (rec.id.clone(), rec.name.clone());
            if h.is_some() {
                list.set_harness(&id, h).map_err(fail)?;
            }
            list.save().map_err(fail)?;
            let status = super::apply(&home).map_err(fail)?;
            // The only time the key is shown: stdout carries just the key, for scripts.
            if as_json {
                println!("{}", json!({"id": id, "name": name, "key": key, "status": status}));
            } else {
                eprintln!("issued {id} ({name}), {status}; it is shown once, only its digest is stored:");
                println!("{key}");
            }
            return Ok(ExitCode::SUCCESS);
        }
        Command::Revoke { key } => {
            list.revoke(&key).map_err(fail)?;
            key
        }
        Command::SetBreak { key, behaviour: b } => {
            let b = if b == "default" { None } else { Some(behaviour(&b)?) };
            list.set_break(&key, b).map_err(fail)?;
            key
        }
        Command::SetHarness { key, harness, clear } => {
            let h = if clear { None } else { harness.as_deref().map(harness_name).transpose()? };
            list.set_harness(&key, h).map_err(fail)?;
            key
        }
    };
    list.save().map_err(fail)?;
    let status = super::apply(&home).map_err(fail)?;
    if as_json {
        println!("{}", json!({"key": done, "status": status}));
    } else {
        println!("{done}: {status}");
    }
    Ok(ExitCode::SUCCESS)
}

fn print_list(rows: &Value, as_json: bool) {
    if as_json {
        println!("{rows:#}");
        return;
    }
    // The harness column appears once any key is bound to one.
    let any_harness = rows.as_array().into_iter().flatten().any(|r| r["harness"].is_string());
    for r in rows.as_array().into_iter().flatten() {
        let s = |k: &str| r[k].as_str().unwrap_or_default().to_owned();
        let harness = if any_harness { format!("{:<14} ", r["harness"].as_str().unwrap_or("-")) } else { String::new() };
        println!(
            "{:<12} {:<20} {:<8} {:<26} {:<26} {:<11} {harness}{}",
            s("id"),
            s("name"),
            s("key"),
            s("created"),
            r["revoked"].as_str().unwrap_or("-"),
            r["break"].as_str().unwrap_or("default"),
            r["last_used"].as_str().unwrap_or("never")
        );
    }
}
