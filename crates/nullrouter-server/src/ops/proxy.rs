//! The proxy operations the CLI and the dashboard share (`proxy add|remove|use|clear|fixed`).
//! Nothing here prints or exits. The CLI reads the password from stdin and prints `Done.text`;
//! the dashboard hands the password in as a `SecretString`. `fixed_with` is the CLI's path (the
//! running server over the operator socket, else a probe here); `fixed` runs the probe in-process
//! as an engine task for the dashboard.

use std::time::Duration;

use nullrouter_engine::accounts::{self, Accounts};
use nullrouter_engine::connection::pause::ProxyBoard;
use nullrouter_engine::connection::proxy::{self, PasswordSource, Proxies, Proxy};
use nullrouter_engine::tasks::TaskKind;
use nullrouter_registry::schema::OperatorConfig;
use nullrouter_registry::{OperatorHome, SecretString};
use serde_json::json;

use crate::operator::{self, CallError};

use super::accounts::fail;
use super::{Ctx, Done, OpError, Status, apply};

/// Where an assignment is made: every provider, one provider, or one account (`provider/name`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    All,
    Provider(String),
    Account(String),
}

/// Builds a [`Target`] from the dashboard's `scope` (`all`, `provider`, `account`) and `target`.
pub fn target(scope: &str, target: &str) -> Result<Target, OpError> {
    let need = || {
        if target.trim().is_empty() {
            Err(fail_in("target", format!("scope {scope} needs a target")))
        } else {
            Ok(target.trim().to_owned())
        }
    };
    match scope {
        "all" => Ok(Target::All),
        "provider" => need().map(Target::Provider),
        "account" => need().map(Target::Account),
        other => Err(fail_in("scope", format!("scope {other:?} is not all, provider or account"))),
    }
}

fn fail_in(field: &'static str, e: impl std::fmt::Display) -> OpError {
    OpError { text: e.to_string(), exit: 1, field: Some(field) }
}

fn done(text: String) -> Done {
    Done { status: Status::Applied, text, once: None }
}

/// Reloads, queuing the reload's notes in `log`; `Err` when the running server kept its state.
fn applied(ctx: &Ctx, log: &mut Vec<String>) -> Result<&'static str, OpError> {
    let a = apply(ctx).map_err(fail)?;
    log.extend(a.notes.iter().map(|n| format!("note: {n}")));
    Ok(a.text)
}

fn names(list: &Proxies) -> String {
    let all: Vec<&str> = list.iter().map(|p| p.name.as_str()).collect();
    if all.is_empty() { "(none defined)".to_owned() } else { all.join(", ") }
}

/// Defines a proxy. `read_password` is called only when `username` is set without `password_env`
/// (the CLI reads stdin there). `Done.text` is the CLI's line.
pub fn add_with(
    ctx: &Ctx,
    log: &mut Vec<String>,
    name: &str,
    url: &str,
    username: Option<&str>,
    password_env: Option<&str>,
    read_password: impl FnOnce() -> Result<SecretString, OpError>,
) -> Result<Done, OpError> {
    let file = ctx.home.path().join(proxy::FILE);
    let mut list = Proxies::load(&file).map_err(fail)?;
    let (password, secret) = match (username, password_env) {
        (_, Some(var)) => {
            let value = std::env::var(var).ok().filter(|v| !v.is_empty());
            (Some(PasswordSource::Env(var.to_owned())), value.map(SecretString::new))
        }
        (Some(_), None) => (Some(PasswordSource::Literal), Some(read_password()?)),
        (None, None) => (None, None),
    };
    let added =
        Proxy { name: name.to_owned(), url: url.to_owned(), username: username.map(str::to_owned), password, secret };
    list.add(added).map_err(|e| fail_in(if e.starts_with("url:") { "url" } else { "name" }, e))?;
    list.save(&file).map_err(fail)?;
    Ok(done(format!("proxy {name} added: {}", applied(ctx, log)?)))
}

/// The dashboard's `proxy add`: a literal password for a proxy with a user.
pub fn add(
    ctx: &Ctx,
    log: &mut Vec<String>,
    name: &str,
    url: &str,
    username: Option<&str>,
    password: Option<SecretString>,
) -> Result<Done, OpError> {
    add_with(ctx, log, name, url, username, None, || password.ok_or_else(|| fail_in("password", "no password given")))
}

/// Removes a proxy that nothing uses.
pub fn remove(ctx: &Ctx, log: &mut Vec<String>, name: &str) -> Result<Done, OpError> {
    let file = ctx.home.path().join(proxy::FILE);
    let mut list = Proxies::load(&file).map_err(fail)?;
    if list.get(name).is_none() {
        return Err(fail_in("name", format!("no proxy {name:?}; known proxies: {}", names(&list))));
    }
    let used = used_by(&ctx.home, name).map_err(fail)?;
    if !used.is_empty() {
        return Err(fail_in(
            "name",
            format!("proxy {name} is in use by {}; run `nullrouter proxy clear` for each first", used.join(", ")),
        ));
    }
    list.remove(name);
    list.save(&file).map_err(fail)?;
    Ok(done(format!("proxy {name} removed: {}", applied(ctx, log)?)))
}

/// Sends `target`'s traffic through `proxy` (a name, or `none`).
pub fn use_proxy(ctx: &Ctx, log: &mut Vec<String>, proxy_name: &str, target: &Target) -> Result<Done, OpError> {
    if proxy_name != proxy::NONE {
        let list = Proxies::load(&ctx.home.path().join(proxy::FILE)).map_err(fail)?;
        if list.get(proxy_name).is_none() {
            return Err(fail_in("proxy", format!("no proxy {proxy_name:?}; known proxies: {}, or none", names(&list))));
        }
    }
    assign(ctx, log, target, Some(proxy_name)).map(done)
}

/// Removes the assignment at `target`, so the level above applies again.
pub fn clear(ctx: &Ctx, log: &mut Vec<String>, target: &Target) -> Result<Done, OpError> {
    assign(ctx, log, target, None).map(done)
}

/// Where `name` is assigned: `all providers`, `provider X`, `account X/Y`.
pub fn used_by(home: &OperatorHome, name: &str) -> Result<Vec<String>, String> {
    let handle = crate::views::open_registry(home).map_err(|e| e.message)?;
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

/// Sets (`Some`) or clears (`None`) the assignment at `target`, then reloads.
fn assign(ctx: &Ctx, log: &mut Vec<String>, target: &Target, value: Option<&str>) -> Result<String, OpError> {
    let home = &ctx.home;
    let tgt = |e: String| fail_in("target", e);
    if let Target::Account(spec) = target {
        let (provider, name) =
            spec.split_once('/').ok_or_else(|| tgt("--account takes provider/name, such as kiro/main".to_owned()))?;
        let path = home.path().join(accounts::FILE);
        let mut list = Accounts::load(&path).map_err(fail)?;
        if list.get(provider, name).is_none() {
            let known: Vec<String> = list.iter().map(|a| format!("{}/{}", a.provider, a.name)).collect();
            return Err(tgt(format!("no account {spec:?}; known accounts: {}", known.join(", "))));
        }
        list.set_proxy(provider, name, value.map(str::to_owned)).map_err(fail)?;
        list.save().map_err(fail)?;
        return Ok(format!("account {spec}: {}: {}", said(value), applied(ctx, log)?));
    }
    let (header, what) = match target {
        Target::Provider(p) => {
            let handle = crate::views::open_registry(home).map_err(|e| fail(e.message))?;
            let snapshot = handle.snapshot();
            let known: Vec<&str> = snapshot.providers().map(|p| p.id.as_str()).collect();
            if !known.contains(&p.as_str()) {
                return Err(tgt(format!("no provider {p:?}; known providers: {}", known.join(", "))));
            }
            (format!("provider.{p}.connection"), format!("provider {p}"))
        }
        _ => ("connection".to_owned(), "all providers".to_owned()),
    };
    let path = home.config_file();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(fail(format!("{}: {e}", path.display()))),
    };
    let quoted = value.map(|v| format!("\"{v}\""));
    let (edited, changed) = super::config_text::edit(&text, &header, "proxy", quoted.as_deref());
    if !changed {
        return Ok(format!("{what}: no proxy was assigned"));
    }
    toml::from_str::<OperatorConfig>(&edited)
        .map_err(|e| fail(format!("{}: the edit would not load: {e}", path.display())))?;
    std::fs::create_dir_all(home.path()).map_err(fail)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, &edited)
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|e| fail(format!("{}: {e}", path.display())))?;
    Ok(format!("{what}: {}: {}", said(value), applied(ctx, log)?))
}

fn said(value: Option<&str>) -> String {
    value.map_or_else(|| "assignment removed".to_owned(), |v| format!("proxy {v}"))
}

/// The line `proxy fixed` says: `Ok` when the proxy answered, `Err` text when it did not.
fn report(name: &str, reachable: bool, reason: Option<&str>) -> Result<Done, OpError> {
    if reachable {
        Ok(done(format!("{name} reachable; traffic resumed")))
    } else {
        Err(fail(format!("{name} still unreachable: {}", reason.unwrap_or("no answer"))))
    }
}

/// `proxy fixed` as the CLI runs it: the running server probes over the operator socket; with
/// none running the probe is made here and the saved pause cleared, which the next `serve` reads.
pub fn fixed_with(ctx: &Ctx, name: &str) -> Result<Done, OpError> {
    let home = &ctx.home;
    match operator::call(home, &json!({"op": "proxy.fixed", "name": name})) {
        Ok(a) if a["ok"] == true => return report(name, a["reachable"] == true, a["reason"].as_str()),
        Ok(a) => return Err(fail(a["error"].as_str().unwrap_or("the server refused the request"))),
        Err(CallError::NoServer(_)) => {}
        Err(e) => return Err(fail(e)),
    }
    let list = Proxies::load(&home.path().join(proxy::FILE)).map_err(fail)?;
    let Some(p) = list.get(name) else {
        return Err(fail_in("name", format!("no proxy {name:?}; known proxies: {}", names(&list))));
    };
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(fail)?;
    let result = rt.block_on(ProxyBoard::open(home.path()).fixed(p, Duration::from_secs(10)));
    report(name, result.is_ok(), result.err().as_deref())
}

/// `proxy fixed` for the dashboard, as an engine task: returns at once with the task's id; the
/// probe runs on its own thread and ends the task with the CLI's line.
pub fn fixed(ctx: &Ctx, name: &str) -> Result<Done, OpError> {
    let Some(engine) = ctx.engine.clone() else {
        return Err(OpError { text: "the dashboard's probe needs the server".into(), exit: 4, field: None });
    };
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return Err(OpError { text: "no server is running".into(), exit: 4, field: None });
    };
    let probe = {
        let st = engine.snapshot();
        match st.clients.proxies().get(name) {
            Some(p) => p.clone(),
            None => {
                let known: Vec<&str> = st.clients.proxies().iter().map(|p| p.name.as_str()).collect();
                let known = if known.is_empty() { "(none defined)".to_owned() } else { known.join(", ") };
                return Err(fail_in("name", format!("no proxy {name:?}; known proxies: {known}")));
            }
        }
    };
    let task = engine.tasks.start(TaskKind::ProxyFixed, name.to_owned()).map_err(fail)?;
    let id = task.id().clone();
    let name_owned = name.to_owned();
    let spawned = std::thread::Builder::new().name(format!("proxy-fixed-{id}")).spawn(move || {
        match runtime.block_on(engine.proxy_board.fixed(&probe, Duration::from_secs(10))) {
            Ok(()) => task.done(format!("{name_owned} reachable; traffic resumed")),
            Err(reason) => task.fail(format!("{name_owned} still unreachable: {reason}")),
        }
    });
    spawned.map_err(|e| fail(format!("could not start the probe: {e}")))?;
    Ok(Done { status: Status::Started { task: id }, text: "started".to_owned(), once: None })
}
