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
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Args as ClapArgs, Subcommand};
use nullrouter_cli::quota_text;
use nullrouter_engine::accounts::{self, Account, Accounts};
use nullrouter_engine::clock;
use nullrouter_engine::quota::fit::is_fitted_account;
use nullrouter_engine::quota::fit::outside::{self, AckTarget, Alert, OutsideEntry, OutsideType};
use nullrouter_engine::quota::history;
use nullrouter_registry::OperatorHome;
use nullrouter_registry::schema::parse_duration;
use nullrouter_server::operator::{self, CallError};
use nullrouter_server::views;
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
    /// Outside use: traffic on an account's quota that 0router did not send.
    Outside {
        /// Only this provider's accounts.
        provider: Option<String>,
        /// Only this account.
        account: Option<String>,
        /// Only entries starting at or after this RFC 3339 time.
        #[arg(long, value_name = "TIME")]
        since: Option<String>,
        /// At most this many entries per account.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Unacknowledged usage alerts.
    Alerts,
    /// Acknowledge an alert by its id or a unique prefix of it, or every alert with `all`.
    Ack {
        /// An alert id, a unique prefix of one, or `all`.
        id: String,
        /// Only this provider's accounts.
        provider: Option<String>,
        /// Only this account.
        account: Option<String>,
    },
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
        let view =
            super::read(&home, views::quota::NEEDS, &json!({"provider": provider, "name": name}), views::quota::build)?;
        print(view.json.as_array().map_or(&[][..], Vec::as_slice), as_json);
        if view.extra["offline"] == true {
            eprintln!("no server is running: quota is polled only while `nullrouter serve` runs");
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
            let args = json!({"provider": provider, "name": name, "since": since, "limit": limit});
            let view = super::read(&home, views::quota::HISTORY_NEEDS, &args, views::quota::history)?;
            let entries = view.json.as_array().map_or(&[][..], Vec::as_slice);
            if as_json {
                println!("{:#}", view.json);
            } else if entries.is_empty() {
                eprintln!("{provider}/{name}: no poll history{}", if since.is_some() { " in that range" } else { "" });
            } else {
                print!("{}", quota_text::history(entries, nullrouter_engine::clock::now(), 0));
            }
        }
        Command::Prune { before, provider, name } => {
            let at = date(&before)?;
            checkpoint(&home)?;
            // A running server folds and prunes itself: it owns the fit file while it runs.
            let req = json!({"op": "quota.prune", "before": clock::rfc3339(at), "provider": provider, "account": name});
            let n = if let Some(a) = ask(&home, &req)? {
                a["removed"].as_u64().unwrap_or(0) as usize
            } else {
                // The rows about to go are folded into each window's prior first (research R11).
                let list = Accounts::load(&home.path().join(accounts::FILE)).map_err(fail)?;
                let reg = crate::open(Some(home.path().to_owned()))?.snapshot();
                let windows = nullrouter_engine::quota::fit::learner::fold_prior(
                    home.path(),
                    &reg,
                    &list,
                    provider.as_deref(),
                    name.as_deref(),
                    at,
                )
                .map_err(fail)?;
                if windows > 0 && !as_json {
                    eprintln!("folded the pruned rows into the fit of {windows} window(s)");
                }
                history::prune(home.path(), at, provider.as_deref(), name.as_deref()).map_err(fail)?
            };
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
        Command::Outside { provider, account, since, limit } => {
            let since = since_arg(since)?;
            let list = Accounts::load(&home.path().join(accounts::FILE)).map_err(fail)?;
            let reg = crate::open(Some(home.path().to_owned()))?.snapshot();
            let mut rows = Vec::new();
            let mut lines = Vec::new();
            for a in list.iter().filter(|a| narrows(a, provider.as_deref(), account.as_deref())) {
                let label = format!("{}/{}", a.provider, a.name);
                if !reg.provider(&a.provider).is_ok_and(|p| is_fitted_account(p, a)) {
                    if !as_json {
                        lines.push(format!("{label:<24} not polled: no outside-use detection"));
                    }
                    continue;
                }
                for e in outside::read(home.path(), &a.provider, &a.name, since, limit).map_err(fail)? {
                    if as_json {
                        rows.push(tagged(serde_json::to_value(&e).unwrap_or(Value::Null), &a.provider, &a.name));
                    } else {
                        lines.push(outside_line(&label, &e));
                    }
                }
            }
            if as_json {
                println!("{:#}", Value::Array(rows));
            } else {
                for l in lines {
                    println!("{l}");
                }
            }
        }
        Command::Alerts => {
            let list = Accounts::load(&home.path().join(accounts::FILE)).map_err(fail)?;
            let mut rows = Vec::new();
            let mut lines = Vec::new();
            for a in list.iter() {
                for al in outside::alerts(home.path(), &a.provider, &a.name) {
                    if as_json {
                        rows.push(tagged(serde_json::to_value(&al).unwrap_or(Value::Null), &a.provider, &a.name));
                    } else {
                        lines.push(alert_line(&a.provider, &a.name, &al));
                    }
                }
            }
            if as_json {
                println!("{:#}", Value::Array(rows));
            } else {
                for l in lines {
                    println!("{l}");
                }
            }
        }
        Command::Ack { id, provider, account } => {
            // `all` names every open alert: the server reads a missing `id` that way.
            let prefix = (id != "all").then_some(id.as_str());
            let req = json!({"op": "quota.ack", "id": prefix, "provider": provider, "account": account});
            let acknowledged = match ask(&home, &req)? {
                Some(answer) => answer["acknowledged"].as_u64().unwrap_or(0) as usize,
                None => {
                    // No server: the files are all there is.
                    let target = prefix.map_or(AckTarget::All, AckTarget::Id);
                    let list = Accounts::load(&home.path().join(accounts::FILE)).map_err(fail)?;
                    let now = clock::now();
                    let mut n = 0;
                    for a in list.iter().filter(|a| narrows(a, provider.as_deref(), account.as_deref())) {
                        n += outside::ack(home.path(), &a.provider, &a.name, target, now).map_err(fail)?.len();
                    }
                    n
                }
            };
            if as_json {
                println!("{}", json!({"acknowledged": acknowledged}));
            } else {
                println!("acknowledged {acknowledged}");
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// `--since`: an RFC 3339 time, or none. A bad time is a refused input (exit 2).
fn since_arg(since: Option<String>) -> Result<Option<SystemTime>, ExitCode> {
    let Some(s) = since else { return Ok(None) };
    match clock::parse_rfc3339(&s) {
        Some(t) => Ok(Some(t)),
        None => {
            eprintln!("{s:?}: --since takes an RFC 3339 time, such as 2026-10-08T00:00:00Z");
            Err(ExitCode::from(2))
        }
    }
}

/// Whether `a` is one of the accounts `provider` and `account` (when given) narrow to.
fn narrows(a: &Account, provider: Option<&str>, account: Option<&str>) -> bool {
    provider.is_none_or(|p| a.provider == p) && account.is_none_or(|n| a.name == n)
}

/// `value` (a JSON object) with `provider` and `account` added.
fn tagged(mut value: Value, provider: &str, account: &str) -> Value {
    if let Some(obj) = value.as_object_mut() {
        obj.insert("provider".into(), json!(provider));
        obj.insert("account".into(), json!(account));
    }
    value
}

const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// `Wed 02:20`, in UTC.
fn weekday_hm(t: SystemTime) -> String {
    let days = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86_400);
    // 1970-01-01 was a Thursday.
    let hm = clock::rfc3339(t).get(11..16).unwrap_or("??:??").to_owned();
    format!("{} {hm}", DAYS[((days + 4) % 7) as usize])
}

/// `Wed 02:20` for an RFC 3339 time, `??` when it doesn't parse.
fn when_text(s: &str) -> String {
    clock::parse_rfc3339(s).map_or_else(|| "??".to_owned(), weekday_hm)
}

/// The amount with its unit: `4%`, `1200 tokens`.
fn quantity(x: f64, unit: &str) -> String {
    if unit == "percent" { format!("{x:.0}%") } else { format!("{x:.0} {unit}") }
}

/// One outside-use entry as the list shows it, the account's label first:
/// `anthropic/max            weekly   idle   Wed 02:10–02:30   4%`.
fn outside_line(label: &str, e: &OutsideEntry) -> String {
    let (kind, when, amount) = match e.ty {
        OutsideType::Idle | OutsideType::Busy => {
            let kind = if e.ty == OutsideType::Idle { "idle" } else { "busy" };
            let start = e.start_time().map_or_else(|| "??".to_owned(), weekday_hm);
            let end = e.end.as_deref().and_then(clock::parse_rfc3339).map_or_else(String::new, |t| {
                let hm = clock::rfc3339(t).get(11..16).unwrap_or("??:??").to_owned();
                format!("–{hm}")
            });
            let mut amount = quantity(e.amount.unwrap_or(0.0), &e.unit);
            if e.ty == OutsideType::Busy {
                amount.push_str(" beyond explained use");
            }
            (kind, format!("{start}{end}"), amount)
        }
        OutsideType::Steady => {
            let part = e.part.as_deref().unwrap_or("??").replace('-', "–");
            let since = e.start_time().map_or_else(|| "??".to_owned(), weekday_hm);
            let rate = e.rate_per_hour.unwrap_or(0.0);
            let amount = if e.unit == "percent" {
                format!("{rate:.1}%/h")
            } else {
                format!("{rate:.1} {}/h", e.unit)
            };
            ("steady", format!("{part} since {since}"), amount)
        }
    };
    let window = &e.window;
    format!("{label:<24} {window:<8} {kind:<6} {when}   {amount}")
}

/// One alert: `01JB7… anthropic/max  Wed 02:20  4% of weekly used …`. The id is printed whole.
fn alert_line(provider: &str, account: &str, al: &Alert) -> String {
    let text = al.text.as_deref().unwrap_or("");
    format!("{} {provider}/{account}  {}  {text}", al.id, when_text(&al.raised_at)).trim_end().to_owned()
}

fn date(s: &str) -> Result<SystemTime, ExitCode> {
    views::quota::date(s).map_err(|e| fail(e.message))
}

/// Asks a running server to write its queued poll entries and running tallies. No server:
/// the files are already all there is.
fn checkpoint(home: &OperatorHome) -> Result<(), ExitCode> {
    ask(home, &json!({"op": "quota.checkpoint"})).map(|_| ())
}
