//! `nullrouter quota` (slice 005 contracts/operator-cli.md § quota): provider-reported quota per
//! account, its poll history, and the polling interval.
//!
//! Polls run in the server (research R11, R13): `quota` reads its latest polls over the
//! operator socket, and `quota poll` asks it to poll now (exit 4 with no server). `quota
//! interval` writes `poll_interval` to `accounts.toml` and tells a running server to reload.
//!
//! The history files (`quota/<provider>/<account>.jsonl`, research R15) are read and edited
//! here directly. When a server runs, `quota history`, `prune` and `forget` first ask it for
//! `quota.checkpoint`, so every poll it kept and every running tally is on disk.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::SystemTime;

use clap::{Args as ClapArgs, Subcommand};
use nullrouter_cli::quota_text;
use nullrouter_engine::accounts::{self, Accounts};
use nullrouter_engine::clock;
use nullrouter_engine::quota::{history, poll};
use nullrouter_registry::OperatorHome;
use nullrouter_registry::schema::parse_duration;
use nullrouter_server::operator::{self, CallError};
use serde_json::{Value, json};

/// `quota [provider [name]]` shows the current windows; the subcommands do the rest.
#[derive(ClapArgs)]
#[command(args_conflicts_with_subcommands = true)]
pub(crate) struct Args {
    #[command(subcommand)]
    command: Option<Command>,
    /// Only this provider's accounts.
    provider: Option<String>,
    /// Only this account.
    name: Option<String>,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Poll entries of one account, with the traffic tally of each interval.
    History {
        provider: String,
        name: String,
        /// `YYYY-MM-DD` or an RFC 3339 time.
        #[arg(long, value_name = "DATE")]
        since: Option<String>,
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Poll one account now (needs a running server).
    Poll { provider: String, name: String },
    /// Set an account's polling interval (`15m`, `1h`, or `default`).
    Interval { provider: String, name: String, every: String },
    /// Delete history entries older than DATE.
    Prune {
        #[arg(long, value_name = "DATE")]
        before: String,
        provider: Option<String>,
        name: Option<String>,
    },
    /// Delete one account's history.
    Forget { provider: String, name: String },
}

fn fail(e: impl std::fmt::Display) -> ExitCode {
    eprintln!("{e}");
    ExitCode::from(1)
}

/// The server's answer; `Ok(None)` when no server runs.
fn ask(home: &OperatorHome, req: &Value) -> Result<Option<Value>, ExitCode> {
    match operator::call(home, req) {
        Ok(a) if a["ok"] == true => Ok(Some(a)),
        Ok(a) => Err(fail(a["error"].as_str().unwrap_or("the server refused the request"))),
        Err(CallError::NoServer(_)) => Ok(None),
        Err(e) => Err(fail(e)),
    }
}

fn print(accounts: &[Value], as_json: bool) {
    if as_json {
        println!("{:#}", Value::Array(accounts.to_vec()));
    } else {
        print!("{}", quota_text::render(accounts, nullrouter_engine::clock::now(), 0));
    }
}

pub(crate) fn run(home: Option<PathBuf>, args: Args, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    let Some(cmd) = args.command else {
        let (provider, name) = (args.provider, args.name);
        let req = json!({"op": "quota.list", "provider": provider, "name": name});
        match ask(&home, &req)? {
            Some(a) => print(a["accounts"].as_array().map_or(&[][..], Vec::as_slice), as_json),
            None => offline(&home, provider.as_deref(), name.as_deref(), as_json)?,
        }
        return Ok(ExitCode::SUCCESS);
    };
    match cmd {
        Command::Poll { provider, name } => {
            let req = json!({"op": "quota.poll", "provider": provider, "name": name});
            let Some(_) = ask(&home, &req)? else {
                eprintln!(
                    "no server is running on {}; polls run in the server (start it with `nullrouter serve`)",
                    operator::socket_path(&home).display()
                );
                return Err(ExitCode::from(4));
            };
            let list =
                ask(&home, &json!({"op": "quota.list", "provider": provider, "name": name}))?.unwrap_or_default();
            print(list["accounts"].as_array().map_or(&[][..], Vec::as_slice), as_json);
        }
        Command::Interval { provider, name, every } => {
            let wanted = match every.as_str() {
                "default" => None,
                s => Some(parse_duration(s).map_err(|e| fail(format!("{s:?}: {e}")))?),
            };
            let mut list = Accounts::load(&home.path().join(accounts::FILE)).map_err(fail)?;
            let now = list.set_poll_interval(&provider, &name, wanted).map_err(fail)?;
            list.save().map_err(fail)?;
            let status = super::apply(&home).map_err(fail)?;
            let raised = wanted.is_some_and(|w| w < now);
            let note = if raised { " (raised to the floor)" } else { "" };
            let every = quota_text::every(now.as_secs());
            if as_json {
                println!(
                    "{}",
                    json!({"provider": provider, "name": name, "interval_s": now.as_secs(), "status": status})
                );
            } else {
                println!("{provider}/{name}: polls every {every}{note}; {status}");
            }
        }
        Command::History { provider, name, since, limit } => {
            let since = since.as_deref().map(date).transpose()?;
            checkpoint(&home)?;
            let entries = history::read(home.path(), &provider, &name, since, limit).map_err(fail)?;
            let entries: Vec<Value> = entries.iter().filter_map(|e| serde_json::to_value(e).ok()).collect();
            if as_json {
                println!("{:#}", Value::Array(entries));
            } else if entries.is_empty() {
                eprintln!("{provider}/{name}: no poll history{}", if since.is_some() { " in that range" } else { "" });
            } else {
                print!("{}", quota_text::history(&entries, nullrouter_engine::clock::now(), 0));
            }
        }
        Command::Prune { before, provider, name } => {
            let at = date(&before)?;
            checkpoint(&home)?;
            let n = history::prune(home.path(), at, provider.as_deref(), name.as_deref()).map_err(fail)?;
            if as_json {
                println!("{}", json!({"removed": n, "before": clock::rfc3339(at)}));
            } else {
                let s = if n == 1 { "entry" } else { "entries" };
                println!("deleted {n} history {s} older than {}", clock::rfc3339(at));
            }
        }
        Command::Forget { provider, name } => {
            checkpoint(&home)?;
            let found = history::forget(home.path(), &provider, &name).map_err(fail)?;
            if as_json {
                println!("{}", json!({"provider": provider, "name": name, "forgotten": found}));
            } else if found {
                println!("{provider}/{name}: poll history deleted");
            } else {
                println!("{provider}/{name}: no poll history");
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// `DATE`: RFC 3339, or `YYYY-MM-DD` (midnight UTC).
fn date(s: &str) -> Result<SystemTime, ExitCode> {
    let full = if s.len() == 10 { format!("{s}T00:00:00Z") } else { s.to_owned() };
    clock::parse_rfc3339(&full)
        .ok_or_else(|| fail(format!("{s:?} is not a date (use YYYY-MM-DD or 2026-10-03T14:00:00Z)")))
}

/// Asks a running server to write its queued poll entries and running tallies. No server:
/// the files are already all there is.
fn checkpoint(home: &OperatorHome) -> Result<(), ExitCode> {
    ask(home, &json!({"op": "quota.checkpoint"})).map(|_| ())
}

/// With no server: the accounts and whether their provider reports quota, and why nothing
/// has been polled.
fn offline(home: &OperatorHome, provider: Option<&str>, name: Option<&str>, as_json: bool) -> Result<(), ExitCode> {
    let list = Accounts::load(&home.path().join(accounts::FILE)).map_err(fail)?;
    let reg = crate::open(Some(home.path().to_owned()))?.snapshot();
    let accounts: Vec<Value> = list
        .iter()
        .filter(|a| provider.is_none_or(|p| a.provider == p) && name.is_none_or(|n| a.name == n))
        .map(|a| {
            let reported = reg.provider(&a.provider).is_ok_and(|p| poll::reported(p, a).is_some());
            json!({
                "provider": a.provider,
                "name": a.name,
                "kind": if a.is_signin() { "signin" } else { "key" },
                "reported": reported,
                "interval_s": a.poll_interval().as_secs(),
                "latest": null,
                "last_failure": null,
            })
        })
        .collect();
    print(&accounts, as_json);
    eprintln!("no server is running: quota is polled only while `nullrouter serve` runs");
    Ok(())
}
