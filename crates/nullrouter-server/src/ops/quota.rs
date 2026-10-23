//! The quota operations the CLI and the dashboard share (`quota interval`, `poll`, `ack`). Nothing
//! here prints or exits. `interval_with` and `ack_with` write the files (and the running server
//! reloads). `poll_with` asks the running server over the operator socket, which is the CLI's
//! path; `poll` runs the poll in-process as a task for the dashboard. The history files
//! (`history`, `prune`, `forget`) and the outside-use views stay in the CLI (spec 014 FR-001).

use std::time::Duration;

use nullrouter_engine::accounts::{self, Account, Accounts};
use nullrouter_engine::clock;
use nullrouter_engine::quota::fit::outside::{self, AckTarget};
use nullrouter_engine::quota::poll::reported;
use nullrouter_engine::tasks::TaskKind;
use nullrouter_registry::schema::parse_duration;
use serde_json::{Value, json};

use crate::operator::{self, CallError};

use super::accounts::{Report, fail};
use super::{Ctx, Done, OpError, Status, apply};

/// `10 min`, `1 h`, `90 s`.
pub fn every_text(secs: u64) -> String {
    if secs != 0 && secs.is_multiple_of(3600) {
        format!("{} h", secs / 3600)
    } else if secs != 0 && secs.is_multiple_of(60) {
        format!("{} min", secs / 60)
    } else {
        format!("{secs} s")
    }
}

/// The server's answer; `Ok(None)` when no server runs.
fn ask(ctx: &Ctx, req: &Value) -> Result<Option<Value>, OpError> {
    match operator::call(&ctx.home, req) {
        Ok(a) if a["ok"] == true => Ok(Some(a)),
        Ok(a) => Err(fail(a["error"].as_str().unwrap_or("the server refused the request"))),
        Err(CallError::NoServer(_)) => Ok(None),
        Err(e) => Err(fail(e)),
    }
}

/// Whether `a` is one of the accounts `provider` and `account` (when given) narrow to.
pub fn narrows(a: &Account, provider: Option<&str>, account: Option<&str>) -> bool {
    provider.is_none_or(|p| a.provider == p) && account.is_none_or(|n| a.name == n)
}

/// Sets an account's polling interval: a duration (`15m`, `1h`) or `default`. `Done.text` is the
/// line the CLI prints (a floor raise is said in it); `json` is the CLI's `--json` object.
pub fn interval_with(
    ctx: &Ctx,
    log: &mut Vec<String>,
    provider: &str,
    name: &str,
    every: &str,
) -> Result<Report, OpError> {
    let wanted: Option<Duration> = match every {
        "default" => None,
        s => Some(parse_duration(s).map_err(|e| fail(format!("{s:?}: {e}")))?),
    };
    let mut list = Accounts::load(&ctx.home.path().join(accounts::FILE)).map_err(fail)?;
    let now = list.set_poll_interval(provider, name, wanted).map_err(fail)?;
    list.save().map_err(fail)?;
    let status = match apply(ctx) {
        Ok(a) => {
            for n in &a.notes {
                log.push(format!("note: {n}"));
            }
            a.text
        }
        Err(reason) => return Err(fail(reason)),
    };
    let raised = wanted.is_some_and(|w| w < now);
    let note = if raised { " (raised to the floor)" } else { "" };
    let text = format!("{provider}/{name}: polls every {}{note}; {status}", every_text(now.as_secs()));
    let json = json!({"provider": provider, "name": name, "interval_s": now.as_secs(), "status": status});
    Ok(Report { done: Done { status: Status::Applied, text, once: None }, json, status })
}

/// Polls one account in the running server over the operator socket, then reads the accounts
/// back (the CLI's `quota poll`). `Report.json` is the accounts array the CLI prints. No server
/// is exit 4, with the socket's path in the text.
pub fn poll_with(ctx: &Ctx, provider: &str, name: &str) -> Result<Report, OpError> {
    let req = json!({"op": "quota.poll", "provider": provider, "name": name});
    if ask(ctx, &req)?.is_none() {
        return Err(OpError {
            text: format!(
                "no server is running on {}; polls run in the server (start it with `nullrouter serve`)",
                operator::socket_path(&ctx.home).display()
            ),
            exit: 4,
            field: None,
        });
    }
    let list = ask(ctx, &json!({"op": "quota.list", "provider": provider, "name": name}))?.unwrap_or_default();
    let json = list["accounts"].clone();
    Ok(Report { done: Done { status: Status::Applied, text: String::new(), once: None }, json, status: "" })
}

/// Polls one account in-process for the dashboard, as an engine task (spec 014 R4). Returns at
/// once with the task's id; the poll runs on its own thread and ends the task with its outcome.
pub fn poll(ctx: &Ctx, provider: &str, name: &str) -> Result<Done, OpError> {
    let Some(engine) = ctx.engine.clone() else {
        return Err(OpError { text: "the dashboard's poll needs the server".into(), exit: 4, field: None });
    };
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return Err(OpError { text: "no server is running".into(), exit: 4, field: None });
    };
    {
        let st = engine.snapshot();
        let Some(account) = st.accounts.get(provider, name) else {
            return Err(fail(format!("provider {provider} has no account named {name}")));
        };
        if !st.registry.provider(provider).is_ok_and(|p| reported(p, account).is_some()) {
            return Err(fail(format!("{provider}/{name}: quota not reported")));
        }
    }
    let task = engine.tasks.start(TaskKind::QuotaPoll, format!("{provider}/{name}")).map_err(fail)?;
    let id = task.id().clone();
    let (provider_owned, name_owned) = (provider.to_owned(), name.to_owned());
    let spawned = std::thread::Builder::new().name(format!("quota-poll-{id}")).spawn(move || {
        match runtime.block_on(engine.poll_quota(&provider_owned, &name_owned)) {
            Some(p) => match p.error {
                Some(e) => task.fail(e.summary()),
                None => task.done(format!("{provider_owned}/{name_owned}: polled")),
            },
            None => task.fail(format!("{provider_owned}/{name_owned}: quota not reported")),
        }
    });
    spawned.map_err(|e| fail(format!("could not start the poll: {e}")))?;
    Ok(Done { status: Status::Started { task: id }, text: "started".to_owned(), once: None })
}

/// Acknowledges the open usage alerts `id` names (a full id or a unique prefix, or `all`), in the
/// accounts `provider` and `account` narrow. `Done.text` is the CLI's line, `json` its object.
pub fn ack_with(ctx: &Ctx, id: &str, provider: Option<&str>, account: Option<&str>) -> Result<Report, OpError> {
    // `all` names every open alert: the server reads a missing `id` that way.
    let prefix = (id != "all").then_some(id);
    let req = json!({"op": "quota.ack", "id": prefix, "provider": provider, "account": account});
    let acknowledged = match ask(ctx, &req)? {
        Some(answer) => answer["acknowledged"].as_u64().unwrap_or(0) as usize,
        None => {
            // No server: the files are all there is.
            let target = prefix.map_or(AckTarget::All, AckTarget::Id);
            let list = Accounts::load(&ctx.home.path().join(accounts::FILE)).map_err(fail)?;
            let now = clock::now();
            let mut n = 0;
            for a in list.iter().filter(|a| narrows(a, provider, account)) {
                n += outside::ack(ctx.home.path(), &a.provider, &a.name, target, now).map_err(fail)?.len();
            }
            n
        }
    };
    let text = format!("acknowledged {acknowledged}");
    let json = json!({"acknowledged": acknowledged});
    Ok(Report { done: Done { status: Status::Applied, text, once: None }, json, status: "" })
}
