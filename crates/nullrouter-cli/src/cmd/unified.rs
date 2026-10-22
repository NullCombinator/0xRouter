//! `nullrouter unified [NAME]`: exit 2 when NAME isn't a loaded unified model.

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args as ClapArgs, Subcommand};

use nullrouter_engine::verdict::State;
use nullrouter_registry::OperatorHome;
use nullrouter_server::ops::targets::{self, Kind};
use nullrouter_server::ops::{Ctx, Status};
use nullrouter_server::views;
use serde_json::{Value, json};

/// `nullrouter unified [NAME]` reads; `add`, `set` and `remove` change `config.toml`.
#[derive(ClapArgs)]
#[command(args_conflicts_with_subcommands = true)]
pub(crate) struct Args {
    /// A unified model to show; without it, all of them.
    pub(crate) name: Option<String>,
    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}

/// The changes, shared by `unified` and `combos`.
#[derive(Subcommand)]
pub(crate) enum Command {
    /// Declare a new one. Members are `provider/model` for a unified model, names for a combo.
    Add {
        name: String,
        #[arg(required = true)]
        members: Vec<String>,
    },
    /// Replace the members of a declared one.
    Set {
        name: String,
        #[arg(required = true)]
        members: Vec<String>,
    },
    /// Remove one that no combo lists.
    Remove {
        name: String,
        #[arg(long)]
        yes: bool,
    },
}

/// Runs a change: prints the one-line result, or the refusal and its exit code.
pub(crate) fn change(home: Option<PathBuf>, kind: Kind, cmd: Command, as_json: bool) -> Result<ExitCode, ExitCode> {
    let fail = |text: String, code: u8| {
        eprintln!("{text}");
        ExitCode::from(code)
    };
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    let ctx = Ctx { home, engine: None };
    let mut log = Vec::new();
    let result = match cmd {
        Command::Add { name, members } => targets::add_with(&ctx, &mut log, kind, &name, &members),
        Command::Set { name, members } => targets::set_with(&ctx, &mut log, kind, &name, &members),
        Command::Remove { name, yes } => {
            if as_json && !yes {
                return Err(fail("--json needs --yes".into(), 2));
            }
            if !yes {
                eprint!("{} [y/N] ", targets::remove_question(kind, &name));
                io::stderr().flush().map_err(|e| fail(e.to_string(), 1))?;
                let mut line = String::new();
                io::stdin().read_line(&mut line).map_err(|e| fail(e.to_string(), 1))?;
                if !matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                    println!("not removed");
                    return Err(ExitCode::from(1));
                }
            }
            targets::remove_with(&ctx, &mut log, kind, &name)
        }
    };
    for l in &log {
        eprintln!("{l}");
    }
    let r = result.map_err(|e| fail(e.text, e.exit))?;
    if let Status::SavedNotApplied { reason } = &r.done.status {
        return Err(fail(reason.clone(), 1));
    }
    if as_json {
        println!("{}", r.json);
    } else {
        println!("{}", r.done.text);
    }
    Ok(ExitCode::SUCCESS)
}

/// `  verdicts: max BROKEN, pro PASS` after member `i`, when any member of the model has a
/// verdict; a member with none reads `untested`.
fn verdicts_of(verdicts: &Value, i: usize) -> String {
    let any = verdicts.as_array().is_some_and(|all| all.iter().any(|m| m.as_array().is_some_and(|v| !v.is_empty())));
    if !any {
        return String::new();
    }
    let mine: Vec<String> = verdicts[i]
        .as_array()
        .into_iter()
        .flatten()
        .map(|v| {
            let state = v["state"].as_str().and_then(State::parse).map_or("?", State::label);
            format!("{} {state}", v["account"].as_str().unwrap_or_default())
        })
        .collect();
    if mine.is_empty() { "  verdicts: untested".to_owned() } else { format!("  verdicts: {}", mine.join(", ")) }
}

fn print_model(entry: &Value, notes: &Value, verdicts: &Value, indent: &str) {
    let name = entry["name"].as_str().unwrap_or_default();
    match entry["model_kind"].as_str() {
        Some(kind) => println!("{name}  {kind}"),
        None => println!("{name}"),
    }
    for (i, m) in entry["members"].as_array().into_iter().flatten().enumerate() {
        println!(
            "{indent}{i}. {} {} → upstream {}{}",
            m["provider"].as_str().unwrap_or_default(),
            m["requested"].as_str().unwrap_or_default(),
            m["upstream_id"].as_str().unwrap_or_default(),
            verdicts_of(verdicts, i)
        );
    }
    for n in notes[name].as_array().into_iter().flatten().filter_map(Value::as_str) {
        println!("{indent}note: {n}");
    }
}

pub(crate) fn run(home: Option<PathBuf>, args: Args, as_json: bool) -> Result<ExitCode, ExitCode> {
    let name = match args.command {
        Some(cmd) => return change(home, Kind::Unified, cmd, as_json),
        None => args.name.as_deref(),
    };
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    let view = super::read(&home, views::unified::NEEDS, &json!({"name": name}), views::unified::build)?;
    let out = &view.json;
    let code = if out["kind"] == "not_found" { 2 } else { 0 };
    if as_json {
        println!("{out:#}");
    } else if out["kind"] == "not_found" {
        eprintln!("not found: {}", out["error"].as_str().unwrap_or_default());
    } else if name.is_some() {
        print_model(out, &view.extra["notes"], &view.extra["verdicts"], "  ");
    } else {
        let (models, dropped) = (out["unified"].as_array().unwrap(), out["dropped"].as_array().unwrap());
        if models.is_empty() && dropped.is_empty() {
            println!("no unified models; declare one with [[unified_model]] in config.toml");
        }
        for m in models {
            print_model(m, &view.extra["notes"], &Value::Null, "  ");
        }
        for d in dropped {
            println!(
                "dropped unified model {}: {}",
                d["name"].as_str().unwrap_or_default(),
                d["reason"].as_str().unwrap_or_default()
            );
        }
    }
    Ok(ExitCode::from(code))
}
