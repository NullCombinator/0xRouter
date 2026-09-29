//! `zerorouter accounts` (contracts/operator-cli.md).

use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;
use serde_json::{Value, json};
use zerorouter_engine::accounts::{self, Account, Accounts, SecretSource};
use zerorouter_registry::{OperatorHome, SecretString};
use zerorouter_server::operator;

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Add an account. The secret is read from stdin unless `--env` names a variable.
    Add {
        provider: String,
        name: String,
        #[arg(long, value_name = "VAR")]
        env: Option<String>,
        #[arg(long, value_name = "N")]
        order: Option<i64>,
    },
    /// List accounts, optionally for one provider.
    List {
        provider: Option<String>,
    },
    Remove {
        provider: String,
        name: String,
    },
    Disable {
        provider: String,
        name: String,
    },
    Enable {
        provider: String,
        name: String,
    },
}

fn fail(e: impl std::fmt::Display) -> ExitCode {
    eprintln!("{e}");
    ExitCode::from(1)
}

/// The secret piped in on stdin, without its trailing newline. A terminal works too, but
/// what is typed shows on screen, so say so first.
fn secret_from_stdin() -> Result<String, ExitCode> {
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        eprintln!("warning: reading the secret from the terminal; it is not hidden as you type.");
        eprintln!("Pipe it in instead, for example: printenv MY_KEY | zerorouter accounts add …");
        eprintln!("Type the secret, then Enter and Ctrl-D:");
    }
    let mut s = String::new();
    stdin.lock().read_to_string(&mut s).map_err(fail)?;
    let s = s.trim_end_matches(['\r', '\n']).to_owned();
    if s.trim().is_empty() {
        return Err(fail("no secret on stdin"));
    }
    Ok(s)
}

pub(crate) fn run(home: Option<PathBuf>, cmd: Command, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    let mut list = Accounts::load(&home.path().join(accounts::FILE)).map_err(fail)?;
    let (provider, name) = match cmd {
        Command::List { provider } => return Ok(print_list(&home, &list, provider.as_deref(), as_json)),
        Command::Add { provider, name, env, order } => {
            if !accounts::valid_name(&name) {
                return Err(fail(accounts::AccountError::BadName(name)));
            }
            // The secret may only go to the hosts the provider sends to now (FR-044).
            let reg = crate::open(Some(home.path().to_owned()))?.snapshot();
            let hosts = match reg.provider(&provider) {
                Ok(p) => accounts::provider_hosts(p),
                Err(e) => return Err(fail(e)),
            };
            let (source, secret) = match env {
                Some(var) => {
                    let secret = std::env::var(&var).ok().filter(|s| !s.is_empty()).map(SecretString::new);
                    if secret.is_none() {
                        eprintln!("warning: {var} is not set here; the server reads it when it starts");
                    }
                    (SecretSource::Env(var), secret)
                }
                None => (SecretSource::Literal, Some(SecretString::new(secret_from_stdin()?))),
            };
            // Without `--order`, a new account goes after the provider's others.
            let order = order
                .unwrap_or_else(|| list.iter().filter(|a| a.provider == provider).map(|a| a.order).max().unwrap_or(0));
            let account = Account {
                provider: provider.clone(),
                name: name.clone(),
                source,
                secret,
                order,
                disabled: false,
                hosts,
            };
            list.add(account).map_err(fail)?;
            (provider, name)
        }
        Command::Remove { provider, name } => {
            list.remove(&provider, &name).map_err(fail)?;
            (provider, name)
        }
        Command::Disable { provider, name } => {
            list.set_disabled(&provider, &name, true).map_err(fail)?;
            (provider, name)
        }
        Command::Enable { provider, name } => {
            list.set_disabled(&provider, &name, false).map_err(fail)?;
            (provider, name)
        }
    };
    list.save().map_err(fail)?;
    let status = super::apply(&home).map_err(fail)?;
    if as_json {
        println!("{}", json!({"provider": provider, "name": name, "status": status}));
    } else {
        println!("{provider}/{name}: {status}");
    }
    Ok(ExitCode::SUCCESS)
}

/// `active`, `disabled`, or the running server's rests: `cooling <model> 12 s, …`.
fn state(a: &Account, live: &Value) -> String {
    if a.disabled {
        return "disabled".into();
    }
    let rests: Vec<String> = live["accounts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["provider"] == a.provider.as_str() && s["name"] == a.name.as_str())
        .flat_map(|s| s["cooling"].as_array().cloned().unwrap_or_default())
        .map(|c| {
            format!(
                "{} {} s",
                c["model"].as_str().unwrap_or("?"),
                c["remaining_ms"].as_u64().unwrap_or(0).div_ceil(1000)
            )
        })
        .collect();
    if rests.is_empty() { "active".into() } else { format!("cooling {}", rests.join(", ")) }
}

fn print_list(home: &OperatorHome, list: &Accounts, provider: Option<&str>, as_json: bool) -> ExitCode {
    // Cooldowns live in the running server; without one, every account is at rest.
    let live = operator::call(home, &json!({"op": "accounts.state"})).unwrap_or(Value::Null);
    let rows: Vec<_> = list
        .iter()
        .filter(|a| provider.is_none_or(|p| a.provider == p))
        .map(|a| {
            json!({
                "provider": a.provider,
                "name": a.name,
                "order": a.order,
                "secret": a.shown_secret(),
                "state": state(a, &live),
            })
        })
        .collect();
    if as_json {
        println!("{:#}", json!(rows));
    } else {
        for r in &rows {
            println!(
                "{:<20} {:<20} {:>5} {:<24} {}",
                r["provider"].as_str().unwrap_or_default(),
                r["name"].as_str().unwrap_or_default(),
                r["order"],
                r["secret"].as_str().unwrap_or_default(),
                r["state"].as_str().unwrap_or_default()
            );
        }
    }
    ExitCode::SUCCESS
}
