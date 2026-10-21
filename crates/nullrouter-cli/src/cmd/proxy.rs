//! `nullrouter proxy` (spec 013, contracts/cli.md § `nullrouter proxy`): define proxies in
//! `proxies.toml`, choose which providers and accounts use them, and resume a paused one.
//!
//! A password comes from stdin or an environment variable, never from argv, and is never printed.

use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, Subcommand};
use nullrouter_engine::accounts::{self, Accounts};
use nullrouter_engine::connection::pause::ProxyBoard;
use nullrouter_engine::connection::proxy::{self, PasswordSource, Proxies, Proxy};
use nullrouter_registry::schema::OperatorConfig;
use nullrouter_registry::{OperatorHome, SecretString};
use nullrouter_server::operator::{self, CallError};
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

pub(crate) fn run(home: Option<PathBuf>, cmd: Command, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    let file = home.path().join(proxy::FILE);
    match cmd {
        Command::Add { name, url, username, password_env } => {
            let mut list = Proxies::load(&file).map_err(fail)?;
            let (password, secret) = match (&username, password_env) {
                (_, Some(var)) => {
                    let value = std::env::var(&var).ok().filter(|v| !v.is_empty());
                    (Some(PasswordSource::Env(var)), value.map(SecretString::new))
                }
                (Some(_), None) => (Some(PasswordSource::Literal), Some(SecretString::new(password_from_stdin()?))),
                (None, None) => (None, None),
            };
            list.add(Proxy { name: name.clone(), url, username, password, secret }).map_err(fail)?;
            list.save(&file).map_err(fail)?;
            println!("proxy {name} added: {}", crate::cmd::apply(&home).map_err(fail)?);
        }
        Command::List => {
            let rows = rows(&home).map_err(fail)?;
            if as_json {
                println!("{:#}", json!(rows));
            } else {
                print!("{}", render(&rows));
            }
        }
        Command::Remove { name } => {
            let mut list = Proxies::load(&file).map_err(fail)?;
            if list.get(&name).is_none() {
                return Err(fail(format!("no proxy {name:?}; known proxies: {}", names(&list))));
            }
            let used = used_by(&home, &name).map_err(fail)?;
            if !used.is_empty() {
                return Err(fail(format!(
                    "proxy {name} is in use by {}; run `nullrouter proxy clear` for each first",
                    used.join(", ")
                )));
            }
            list.remove(&name);
            list.save(&file).map_err(fail)?;
            println!("proxy {name} removed: {}", crate::cmd::apply(&home).map_err(fail)?);
        }
        Command::Use { proxy: name, target } => {
            if name != proxy::NONE {
                let list = Proxies::load(&file).map_err(fail)?;
                if list.get(&name).is_none() {
                    return Err(fail(format!("no proxy {name:?}; known proxies: {}, or none", names(&list))));
                }
            }
            let said = assign(&home, &target, Some(&name)).map_err(fail)?;
            println!("{said}");
        }
        Command::Clear { target } => {
            let said = assign(&home, &target, None).map_err(fail)?;
            println!("{said}");
        }
        Command::Fixed { name } => return fixed(&home, &name),
    }
    Ok(ExitCode::SUCCESS)
}

fn names(list: &Proxies) -> String {
    let all: Vec<&str> = list.iter().map(|p| p.name.as_str()).collect();
    if all.is_empty() { "(none defined)".to_owned() } else { all.join(", ") }
}

/// The password piped in on stdin, without its trailing newline.
fn password_from_stdin() -> Result<String, ExitCode> {
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        eprintln!("warning: reading the password from the terminal; it is not hidden as you type.");
        eprintln!("Pipe it in instead, or use --password-env. Type the password, then Enter and Ctrl-D:");
    }
    let mut s = String::new();
    stdin.lock().read_to_string(&mut s).map_err(fail)?;
    let s = s.trim_end_matches(['\r', '\n']).to_owned();
    if s.is_empty() {
        return Err(fail("no password on stdin"));
    }
    Ok(s)
}

/// Where `name` is assigned: `all providers`, `provider X`, `account X/Y`.
fn used_by(home: &OperatorHome, name: &str) -> Result<Vec<String>, String> {
    let handle = nullrouter_server::views::open_registry(home).map_err(|e| e.message)?;
    let registry = handle.snapshot();
    let mut out = Vec::new();
    if registry.runtime().connection_proxy.as_deref() == Some(name) {
        out.push("all providers".to_owned());
    }
    for p in registry.providers() {
        if registry.settings(&p.id).connection.proxy.as_deref() == Some(name) {
            out.push(format!("provider {}", p.id));
        }
    }
    let list = Accounts::load(&home.path().join(accounts::FILE)).map_err(|e| e.to_string())?;
    out.extend(
        list.iter().filter(|a| a.proxy.as_deref() == Some(name)).map(|a| format!("account {}/{}", a.provider, a.name)),
    );
    Ok(out)
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
                "used_by": used_by(home, &p.name)?,
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

/// Sets (`Some`) or clears (`None`) the assignment at `target`, then asks the server to reload.
fn assign(home: &OperatorHome, target: &Target, value: Option<&str>) -> Result<String, String> {
    if let Some(spec) = &target.account {
        let (provider, name) = spec.split_once('/').ok_or("--account takes provider/name, such as kiro/main")?;
        let path = home.path().join(accounts::FILE);
        let mut list = Accounts::load(&path).map_err(|e| e.to_string())?;
        if list.get(provider, name).is_none() {
            let known: Vec<String> = list.iter().map(|a| format!("{}/{}", a.provider, a.name)).collect();
            return Err(format!("no account {spec:?}; known accounts: {}", known.join(", ")));
        }
        list.set_proxy(provider, name, value.map(str::to_owned)).map_err(|e| e.to_string())?;
        list.save().map_err(|e| e.to_string())?;
        return Ok(format!("account {spec}: {}: {}", said(value), crate::cmd::apply(home)?));
    }
    let (header, what) = match &target.provider {
        Some(p) => {
            let handle = nullrouter_server::views::open_registry(home).map_err(|e| e.message)?;
            let snapshot = handle.snapshot();
            let known: Vec<&str> = snapshot.providers().map(|p| p.id.as_str()).collect();
            if !known.contains(&p.as_str()) {
                return Err(format!("no provider {p:?}; known providers: {}", known.join(", ")));
            }
            (format!("provider.{p}.connection"), format!("provider {p}"))
        }
        None => ("connection".to_owned(), "all providers".to_owned()),
    };
    let path = home.config_file();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let quoted = value.map(|v| format!("\"{v}\""));
    let (edited, changed) = crate::cmd::connection::edit(&text, &header, "proxy", quoted.as_deref());
    if !changed {
        return Ok(format!("{what}: no proxy was assigned"));
    }
    toml::from_str::<OperatorConfig>(&edited)
        .map_err(|e| format!("{}: the edit would not load: {e}", path.display()))?;
    std::fs::create_dir_all(home.path()).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, &edited)
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(format!("{what}: {}: {}", said(value), crate::cmd::apply(home)?))
}

fn said(value: Option<&str>) -> String {
    value.map_or_else(|| "assignment removed".to_owned(), |v| format!("proxy {v}"))
}

/// `proxy fixed`: the running server probes; with none running the probe is made here and the
/// saved pause cleared, which the next `serve` reads.
fn fixed(home: &OperatorHome, name: &str) -> Result<ExitCode, ExitCode> {
    match operator::call(home, &json!({"op": "proxy.fixed", "name": name})) {
        Ok(a) if a["ok"] == true => return report(name, a["reachable"] == true, a["reason"].as_str()),
        Ok(a) => return Err(fail(a["error"].as_str().unwrap_or("the server refused the request"))),
        Err(CallError::NoServer(_)) => {}
        Err(e) => return Err(fail(e)),
    }
    let list = Proxies::load(&home.path().join(proxy::FILE)).map_err(fail)?;
    let Some(p) = list.get(name) else {
        return Err(fail(format!("no proxy {name:?}; known proxies: {}", names(&list))));
    };
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(fail)?;
    let result = rt.block_on(ProxyBoard::open(home.path()).fixed(p, Duration::from_secs(10)));
    report(name, result.is_ok(), result.err().as_deref())
}

fn report(name: &str, reachable: bool, reason: Option<&str>) -> Result<ExitCode, ExitCode> {
    if reachable {
        println!("{name} reachable; traffic resumed");
        Ok(ExitCode::SUCCESS)
    } else {
        Err(fail(format!("{name} still unreachable: {}", reason.unwrap_or("no answer"))))
    }
}
