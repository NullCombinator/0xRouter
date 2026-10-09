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
        Command::Issue { name, break_behaviour, harness, adapter } => {
            let b = break_behaviour.as_deref().map(behaviour).transpose()?;
            // Checked before anything is issued, so a refused tag or adapter writes nothing.
            let tag = harness.as_deref().map(keys::check_harness).transpose().map_err(fail)?;
            let h = adapter.as_deref().map(harness_name).transpose()?;
            let (key, rec) = list.issue(&name, b).map_err(fail)?;
            let (id, name) = (rec.id.clone(), rec.name.clone());
            if tag.is_some() {
                list.set_harness(&id, tag.as_deref()).map_err(fail)?;
            }
            if h.is_some() {
                list.set_adapter(&id, h.clone()).map_err(fail)?;
            }
            list.save().map_err(fail)?;
            let status = super::apply(&home).map_err(fail)?;
            // The only time the key is shown: stdout carries just the key, for scripts.
            if as_json {
                let adapter = h.as_ref().map(HarnessName::as_str);
                println!(
                    "{}",
                    json!({"id": id, "name": name, "harness": tag, "adapter": adapter, "key": key, "status": status})
                );
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
        Command::Tag { key, text, clear } => {
            let text = match (text, clear) {
                (Some(t), false) => Some(t),
                (None, true) => None,
                _ => return Err(fail("give a TEXT or --clear")),
            };
            list.set_harness(&key, text.as_deref()).map_err(|e| match e {
                keys::KeyError::NotFound(_) => {
                    eprintln!("no key {key:?}");
                    ExitCode::from(2)
                }
                e => fail(e),
            })?;
            let rec = list.iter().find(|k| k.id == key || k.name == key).expect("just tagged");
            let (id, name, tag) = (rec.id.clone(), rec.name.clone(), rec.harness.clone());
            list.save().map_err(fail)?;
            let status = super::apply(&home).map_err(fail)?;
            if as_json {
                println!("{}", json!({"id": id, "name": name, "harness": tag, "status": status}));
            } else {
                match &tag {
                    Some(t) => println!("{name}: harness {t}"),
                    None => println!("{name}: no harness"),
                }
                println!("{status}");
            }
            return Ok(ExitCode::SUCCESS);
        }
        Command::SetBreak { key, behaviour: b } => {
            let b = if b == "default" { None } else { Some(behaviour(&b)?) };
            list.set_break(&key, b).map_err(fail)?;
            key
        }
        Command::SetAdapter { key, harness, clear } => {
            let h = if clear { None } else { harness.as_deref().map(harness_name).transpose()? };
            list.set_adapter(&key, h).map_err(fail)?;
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
    // The adapter column appears once any key is bound to one.
    let any_adapter = rows.as_array().into_iter().flatten().any(|r| r["adapter"].is_string());
    for r in rows.as_array().into_iter().flatten() {
        let s = |k: &str| r[k].as_str().unwrap_or_default().to_owned();
        let adapter = if any_adapter { format!("{:<14} ", r["adapter"].as_str().unwrap_or("-")) } else { String::new() };
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
