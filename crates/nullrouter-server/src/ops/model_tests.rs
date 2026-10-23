//! The model-test and verdict operations the CLI and the dashboard share (`test`, `verdicts mark`,
//! `clear`, `settings`). Nothing here prints or exits. `plan` names the calls and the question
//! asked before more than one is billed. `run_with` is the one driver: it streams each result to a
//! sink, over the operator socket from the CLI or in-process inside the server. `run` starts it as
//! an engine task for the dashboard, each streamed line becoming a progress line.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use nullrouter_engine::clock;
use nullrouter_engine::state::Engine;
use nullrouter_engine::tasks::TaskKind;
use nullrouter_engine::tests::{self as engine_tests};
use nullrouter_engine::verdict::{self, Mark, Rejection, Source, State};
use nullrouter_registry::OperatorHome;
use nullrouter_registry::schema::{OperatorConfig, TEST_TYPES, parse_broken_retest, parse_duration};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::operator::{self, CallError};

use super::accounts::{Report, fail};
use super::{Ctx, Done, OpError, Status, apply};

const NO_SERVER: &str = "no running server; start it with nullrouter serve";

fn fail_in(field: &'static str, e: impl std::fmt::Display) -> OpError {
    OpError { text: e.to_string(), exit: 1, field: Some(field) }
}

/// What a test would do: the billed calls per model type, and the question to ask first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Billed calls per type, in type-name order, zero counts kept.
    pub calls: Vec<(String, u64)>,
    pub billed: u64,
    /// The combo the target names, if it is one.
    pub combo: Option<String>,
}

impl Plan {
    /// Whether the run must be confirmed first: more than one billed call.
    pub fn needs_confirm(&self) -> bool {
        self.billed > 1
    }

    /// `12 billed calls: 8 text, 2 embeddings. Continue?` (the CLI adds ` [y/N] `).
    pub fn question(&self) -> String {
        let parts: Vec<String> = self.calls.iter().filter(|(_, c)| *c > 0).map(|(k, c)| format!("{c} {k}")).collect();
        format!("{} billed calls: {}. Continue?", self.billed, parts.join(", "))
    }
}

/// The server's answer as a failure, as the CLI says it.
fn answer(got: Result<Value, CallError>) -> Result<Value, OpError> {
    match got {
        Ok(a) if a["ok"] == true => Ok(a),
        Ok(a) => Err(fail(a["error"].as_str().unwrap_or("the server refused the request"))),
        Err(CallError::NoServer(_)) => Err(fail(NO_SERVER)),
        Err(e) => Err(fail(e)),
    }
}

/// The combo `target` names, with its call's type; an `account` with a combo is refused.
fn combo_of(engine: &Engine, target: Option<&str>, account: Option<&str>) -> Result<Option<(String, String)>, OpError> {
    let Some(target) = target else { return Ok(None) };
    let st = engine.snapshot();
    let Some(combo) = st.registry.combo(target) else { return Ok(None) };
    if account.is_some() {
        return Err(fail(format!("{target} is a combo: its test isn't per account")));
    }
    Ok(Some((combo.name.clone(), engine_tests::combo::kind(combo).to_string())))
}

fn planned(
    engine: &Engine,
    target: Option<&str>,
    account: Option<&str>,
) -> Result<Vec<engine_tests::Planned>, OpError> {
    engine_tests::expand(engine, &engine.snapshot(), target, account).map_err(fail)
}

/// Plans a test of `target` (`None` = every pair), narrowed to `account`. Inside the server it
/// reads the engine; otherwise it asks the running server over the operator socket.
pub fn plan(ctx: &Ctx, target: Option<&str>, account: Option<&str>) -> Result<Plan, OpError> {
    let (calls, combo): (BTreeMap<String, u64>, Option<String>) = if let Some(engine) = &ctx.engine {
        if let Some((name, ty)) = combo_of(engine, target, account)? {
            (BTreeMap::from([(ty, 1)]), Some(name))
        } else {
            let p = planned(engine, target, account)?;
            (engine_tests::calls(&p).into_iter().map(|(k, v)| (k, v as u64)).collect(), None)
        }
    } else {
        let req = json!({"op": "test.plan", "target": target, "account": account, "all": target.is_none()});
        let a = answer(operator::call(&ctx.home, &req))?;
        let calls = a["calls"]
            .as_object()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.as_u64().unwrap_or(0))).collect())
            .unwrap_or_default();
        (calls, a["combo"].as_str().map(str::to_owned))
    };
    let billed = calls.values().sum();
    Ok(Plan { calls: calls.into_iter().collect(), billed, combo })
}

/// One streamed event of a run.
pub enum Event<'a> {
    Result(&'a Value),
    Combo(&'a Value),
}

/// Runs the test and feeds each result to `sink`; returns the closing `done` counts. Without an
/// engine it streams from the running server over the operator socket (the CLI). With one it runs
/// the calls here and must not be called on a runtime thread: it blocks on the runtime.
pub fn run_with(
    ctx: &Ctx,
    target: Option<&str>,
    account: Option<&str>,
    mut sink: impl FnMut(Event<'_>),
) -> Result<Value, OpError> {
    let Some(engine) = &ctx.engine else {
        let req = json!({"op": "test.run", "target": target, "account": account, "all": target.is_none()});
        let done = answer(operator::call_stream(&ctx.home, &req, |line| {
            let r = &line["result"];
            if line["event"] == "combo" {
                sink(Event::Combo(r));
            } else {
                sink(Event::Result(r));
            }
        }))?;
        return Ok(done["done"].clone());
    };
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return Err(OpError { text: "no server is running".into(), exit: 4, field: None });
    };
    runtime.block_on(run_local(engine, target, account, &mut sink))
}

async fn run_local(
    engine: &Arc<Engine>,
    target: Option<&str>,
    account: Option<&str>,
    sink: &mut impl FnMut(Event<'_>),
) -> Result<Value, OpError> {
    let stop = CancellationToken::new();
    if let Some((name, _)) = combo_of(engine, target, account)? {
        let run = engine_tests::run_id();
        let r = match engine_tests::combo::run_combo(engine, &name, Source::Test, &run, &stop).await {
            Ok(Some(r)) => r,
            Ok(None) => return Err(fail("the combo test was cancelled")),
            Err(e) => return Err(fail(e)),
        };
        let mut done = json!({"pass": 0, "broken": 0, "unknown": 0, "skipped": 0});
        done[r.state.as_str()] = json!(1);
        let r = serde_json::to_value(&r).map_err(fail)?;
        sink(Event::Combo(&r));
        return Ok(done);
    }
    let planned = planned(engine, target, account)?;
    let (tx, mut rx) = mpsc::channel(16);
    tokio::spawn({
        let (engine, stop, run) = (engine.clone(), stop.clone(), engine_tests::run_id());
        async move { engine_tests::run(&engine, planned, Source::Test, &run, stop, tx).await }
    });
    let (mut pass, mut broken, mut unknown, mut skipped) = (0, 0, 0, 0);
    while let Some(r) = rx.recv().await {
        match r.state {
            Some(State::Pass) => pass += 1,
            Some(State::Broken) => broken += 1,
            Some(State::Unknown) => unknown += 1,
            None => skipped += 1,
        }
        let v = serde_json::to_value(&r).map_err(fail)?;
        sink(Event::Result(&v));
    }
    Ok(json!({"pass": pass, "broken": broken, "unknown": unknown, "skipped": skipped}))
}

/// `3 pairs: 2 pass, 1 broken, 0 unknown, 0 skipped`, the run's closing line.
pub fn summary_line(pairs: usize, done: &Value) -> String {
    let count = |k: &str| done[k].as_u64().unwrap_or(0);
    format!(
        "{pairs} pairs: {} pass, {} broken, {} unknown, {} skipped",
        count("pass"),
        count("broken"),
        count("unknown"),
        count("skipped")
    )
}

/// Starts a test for the dashboard as an engine task; returns at once with its id. More than one
/// billed call is refused with the question text (field `confirmed`) until `confirmed`. Each
/// result line is a progress line; the task ends with the closing line.
pub fn run(ctx: &Ctx, target: Option<&str>, account: Option<&str>, confirmed: bool) -> Result<Done, OpError> {
    let Some(engine) = ctx.engine.clone() else {
        return Err(OpError { text: "the dashboard's test needs the server".into(), exit: 4, field: None });
    };
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return Err(OpError { text: "no server is running".into(), exit: 4, field: None });
    };
    let plan = plan(ctx, target, account)?;
    if plan.needs_confirm() && !confirmed {
        return Err(fail_in("confirmed", plan.question()));
    }
    let subject = match (&plan.combo, target) {
        (Some(name), _) => format!("combo:{name}"),
        (None, Some(t)) => t.to_owned(),
        (None, None) => "all".to_owned(),
    };
    let task = engine.tasks.start(TaskKind::Test, subject).map_err(fail)?;
    let id = task.id().clone();
    let (target, account) = (target.map(str::to_owned), account.map(str::to_owned));
    let ctx = Ctx { home: ctx.home.clone(), engine: Some(engine) };
    let spawned = std::thread::Builder::new().name(format!("model-test-{id}")).spawn(move || {
        let _enter = runtime.enter();
        let (mut pairs, mut head) = (0usize, None);
        let ended = run_with(&ctx, target.as_deref(), account.as_deref(), |e| match e {
            Event::Result(r) => {
                pairs += 1;
                task.progress(result_line(r, SystemTime::now()));
            }
            Event::Combo(c) => {
                for (i, l) in combo_lines(c, SystemTime::now()).into_iter().enumerate() {
                    if i == 0 {
                        head = Some(l.clone());
                    }
                    task.progress(l);
                }
            }
        });
        match ended {
            Ok(done) => task.done(head.unwrap_or_else(|| summary_line(pairs, &done))),
            Err(e) => task.fail(e.text),
        }
    });
    spawned.map_err(|e| fail(format!("could not start the test: {e}")))?;
    Ok(Done { status: Status::Started { task: id }, text: "started".to_owned(), once: None })
}

/// One result as its output line (contracts/cli.md).
pub fn result_line(r: &Value, now: SystemTime) -> String {
    let s = |k: &str| r[k].as_str().unwrap_or_default();
    let who = format!("{}/{}", s("provider"), s("account"));
    let (label, text) = match (r["skipped"].as_str(), s("state")) {
        (Some(why), _) => ("SKIPPED", why.to_owned()),
        (None, "pass") => ("PASS", timing(r)),
        (None, "broken") => {
            let what = Rejection::parse(s("rejection")).map_or("rejected", |x| x.describe());
            ("BROKEN", format!("{what}: {}", s("reason")))
        }
        (None, _) => {
            let retest = r["next"].as_str().and_then(clock::parse_rfc3339).map(|t| {
                let left = t.duration_since(now).unwrap_or_default();
                format!("; retest in {}", short(left))
            });
            ("UNKNOWN", format!("{}{}", s("reason"), retest.unwrap_or_default()))
        }
    };
    format!("{label:<8} {who:<20} {:<26} {text}", s("model"))
}

/// A combo result as its output tree (contracts/cli.md § Combo output). An UNKNOWN attempt
/// changes no verdict, so it reads `(not saved)`.
pub fn combo_lines(r: &Value, now: SystemTime) -> Vec<String> {
    let s = |k: &str| r[k].as_str().unwrap_or_default();
    let mut head = format!("combo {}: {}", s("combo"), label(r));
    match r["answered_by"].as_str() {
        Some(by) => head += &format!(", answered by {by}"),
        None if !s("reason").is_empty() => head += &format!(": {}", s("reason")),
        None => {}
    }
    if let Some(next) = r["next"].as_str().and_then(clock::parse_rfc3339) {
        head += &format!("; retest in {}", short(next.duration_since(now).unwrap_or_default()));
    }
    let mut out = vec![head];
    tree(&r["tried"], 1, &mut out);
    out
}

fn label(v: &Value) -> &'static str {
    v["state"].as_str().and_then(State::parse).map_or("SKIPPED", State::label)
}

fn tree(tried: &Value, depth: usize, out: &mut Vec<String>) {
    let pad = "  ".repeat(depth);
    for t in tried.as_array().into_iter().flatten() {
        let name = format!("{pad}{}", t["member"].as_str().unwrap_or_default());
        let attempts = t["attempts"].as_array().map_or(&[][..], Vec::as_slice);
        let every_skip = !attempts.is_empty() && attempts.iter().all(|a| a["skipped"].is_string());
        let text = match attempts {
            _ if t["kind"] == "combo" => String::new(),
            _ if every_skip && t["state"] == "broken" => t["reason"].as_str().unwrap_or_default().to_owned(),
            [one] => attempt_text(one),
            _ => String::new(),
        };
        out.push(format!("{name:<18} {:<8} {text}", label(t)).trim_end().to_owned());
        if t["kind"] == "combo" {
            tree(&t["tried"], depth + 1, out);
        } else if attempts.len() > 1 && text.is_empty() {
            for a in attempts {
                out.push(format!("{pad}  {:<16} {:<8} {}", "", label(a), attempt_text(a)).trim_end().to_owned());
            }
        }
    }
}

/// `openrouter/main 503 upstream overloaded (not saved)`, `opencode-go/main 2.1 s`.
fn attempt_text(a: &Value) -> String {
    let s = |k: &str| a[k].as_str().unwrap_or_default();
    let who = format!("{}/{}", s("provider"), s("account"));
    match (a["skipped"].as_str(), s("state")) {
        (Some(why), _) => format!("{who} skipped: {why}"),
        (None, "pass") => format!("{who} {}", timing(a)),
        (None, "broken") => {
            let what = Rejection::parse(s("rejection")).map_or("rejected", |x| x.describe());
            format!("{who} {what}: {}", s("reason"))
        }
        (None, _) => format!("{who} {} (not saved)", s("reason")),
    }
}

fn timing(r: &Value) -> String {
    let secs = |ms: &Value| ms.as_f64().unwrap_or(0.0) / 1000.0;
    match r.get("ttft_ms") {
        Some(t) if !t.is_null() => format!("{:.1} s (first output {:.1} s)", secs(&r["ms"]), secs(t)),
        _ => format!("{:.1} s", secs(&r["ms"])),
    }
}

/// `40 s`, `5 min`, `6 h`.
fn short(d: Duration) -> String {
    match d.as_secs() {
        s if s < 60 => format!("{s} s"),
        s if s < 3600 => format!("{} min", (s + 30) / 60),
        s => format!("{} h", (s + 1800) / 3600),
    }
}

/// Marks `provider/account model` BROKEN by the operator, or clears it; the report is the CLI's
/// line and `--json` object. Inside the server the engine writes it; otherwise the running server
/// is asked, and with none the engine is opened on the home here, so the verdict carries the
/// account's basis as a server would have written it.
fn set_verdict(ctx: &Ctx, provider: &str, account: &str, model: &str, mark: Mark) -> Result<Report, OpError> {
    let (state, note) = match &mark {
        Mark::Broken { note } => ("broken", note.clone()),
        Mark::Clear => ("clear", None),
    };
    let status = if let Some(engine) = &ctx.engine {
        verdict::mark(engine, provider, account, model, mark).map_err(fail)?;
        "applied"
    } else {
        let req = json!({"op": "verdicts.set", "provider": provider, "account": account, "model": model,
                         "state": state, "note": note});
        match operator::call(&ctx.home, &req) {
            Ok(a) if a["ok"] == true => "applied",
            Ok(a) => return Err(fail(a["error"].as_str().unwrap_or("the server refused the request"))),
            Err(CallError::NoServer(_)) => {
                let (engine, _) = Engine::open(ctx.home.clone()).map_err(|e| fail(format!("startup failed:\n{e}")))?;
                let done = verdict::mark(&engine, provider, account, model, mark);
                engine.journal.flush_blocking();
                done.map_err(fail)?;
                "saved; applies at next start"
            }
            Err(e) => return Err(fail(e)),
        }
    };
    let text = if state == "clear" {
        format!("{provider}/{account} {model}: untested: {status}")
    } else {
        format!("{provider}/{account} {model}: BROKEN, set by the operator: {status}")
    };
    let json = json!({"provider": provider, "account": account, "model": model, "state": state, "status": status});
    Ok(Report { done: Done { status: Status::Applied, text, once: None }, json, status })
}

/// `verdicts mark` for the CLI: the note is kept as given.
pub fn mark_with(ctx: &Ctx, provider: &str, account: &str, model: &str, note: Option<&str>) -> Result<Report, OpError> {
    set_verdict(ctx, provider, account, model, Mark::Broken { note: note.map(str::to_owned) })
}

/// `verdicts clear` for the CLI.
pub fn clear_with(ctx: &Ctx, provider: &str, account: &str, model: &str) -> Result<Report, OpError> {
    set_verdict(ctx, provider, account, model, Mark::Clear)
}

/// Marks a model BROKEN by the operator (the dashboard's form); a blank note is none.
pub fn mark(ctx: &Ctx, provider: &str, account: &str, model: &str, note: Option<&str>) -> Result<Done, OpError> {
    let note = note.map(str::trim).filter(|n| !n.is_empty());
    mark_with(ctx, provider, account, model, note).map(|r| r.done)
}

/// Returns a model to untested on one account.
pub fn clear(ctx: &Ctx, provider: &str, account: &str, model: &str) -> Result<Done, OpError> {
    clear_with(ctx, provider, account, model).map(|r| r.done)
}

const SETTINGS: &str = "retest <steps,…|default>, broken-retest <on|off|duration|default>, \
                        timeout <text|embedding|tts|stt|image|video> <duration|default>, concurrency <1-32|default>";

/// The usage error for a `settings` call of the wrong shape.
pub fn settings_usage() -> OpError {
    fail(format!("usage: verdicts settings [{SETTINGS}]"))
}

/// `config.toml` as written, or empty when there is none.
pub fn config_text(home: &OperatorHome) -> Result<String, OpError> {
    let path = home.config_file();
    match std::fs::read_to_string(&path) {
        Ok(t) => Ok(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(fail(format!("{}: {e}", path.display()))),
    }
}

/// Sets one `[tests]` value in `config.toml`: `retest`, `broken-retest`, `concurrency`, or
/// `timeout` with a `type_`; `default` removes it. A value that doesn't parse, or breaks a rule,
/// writes nothing. Reload notes are queued in `log`; `Report.done.text` is the CLI's line.
pub fn settings_with(
    ctx: &Ctx,
    log: &mut Vec<String>,
    setting: &str,
    type_: Option<&str>,
    value: &str,
) -> Result<Report, OpError> {
    let home = &ctx.home;
    let v = value;
    // (table, key, TOML value or None for default, what to print)
    let (table, key, toml, shown) = match (setting, type_) {
        ("retest", None) => {
            let steps: Vec<&str> = v.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
            if v != "default" {
                for s in &steps {
                    parse_duration(s).map_err(|e| fail_in("value", format!("tests.retest: {s}: {e}")))?;
                }
            }
            let quoted: Vec<String> = steps.iter().map(|s| format!("\"{s}\"")).collect();
            ("[tests]", "retest", (v != "default").then(|| format!("[{}]", quoted.join(", "))), v.to_owned())
        }
        ("broken-retest", None) => {
            if v != "default" {
                parse_broken_retest(v).map_err(|rule| fail_in("value", format!("tests.broken_retest: {rule}")))?;
            }
            ("[tests]", "broken_retest", (v != "default").then(|| format!("\"{v}\"")), v.to_owned())
        }
        ("concurrency", None) => {
            if v != "default" {
                v.parse::<u32>().map_err(|_| fail_in("value", "tests.concurrency: 1 to 32"))?;
            }
            ("[tests]", "concurrency", (v != "default").then(|| v.to_owned()), v.to_owned())
        }
        ("timeout", Some(ty)) => {
            if !TEST_TYPES.contains(&ty) {
                return Err(fail_in("type", format!("tests.timeout: the type is one of {}", TEST_TYPES.join(", "))));
            }
            if v != "default" {
                parse_duration(v).map_err(|e| fail_in("value", format!("tests.timeout.{ty}: {e}")))?;
            }
            ("[tests.timeout]", ty, (v != "default").then(|| format!("\"{v}\"")), format!("{ty} {v}"))
        }
        _ => return Err(settings_usage()),
    };
    let path = home.config_file();
    let _lock = super::config_text::lock(home);
    let text = super::routing::edit_key(&config_text(home)?, table, key, toml.as_deref());
    // The file must still load and keep the rules: a config the server would refuse is never written.
    let config: OperatorConfig =
        toml::from_str(&text).map_err(|e| fail(format!("{}: the edit would not load: {e}", path.display())))?;
    let broken = config.tests.check();
    if !broken.is_empty() {
        let lines: Vec<String> = broken.iter().map(|(at, rule)| format!("{at}: {rule}")).collect();
        return Err(fail(lines.join("\n")));
    }
    std::fs::create_dir_all(home.path()).map_err(fail)?;
    super::config_text::write_atomic(&path, text.as_bytes()).map_err(|e| fail(format!("{}: {e}", path.display())))?;
    let applied = apply(ctx).map_err(fail)?;
    log.extend(applied.notes.iter().map(|n| format!("note: {n}")));
    let text = format!("{setting} = {shown}: {}", applied.text);
    let json = json!({"setting": setting, "value": shown, "status": applied.text});
    Ok(Report { done: Done { status: Status::Applied, text, once: None }, json, status: applied.text })
}

/// `verdicts settings` for the dashboard's form.
pub fn settings(ctx: &Ctx, setting: &str, type_: Option<&str>, value: &str) -> Result<Done, OpError> {
    settings_with(ctx, &mut Vec::new(), setting, type_, value).map(|r| r.done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn result_lines() {
        let now = clock::parse_rfc3339("2026-10-07T09:00:00Z").unwrap();
        let pass = json!({"provider": "anthropic", "account": "max", "model": "claude-sonnet-4-5", "state": "pass", "reason": "", "ms": 1800, "ttft_ms": 900});
        assert_eq!(
            result_line(&pass, now),
            "PASS     anthropic/max        claude-sonnet-4-5          1.8 s (first output 0.9 s)"
        );
        let broken = json!({"provider": "anthropic", "account": "max", "model": "claude-opus-4-1", "state": "broken", "rejection": "model_not_available", "reason": "403: model claude-opus-4-1 is not available on your plan", "ms": 300});
        assert!(
            result_line(&broken, now)
                .ends_with("model not available: 403: model claude-opus-4-1 is not available on your plan")
        );
        let unknown = json!({"provider": "openrouter", "account": "main", "model": "x", "state": "unknown", "reason": "503: upstream overloaded", "ms": 300, "next": "2026-10-07T09:01:00Z"});
        assert!(
            result_line(&unknown, now).ends_with("503: upstream overloaded; retest in 1 min"),
            "{}",
            result_line(&unknown, now)
        );
        let skipped = json!({"provider": "xai", "account": "backup", "model": "grok-4", "state": null, "reason": "rate-limited until 09:14 UTC", "skipped": "rate-limited until 09:14 UTC", "ms": 0});
        assert!(result_line(&skipped, now).starts_with("SKIPPED  xai/backup"));
    }

    #[test]
    fn a_combo_prints_as_the_contract_shows() {
        let now = clock::parse_rfc3339("2026-10-07T09:00:00Z").unwrap();
        let skip = "BROKEN since 2026-10-07 08:00: 404: no such model";
        let r = json!({
            "combo": "coder", "state": "pass", "answered_by": "glm", "reason": "", "ms": 2400,
            "tried": [
                {"member": "sonnet", "kind": "unified", "state": "broken", "reason": "skipped: BROKEN on every account",
                 "attempts": [{"provider": "anthropic", "account": "max", "model": "s", "state": "broken",
                               "reason": skip, "skipped": skip, "ms": 0}]},
                {"member": "fallback-chain", "kind": "combo", "state": "pass", "reason": "", "tried": [
                    {"member": "gpt", "kind": "unified", "state": "unknown", "reason": "503: upstream overloaded",
                     "attempts": [{"provider": "openrouter", "account": "main", "model": "g", "state": "unknown",
                                   "reason": "503: upstream overloaded", "ms": 300}]},
                    {"member": "glm", "kind": "unified", "state": "pass", "reason": "",
                     "attempts": [{"provider": "opencode-go", "account": "main", "model": "z", "state": "pass",
                                   "reason": "", "ms": 2100}]}
                ]}
            ]
        });
        assert_eq!(
            combo_lines(&r, now),
            [
                "combo coder: PASS, answered by glm",
                "  sonnet           BROKEN   skipped: BROKEN on every account",
                "  fallback-chain   PASS",
                "    gpt            UNKNOWN  openrouter/main 503: upstream overloaded (not saved)",
                "    glm            PASS     opencode-go/main 2.1 s",
            ]
        );
        let unknown = json!({"combo": "writer", "state": "unknown", "reason": "503: busy", "ms": 1,
                             "next": "2026-10-07T09:05:00Z", "tried": []});
        assert_eq!(combo_lines(&unknown, now), ["combo writer: UNKNOWN: 503: busy; retest in 5 min"]);
    }
}
