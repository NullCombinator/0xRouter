//! `nullrouter proxy` (spec 013, contracts/cli.md § `nullrouter proxy`): define proxies in
//! `proxies.toml`, choose which providers and accounts use them, and resume a paused one.
//!
//! A password comes from stdin or an environment variable, never from argv, and is never printed.

use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand};
use nullrouter_engine::connection::pause::ProxyBoard;
use nullrouter_engine::connection::proxy::{self, Proxies};
use nullrouter_registry::{OperatorHome, SecretString};
use nullrouter_server::operator;
use nullrouter_server::ops::{self, Ctx, OpError};
use serde_json::{Value, json};

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Define a proxy. With `--username`, the password is read from stdin unless
    /// `--password-env` names a variable.
    Add {
        name: String,
        /// `http://host:port`, `https://host:port` or `socks5://host:port`, without credentials.
        url: String,
        #[arg(long)]
        username: Option<String>,
        #[arg(long, value_name = "VAR")]
        password_env: Option<String>,
    },
    /// List the proxies: address, whether a user is set, state, and where each is used.
    List,
    /// Remove a proxy that nothing uses.
    Remove { name: String },
    /// Send traffic through a proxy (or `none` for a direct connection).
    Use {
        /// A proxy name, or `none`.
        proxy: String,
        #[command(flatten)]
        target: Target,
    },
    /// Remove an assignment, so the level above applies again.
    Clear {
        #[command(flatten)]
        target: Target,
    },
    /// Probe a paused proxy now, and resume traffic through it if it answers.
    Fixed { name: String },
}

#[derive(Args)]
#[group(required = true, multiple = false)]
pub(crate) struct Target {
    /// Every provider that has no assignment of its own.
    #[arg(long)]
    all: bool,
    /// One provider.
    #[arg(long, value_name = "PROVIDER")]
    provider: Option<String>,
    /// One account, as `provider/name`.
    #[arg(long, value_name = "PROVIDER/NAME")]
    account: Option<String>,
}

fn fail(e: impl std::fmt::Display) -> ExitCode {
    eprintln!("{e}");
    ExitCode::from(1)
}

fn op_fail(e: OpError) -> ExitCode {
    eprintln!("{}", e.text);
    ExitCode::from(e.exit)
}

impl Target {
    fn parsed(&self) -> ops::proxy::Target {
        if let Some(spec) = &self.account {
            ops::proxy::Target::Account(spec.clone())
        } else if let Some(p) = &self.provider {
            ops::proxy::Target::Provider(p.clone())
        } else {
            ops::proxy::Target::All
        }
    }
}

pub(crate) fn run(home: Option<PathBuf>, cmd: Command, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    let ctx = Ctx { home, engine: None };
    let home = &ctx.home;
    let mut log = Vec::new();
    let result = match cmd {
        Command::Add { name, url, username, password_env } => {
            ops::proxy::add_with(&ctx, &mut log, &name, &url, username.as_deref(), password_env.as_deref(), || {
                password_from_stdin().map(SecretString::new).map_err(|text| OpError { text, exit: 1, field: None })
            })
        }
        Command::List => {
            let rows = rows(home).map_err(fail)?;
            if as_json {
                println!("{:#}", json!(rows));
            } else {
                print!("{}", render(&rows));
            }
            return Ok(ExitCode::SUCCESS);
        }
        Command::Remove { name } => ops::proxy::remove(&ctx, &mut log, &name),
        Command::Use { proxy: name, target } => ops::proxy::use_proxy(&ctx, &mut log, &name, &target.parsed()),
        Command::Clear { target } => ops::proxy::clear(&ctx, &mut log, &target.parsed()),
        Command::Fixed { name } => ops::proxy::fixed_with(&ctx, &name),
    };
    for l in &log {
        eprintln!("{l}");
    }
    println!("{}", result.map_err(op_fail)?.text);
    Ok(ExitCode::SUCCESS)
}

/// The password piped in on stdin, without its trailing newline.
fn password_from_stdin() -> Result<String, String> {
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        eprintln!("warning: reading the password from the terminal; it is not hidden as you type.");
        eprintln!("Pipe it in instead, or use --password-env. Type the password, then Enter and Ctrl-D:");
    }
    let mut s = String::new();
    stdin.lock().read_to_string(&mut s).map_err(|e| e.to_string())?;
    let s = s.trim_end_matches(['\r', '\n']).to_owned();
    if s.is_empty() {
        return Err("no password on stdin".to_owned());
    }
    Ok(s)
}

/// The paused proxies: the running server's answer, else the saved state.
fn paused(home: &OperatorHome) -> Vec<String> {
    if let Ok(a) = operator::call(home, &json!({"op": "live.snapshot"}))
        && let Some(list) = a["paused_proxies"].as_array()
    {
        return list.iter().filter_map(|p| p["name"].as_str().map(str::to_owned)).collect();
    }
    ProxyBoard::open(home.path()).list().into_iter().map(|(n, _)| n).collect()
}

fn rows(home: &OperatorHome) -> Result<Vec<Value>, String> {
    let list = Proxies::load(&home.path().join(proxy::FILE)).map_err(|e| e.to_string())?;
    let paused = paused(home);
    list.iter()
        .map(|p| {
            Ok(json!({
                "name": p.name,
                "url": p.shown(),
                "user": p.username.is_some(),
                "state": if paused.contains(&p.name) { "paused" } else { "in use" },
                "used_by": ops::proxy::used_by(home, &p.name)?,
            }))
        })
        .collect()
}

fn render(rows: &[Value]) -> String {
    if rows.is_empty() {
        return "no proxies; add one with `nullrouter proxy add <name> <url>`\n".to_owned();
    }
    let mut out = format!("{:<16} {:<32} {:<6} {:<8} USED BY\n", "NAME", "ADDRESS", "USER", "STATE");
    for r in rows {
        let used: Vec<&str> = r["used_by"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        out += &format!(
            "{:<16} {:<32} {:<6} {:<8} {}\n",
            r["name"].as_str().unwrap_or(""),
            r["url"].as_str().unwrap_or(""),
            if r["user"] == true { "user ✓" } else { "—" },
            r["state"].as_str().unwrap_or(""),
            if used.is_empty() { "—".to_owned() } else { used.join(", ") },
        );
    }
    out
}
