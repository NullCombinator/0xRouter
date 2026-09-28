//! `zerorouter keys` (contracts/operator-cli.md).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;
use serde_json::json;
use zerorouter_engine::keys::{self, BreakBehaviour, Keys};
use zerorouter_registry::OperatorHome;

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Issue a key. It is printed once; only its digest is stored.
    Issue {
        name: String,
        #[arg(long = "break", value_name = "BEHAVIOUR")]
        break_behaviour: Option<String>,
    },
    List,
    /// Revoke a key by name or id.
    Revoke { key: String },
    /// Per-key break behaviour: `restart`, `error_event`, or `default`.
    SetBreak { key: String, behaviour: String },
}

fn fail(e: impl std::fmt::Display) -> ExitCode {
    eprintln!("{e}");
    ExitCode::from(1)
}

fn behaviour(s: &str) -> Result<BreakBehaviour, ExitCode> {
    BreakBehaviour::parse(s).ok_or_else(|| fail(format!("unknown break behaviour {s:?}; allowed: restart, error_event")))
}

pub(crate) fn run(home: Option<PathBuf>, cmd: Command, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    let mut list = Keys::load(&home.path().join(keys::FILE)).map_err(fail)?;
    let done = match cmd {
        Command::List => return Ok(print_list(&list, as_json)),
        Command::Issue { name, break_behaviour } => {
            let b = break_behaviour.as_deref().map(behaviour).transpose()?;
            let (key, rec) = list.issue(&name, b).map_err(fail)?;
            let (id, name) = (rec.id.clone(), rec.name.clone());
            list.save().map_err(fail)?;
            // The only time the key is shown: stdout carries just the key, for scripts.
            if as_json {
                println!("{}", json!({"id": id, "name": name, "key": key}));
            } else {
                eprintln!("issued {id} ({name}); it is shown once, only its digest is stored:");
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
    };
    list.save().map_err(fail)?;
    // The running server picks the file up over the operator socket (T104, T107).
    if as_json {
        println!("{}", json!({"key": done, "status": "saved"}));
    } else {
        println!("{done}: saved; applies at next start");
    }
    Ok(ExitCode::SUCCESS)
}

fn print_list(list: &Keys, as_json: bool) -> ExitCode {
    let rows: Vec<_> = list
        .iter()
        .map(|k| {
            json!({
                "id": k.id,
                "name": k.name,
                "key": format!("…{}", k.last4),
                "created": k.created,
                "revoked": k.revoked,
                "break": k.break_behaviour.map(BreakBehaviour::as_str),
            })
        })
        .collect();
    if as_json {
        println!("{:#}", json!(rows));
    } else {
        for (r, k) in rows.iter().zip(list.iter()) {
            println!(
                "{:<12} {:<20} {:<8} {:<26} {:<26} {}",
                k.id,
                k.name,
                r["key"].as_str().unwrap_or_default(),
                k.created,
                k.revoked.as_deref().unwrap_or("-"),
                k.break_behaviour.map_or("default", BreakBehaviour::as_str)
            );
        }
    }
    ExitCode::SUCCESS
}
