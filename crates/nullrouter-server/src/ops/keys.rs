//! The client-key operations the CLI and the dashboard share (`keys issue`, `revoke`, `tag`,
//! `set-break`, `set-adapter`). Nothing here prints or exits; `issue` returns the new key in
//! `Done.once`, and what the CLI prints on stderr comes back in `log`.

use nullrouter_engine::keys::{self, BreakBehaviour, HarnessName, KeyError, Keys};
use nullrouter_registry::SecretString;
use serde_json::json;

use super::accounts::{Report, fail, refused};
use super::{Ctx, Done, Once, OpError, Status, apply};

fn fail_in(field: &'static str, e: impl std::fmt::Display) -> OpError {
    OpError { text: e.to_string(), exit: 1, field: Some(field) }
}

fn load(ctx: &Ctx) -> Result<Keys, OpError> {
    Keys::load(&ctx.home.path().join(keys::FILE)).map_err(fail)
}

fn harness_name(s: &str) -> Result<HarnessName, OpError> {
    HarnessName::new(s).map_err(|e| fail_in("harness", e))
}

fn behaviour(s: &str) -> Result<BreakBehaviour, OpError> {
    BreakBehaviour::parse(s)
        .ok_or_else(|| fail_in("behaviour", format!("unknown break behaviour {s:?}; allowed: restart, error_event")))
}

/// Saves and reloads. `Err` is the report to return as it stands (a refused reload); `Ok` is the
/// status word.
fn save_and_apply(ctx: &Ctx, log: &mut Vec<String>, list: &Keys) -> Result<Result<&'static str, Report>, OpError> {
    list.save().map_err(fail)?;
    match apply(ctx) {
        Ok(a) => {
            for n in &a.notes {
                log.push(format!("note: {n}"));
            }
            Ok(Ok(a.text))
        }
        Err(reason) => Ok(Err(refused(reason))),
    }
}

fn done(text: String) -> Done {
    Done { status: Status::Applied, text, once: None }
}

/// Issues a key. `Done.text` is the line the CLI prints on stderr; the key is in `once`.
pub fn issue_with(
    ctx: &Ctx,
    log: &mut Vec<String>,
    name: &str,
    break_behaviour: Option<&str>,
    harness: Option<&str>,
    adapter: Option<&str>,
) -> Result<Report, OpError> {
    let mut list = load(ctx)?;
    let b = break_behaviour.map(behaviour).transpose()?;
    // Checked before anything is issued, so a refused tag or adapter writes nothing.
    let tag = harness.map(keys::check_harness).transpose().map_err(|e| fail_in("harness", e))?;
    let h = adapter.map(harness_name).transpose()?;
    let (key, rec) = list.issue(name, b).map_err(|e| fail_in("name", e))?;
    let (id, name) = (rec.id.clone(), rec.name.clone());
    if tag.is_some() {
        list.set_harness(&id, tag.as_deref()).map_err(fail)?;
    }
    if h.is_some() {
        list.set_adapter(&id, h.clone()).map_err(fail)?;
    }
    let status = match save_and_apply(ctx, log, &list)? {
        Ok(s) => s,
        Err(r) => return Ok(r),
    };
    let adapter = h.as_ref().map(HarnessName::as_str);
    let json = json!({"id": id, "name": name, "harness": tag, "adapter": adapter, "key": key, "status": status});
    let text = format!("issued {id} ({name}), {status}; it is shown once, only its digest is stored:");
    let once = Once { label: "key".to_owned(), value: SecretString::new(key) };
    Ok(Report { done: Done { status: Status::Applied, text, once: Some(once) }, json, status })
}

pub fn issue(
    ctx: &Ctx,
    name: &str,
    break_behaviour: Option<&str>,
    harness: Option<&str>,
    adapter: Option<&str>,
) -> Result<Done, OpError> {
    issue_with(ctx, &mut Vec::new(), name, break_behaviour, harness, adapter).map(|r| r.done)
}

/// The sentence the CLI's `[y/N]` prompt and the dashboard's confirm window show for a revoke.
#[must_use]
pub fn revoke_question(key: &str) -> String {
    super::describe_loss("keys.revoke", key).unwrap_or_default()
}

pub fn revoke_with(ctx: &Ctx, log: &mut Vec<String>, key: &str) -> Result<Report, OpError> {
    let mut list = load(ctx)?;
    list.revoke(key).map_err(|e| fail_in("key", e))?;
    simple_tail(ctx, log, &list, key)
}

pub fn revoke(ctx: &Ctx, key: &str) -> Result<Done, OpError> {
    revoke_with(ctx, &mut Vec::new(), key).map(|r| r.done)
}

fn simple_tail(ctx: &Ctx, log: &mut Vec<String>, list: &Keys, key: &str) -> Result<Report, OpError> {
    let status = match save_and_apply(ctx, log, list)? {
        Ok(s) => s,
        Err(r) => return Ok(r),
    };
    Ok(Report { done: done(format!("{key}: {status}")), json: json!({"key": key, "status": status}), status })
}

/// Sets, replaces (`text`) or removes (`clear`) the harness tag of a key.
pub fn tag_with(
    ctx: &Ctx,
    log: &mut Vec<String>,
    key: &str,
    text: Option<&str>,
    clear: bool,
) -> Result<Report, OpError> {
    let mut list = load(ctx)?;
    let text = match (text, clear) {
        (Some(t), false) => Some(t),
        (None, true) => None,
        _ => return Err(fail_in("text", "give a TEXT or --clear")),
    };
    list.set_harness(key, text).map_err(|e| match e {
        KeyError::NotFound(_) => OpError { text: format!("no key {key:?}"), exit: 2, field: Some("key") },
        e => fail_in("text", e),
    })?;
    let rec = list.iter().find(|k| k.id == key || k.name == key).expect("just tagged");
    let (id, name, tag) = (rec.id.clone(), rec.name.clone(), rec.harness.clone());
    let status = match save_and_apply(ctx, log, &list)? {
        Ok(s) => s,
        Err(r) => return Ok(r),
    };
    let line = match &tag {
        Some(t) => format!("{name}: harness {t}"),
        None => format!("{name}: no harness"),
    };
    Ok(Report {
        done: done(format!("{line}\n{status}")),
        json: json!({"id": id, "name": name, "harness": tag, "status": status}),
        status,
    })
}

pub fn tag(ctx: &Ctx, key: &str, text: Option<&str>, clear: bool) -> Result<Done, OpError> {
    tag_with(ctx, &mut Vec::new(), key, text, clear).map(|r| r.done)
}

/// Sets a key's break behaviour: `restart`, `error_event`, or `default`.
pub fn set_break_with(ctx: &Ctx, log: &mut Vec<String>, key: &str, behaviour_text: &str) -> Result<Report, OpError> {
    let mut list = load(ctx)?;
    let b = if behaviour_text == "default" { None } else { Some(behaviour(behaviour_text)?) };
    list.set_break(key, b).map_err(|e| fail_in("key", e))?;
    simple_tail(ctx, log, &list, key)
}

pub fn set_break(ctx: &Ctx, key: &str, behaviour_text: &str) -> Result<Done, OpError> {
    set_break_with(ctx, &mut Vec::new(), key, behaviour_text).map(|r| r.done)
}

/// Binds a key to a harness's adapter, or unbinds it (`clear`).
pub fn set_adapter_with(
    ctx: &Ctx,
    log: &mut Vec<String>,
    key: &str,
    harness: Option<&str>,
    clear: bool,
) -> Result<Report, OpError> {
    let mut list = load(ctx)?;
    let h = if clear { None } else { harness.map(harness_name).transpose()? };
    list.set_adapter(key, h).map_err(|e| fail_in("key", e))?;
    simple_tail(ctx, log, &list, key)
}

pub fn set_adapter(ctx: &Ctx, key: &str, harness: Option<&str>, clear: bool) -> Result<Done, OpError> {
    set_adapter_with(ctx, &mut Vec::new(), key, harness, clear).map(|r| r.done)
}
