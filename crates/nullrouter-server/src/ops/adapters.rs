//! The adapter operations the CLI and the dashboard share (`adapters install|review|build|
//! approve|reject|clear|rebuild|remove|review-settings`, `catalogue install`, `alerts ack`).
//! Nothing here prints, asks or exits.
//!
//! Review, build, rebuild and catalogue install are long. Inside the server (`ctx.engine` is
//! `Some`) each starts an engine task and returns `Status::Started` at once; in the CLI they run
//! inline. A review never runs in the CLI process, which would drop the queue at exit: after a
//! step that leaves a version `in_review` the CLI asks the running server over the operator
//! socket, and with no server the review starts when `serve` starts (`Engine::resume_reviews`).
//!
//! The catalogue is contacted only here, by an operator's `catalogue install` (FR-031): requests,
//! reloads and restarts never reach it, and the core does all the sending.

use std::future::Future;
use std::path::Path;
use std::sync::Arc;

use jiff::Timestamp;
use nullrouter_adapters::alerts::{Alert, AlertId, AlertKind, AlertLog};
use nullrouter_adapters::catalogue::{CatalogueError, Client};
use nullrouter_adapters::store::{Origin, ReviewConfig, Store, VersionId, VersionState};
use nullrouter_adapters::{BUILD_TIMEOUT, HarnessName, InstallError, InstallOptions, Installed};
use nullrouter_engine::keys::{self, Keys};
use nullrouter_engine::state::Engine;
use nullrouter_engine::tasks::TaskKind;
use serde_json::{Value, json};

use super::accounts::{Report, fail, refused};
use super::{Ctx, Done, OpError, Status, apply, describe_loss};
use crate::operator::{self, CallError};

/// Exit code for a package the gate or verification refused.
const REFUSED: u8 = 3;
/// Exit code for a catalogue that could not be reached or answered too much.
const UNREACHABLE: u8 = 5;

/// How long a review task waits for the queue to finish a version (the pipeline has no limit of its own).
const REVIEW_WAIT: std::time::Duration = std::time::Duration::from_secs(600);

/// The last line of a refusal.
pub const REFUSED_LINE: &str = "refused by the gate or by verification";

/// A failed install, build, rebuild or catalogue step, with what the CLI prints for it.
#[derive(Debug)]
pub struct Failure {
    /// The text and exit code, whole: what the dashboard shows.
    pub error: OpError,
    /// What `--json` prints on stdout in place of the text; `None` prints the text on stderr.
    pub json: Option<Value>,
    /// Text mode: this goes to stdout, then `tail` to stderr, in place of `error.text`.
    pub out: Option<String>,
    pub tail: Option<&'static str>,
}

impl From<OpError> for Box<Failure> {
    fn from(error: OpError) -> Self {
        Box::new(Failure { error, json: None, out: None, tail: None })
    }
}

fn field_fail(field: &'static str, e: impl std::fmt::Display) -> OpError {
    OpError { text: e.to_string(), exit: 1, field: Some(field) }
}

fn harness_of(s: &str) -> Result<HarnessName, OpError> {
    HarnessName::new(s).map_err(|e| field_fail("harness", e))
}

fn store_of(ctx: &Ctx) -> Result<Store, OpError> {
    Store::open(ctx.home.path()).map_err(fail)
}

/// The index, empty while there is no `adapters/` (a read creates nothing).
fn read_index(ctx: &Ctx) -> Result<nullrouter_adapters::store::Index, OpError> {
    if !ctx.home.path().join("adapters").is_dir() {
        return Ok(nullrouter_adapters::store::Index::default());
    }
    store_of(ctx)?.load_index().map_err(fail)
}

/// The engine of the server, or one opened over the home for the CLI (which reads the settings
/// and the styles from it).
fn engine_of(ctx: &Ctx) -> Result<Arc<Engine>, OpError> {
    match &ctx.engine {
        Some(e) => Ok(e.clone()),
        None => {
            let (engine, _) = Engine::open(ctx.home.clone()).map_err(|e| fail(format!("startup failed:\n{e}")))?;
            Ok(Arc::new(engine))
        }
    }
}

/// Runs `f` to its end on the server's runtime, or on a private one in the CLI. Never call it
/// from a runtime thread: it blocks.
fn block_on<F: Future>(f: F) -> Result<F::Output, OpError> {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        return Ok(handle.block_on(f));
    }
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(fail)?;
    Ok(rt.block_on(f))
}

/// Runs `work` inline when there is no engine (the CLI); inside the server starts it as an
/// engine task on its own thread and returns the task's id. A task ends `done` with the report's
/// text, or `failed` with the failure's.
fn long<F>(ctx: &Ctx, log: &mut Vec<String>, kind: TaskKind, subject: String, work: F) -> Result<Report, Box<Failure>>
where
    F: FnOnce(&Ctx, &mut Vec<String>) -> Result<Report, Box<Failure>> + Send + 'static,
{
    let Some(engine) = ctx.engine.clone() else { return work(ctx, log) };
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return Err(OpError { text: "no server is running".into(), exit: 4, field: None }.into());
    };
    let task = engine.tasks.start(kind, subject).map_err(fail)?;
    let id = task.id().clone();
    let ctx = Ctx { home: ctx.home.clone(), engine: Some(engine) };
    let spawned = std::thread::Builder::new().name(format!("adapter-task-{id}")).spawn(move || {
        let _enter = runtime.enter();
        let mut lines = Vec::new();
        let ended = work(&ctx, &mut lines);
        for l in lines {
            task.progress(l);
        }
        match ended {
            Ok(r) => task.done(r.done.text),
            Err(f) => task.fail(f.error.text),
        }
    });
    spawned.map_err(|e| fail(format!("could not start the task: {e}")))?;
    Ok(Report {
        done: Done { status: Status::Started { task: id }, text: "started".to_owned(), once: None },
        json: Value::Null,
        status: "",
    })
}

/// Reloads and builds the report: `line` is the text before `: <status>`.
fn settled(ctx: &Ctx, log: &mut Vec<String>, line: &str, mut json: Value) -> Report {
    match apply(ctx) {
        Ok(a) => {
            for n in &a.notes {
                log.push(format!("note: {n}"));
            }
            json["status"] = a.text.into();
            Report {
                done: Done { status: Status::Applied, text: format!("{line}: {}", a.text), once: None },
                json,
                status: a.text,
            }
        }
        Err(reason) => refused(format!("{line}: {reason}")),
    }
}

fn plain(text: String, json: Value) -> Report {
    Report { done: Done { status: Status::Applied, text, once: None }, json, status: "" }
}

/// After a step that left a version `in_review`: reload, then have the server review it. In the
/// server the review is queued here; in the CLI the running server is asked, and with none the
/// review starts when `serve` does.
fn start_review(ctx: &Ctx, harness: &HarnessName, version: &VersionId) -> Result<&'static str, OpError> {
    let applied = apply(ctx).map_err(fail)?;
    if applied.text != "applied" {
        return Ok("the review starts when `serve` starts");
    }
    if let Some(engine) = &ctx.engine {
        engine.enqueue_review(harness.clone(), version.clone());
        return Ok("review queued");
    }
    let req = json!({"op": "adapters.review", "harness": harness.as_str(), "version": version.as_str()});
    match operator::call(&ctx.home, &req) {
        Ok(a) if a["ok"] == true => Ok("review queued"),
        Ok(a) => Err(fail(a["error"].as_str().unwrap_or("the server refused the review"))),
        Err(CallError::NoServer(_)) => Ok("the review starts when `serve` starts"),
        Err(e) => Err(fail(e)),
    }
}

fn installed_text(done: &Installed, hint: Option<&str>) -> String {
    let mut text = format!("{} {}: {}", done.harness, done.version, done.state);
    if !done.reasons.is_empty() {
        // One line per gate reason, as `gate/invalid/*.expected` lists them.
        text = format!("{text}\n{}", done.reasons.join("\n"));
    } else if !done.reason.is_empty() {
        text = format!("{text} ({})", done.reason);
    }
    if let Some(h) = hint {
        text = format!("{text}\n{h}");
    }
    text
}

fn installed_json(done: &Installed, hint: Option<&str>) -> Value {
    json!({"harness": done.harness, "version": done.version, "state": done.state,
           "reason": done.reason, "reasons": done.reasons, "note": hint})
}

/// Where an install or build ended, and the follow-up each end calls for.
fn finish_installed(ctx: &Ctx, done: &Installed) -> Result<Report, Box<Failure>> {
    let hint = match done.state {
        VersionState::Refused => {
            let out = installed_text(done, None);
            return Err(Box::new(Failure {
                error: OpError { text: format!("{out}\n{REFUSED_LINE}"), exit: REFUSED, field: None },
                json: Some(installed_json(done, None)),
                out: Some(out),
                tail: Some(REFUSED_LINE),
            }));
        }
        VersionState::InReview => start_review(ctx, &done.harness, &done.version)?.to_owned(),
        _ => format!(
            "install the builder, then run `nullrouter adapters build {} {} --retry`",
            done.harness, done.version
        ),
    };
    Ok(plain(installed_text(done, Some(&hint)), installed_json(done, Some(&hint))))
}

fn install_failure(e: InstallError) -> Box<Failure> {
    match e {
        InstallError::Refused(reasons) => {
            let lines: Vec<String> = reasons.iter().map(ToString::to_string).collect();
            Box::new(Failure {
                error: OpError { text: format!("{}\n{REFUSED_LINE}", lines.join("\n")), exit: REFUSED, field: None },
                json: Some(json!({"state": "refused", "reasons": lines})),
                out: None,
                tail: None,
            })
        }
        e => fail(e).into(),
    }
}

fn catalogue_failure(e: &CatalogueError) -> Box<Failure> {
    let code = e.code();
    let reasons: Option<Vec<String>> = match e {
        CatalogueError::Install(InstallError::Refused(r)) => Some(r.iter().map(ToString::to_string).collect()),
        _ => None,
    };
    let exit = match e {
        CatalogueError::Fetch { .. } | CatalogueError::NotHttps(_) | CatalogueError::TooLarge { .. } => UNREACHABLE,
        CatalogueError::HashMismatch { .. }
        | CatalogueError::FpMismatch { .. }
        | CatalogueError::Unpack(_)
        | CatalogueError::Install(InstallError::Refused(_)) => REFUSED,
        _ => 1,
    };
    let mut text = format!("error[{code}]: {e}");
    for line in reasons.iter().flatten() {
        text = format!("{text}\n{line}");
    }
    if reasons.is_some() {
        text = format!("{text}\n{REFUSED_LINE}");
    }
    Box::new(Failure {
        error: OpError { text, exit, field: None },
        json: Some(json!({"error": code, "message": e.to_string(), "reasons": reasons})),
        out: None,
        tail: None,
    })
}

fn into_op(r: Result<Report, Box<Failure>>) -> Result<Done, OpError> {
    r.map(|r| r.done).map_err(|f| f.error)
}

fn into_done(r: Result<Report, OpError>) -> Result<Done, OpError> {
    r.map(|r| r.done)
}

/// Gates, stores and builds a package (a directory or a `.tar.gz`); the review follows.
pub fn install_with(ctx: &Ctx, log: &mut Vec<String>, package: &Path) -> Result<Report, Box<Failure>> {
    let _ = log;
    let engine = engine_of(ctx)?;
    let builder = engine.snapshot().settings().adapters.builder.clone();
    let done = block_on(engine.install_adapter_only(package, builder.as_deref()))?.map_err(install_failure)?;
    finish_installed(ctx, &done)
}

pub fn install(ctx: &Ctx, package: &Path) -> Result<Done, OpError> {
    into_op(install_with(ctx, &mut Vec::new(), package))
}

/// Requeues a build that waited for the builder. A task inside the server.
pub fn build_with(ctx: &Ctx, log: &mut Vec<String>, harness: &str, version: &str) -> Result<Report, Box<Failure>> {
    let (h, v) = (harness_of(harness)?, VersionId::from_run(version));
    let subject = format!("{h} {v}");
    long(ctx, log, TaskKind::AdapterBuild, subject, move |ctx, _log| {
        let engine = engine_of(ctx)?;
        let st = engine.snapshot();
        let styles: Vec<&str> = st.styles.keys().map(String::as_str).collect();
        let opts = InstallOptions {
            styles: &styles,
            builder: st.settings().adapters.builder.as_deref(),
            origin: Origin::Local(String::new()),
            build_timeout: BUILD_TIMEOUT,
        };
        let done = block_on(nullrouter_adapters::build(ctx.home.path(), &h, &v, &opts))?.map_err(install_failure)?;
        finish_installed(ctx, &done)
    })
}

pub fn build(ctx: &Ctx, harness: &str, version: &str) -> Result<Done, OpError> {
    into_op(build_with(ctx, &mut Vec::new(), harness, version))
}

/// Requeues a quarantined review (`review --retry`). A task inside the server, which reviews
/// now and ends with the version's new state.
pub fn review_with(ctx: &Ctx, log: &mut Vec<String>, harness: &str, version: &str) -> Result<Report, Box<Failure>> {
    let (h, v) = (harness_of(harness)?, VersionId::from_run(version));
    let store = store_of(ctx)?;
    let mut index = store.load_index().map_err(fail)?;
    index.transition(&h, &v, VersionState::InReview, "").map_err(fail)?;
    store.save_index(&index).map_err(fail)?;
    let subject = format!("{h} {v}");
    long(ctx, log, TaskKind::AdapterReview, subject, move |ctx, log| {
        let Some(engine) = &ctx.engine else {
            let note = start_review(ctx, &h, &v)?;
            return Ok(plain(
                format!("{h} {v}: in_review\n{note}"),
                json!({"harness": h, "version": v, "state": "in_review", "note": note}),
            ));
        };
        let applied = apply(ctx).map_err(fail)?;
        for n in &applied.notes {
            log.push(format!("note: {n}"));
        }
        // Reviews stay serial: the queue runs it; this task waits for the version to leave `in_review`.
        engine.enqueue_review(h.clone(), v.clone());
        let deadline = std::time::Instant::now() + REVIEW_WAIT;
        loop {
            let index = read_index(ctx)?;
            let entry = index.version(&h, &v).ok_or_else(|| fail(format!("no version {v} of {h} in the index")))?;
            if entry.state != VersionState::InReview {
                let reason = if entry.state_reason.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", entry.state_reason)
                };
                return Ok(plain(
                    format!("{h} {v}: {}{reason}", entry.state),
                    json!({"harness": h, "version": v, "state": entry.state, "reason": entry.state_reason}),
                ));
            }
            if std::time::Instant::now() >= deadline {
                return Ok(plain(
                    format!("{h} {v}: still in review"),
                    json!({"harness": h, "version": v, "state": "in_review", "reason": ""}),
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    })
}

pub fn review(ctx: &Ctx, harness: &str, version: &str) -> Result<Done, OpError> {
    into_op(review_with(ctx, &mut Vec::new(), harness, version))
}

fn decide_with(
    ctx: &Ctx,
    log: &mut Vec<String>,
    harness: &str,
    version: &str,
    approve: bool,
    note: Option<&str>,
) -> Result<Report, OpError> {
    let (h, v) = (harness_of(harness)?, VersionId::from_run(version));
    let home = ctx.home.path();
    let word = if approve { "approved" } else { "rejected" };
    let ran = if approve {
        nullrouter_adapters::approve(home, &h, &v, note)
    } else {
        nullrouter_adapters::reject(home, &h, &v, note)
    };
    ran.map_err(fail)?;
    Ok(settled(ctx, log, &format!("{h} {v}: {word}"), json!({"harness": h, "version": v, "state": word})))
}

/// Approves a reported version; it becomes the active one.
pub fn approve_with(
    ctx: &Ctx,
    log: &mut Vec<String>,
    harness: &str,
    version: &str,
    note: Option<&str>,
) -> Result<Report, OpError> {
    decide_with(ctx, log, harness, version, true, note)
}

pub fn approve(ctx: &Ctx, harness: &str, version: &str, note: Option<&str>) -> Result<Done, OpError> {
    into_done(approve_with(ctx, &mut Vec::new(), harness, version, note))
}

/// Rejects a reported version.
pub fn reject_with(
    ctx: &Ctx,
    log: &mut Vec<String>,
    harness: &str,
    version: &str,
    note: Option<&str>,
) -> Result<Report, OpError> {
    decide_with(ctx, log, harness, version, false, note)
}

pub fn reject(ctx: &Ctx, harness: &str, version: &str, note: Option<&str>) -> Result<Done, OpError> {
    into_done(reject_with(ctx, &mut Vec::new(), harness, version, note))
}

/// The sentence the CLI's `[y/N]` prompt and the dashboard's confirm window show for `clear`.
#[must_use]
pub fn clear_question(harness: &str, version: &str) -> String {
    describe_loss("adapters.clear", &format!("{harness} {version}")).unwrap_or_default()
}

/// The guardrail events of a suspect version, oldest first. Refuses a version that isn't suspect.
pub fn clear_events(ctx: &Ctx, harness: &str, version: &str) -> Result<Vec<Alert>, OpError> {
    let (h, v) = (harness_of(harness)?, VersionId::from_run(version));
    let index = read_index(ctx)?;
    let state = index
        .version(&h, &v)
        .ok_or_else(|| fail(format!("no version {v} of {h} in the index")))?
        .state;
    if state != VersionState::Suspect {
        return Err(fail(format!("{h} {v} is {state}, not suspect")));
    }
    let store = store_of(ctx)?;
    let events = AlertLog::open(&store).list().map_err(fail)?;
    Ok(events
        .into_iter()
        .filter(|a| a.kind == AlertKind::Guardrail && a.harness == h && a.version == v)
        .collect())
}

/// One guardrail event as a line: when, the record it came from, what happened.
#[must_use]
pub fn event_line(a: &Alert) -> String {
    let record = a.record.as_deref().unwrap_or("-");
    let times = if a.count > 1 { format!("  (x{})", a.count) } else { String::new() };
    format!("{}  {record}  {}{times}", a.at, a.detail)
}

/// `suspect` to `approved`: the version serves again from the next request.
pub fn clear_with(ctx: &Ctx, log: &mut Vec<String>, harness: &str, version: &str) -> Result<Report, OpError> {
    let events = clear_events(ctx, harness, version)?;
    let (h, v) = (harness_of(harness)?, VersionId::from_run(version));
    let store = store_of(ctx)?;
    let mut index = store.load_index().map_err(fail)?;
    index.transition(&h, &v, VersionState::Approved, "cleared by the operator").map_err(fail)?;
    store.save_index(&index).map_err(fail)?;
    let json = json!({"harness": h, "version": v, "state": "approved", "events": events});
    Ok(settled(ctx, log, &format!("{h} {v}: approved"), json))
}

pub fn clear(ctx: &Ctx, harness: &str, version: &str) -> Result<Done, OpError> {
    into_done(clear_with(ctx, &mut Vec::new(), harness, version))
}

/// What a removal would do, checked before anything is asked: the keys that become plain clients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovePreview {
    pub bound: Vec<String>,
}

/// The sentence the CLI's `[y/N]` prompt and the dashboard's confirm window show for `remove`.
#[must_use]
pub fn remove_question(harness: &str, version: Option<&str>) -> String {
    let subject = version.map_or_else(|| format!("{harness} and all its versions"), |v| format!("{harness} {v}"));
    describe_loss("adapters.remove", &subject).unwrap_or_default()
}

/// Names of the keys bound to `harness`, revoked ones included (they keep the binding).
fn bound_keys(ctx: &Ctx, harness: &HarnessName) -> Result<Vec<String>, OpError> {
    let keys = Keys::load(&ctx.home.path().join(keys::FILE)).map_err(fail)?;
    Ok(keys.iter().filter(|k| k.adapter.as_ref() == Some(harness)).map(|k| k.name.clone()).collect())
}

/// The checks a removal makes first, and the keys it would turn into plain clients.
pub fn remove_preview(
    ctx: &Ctx,
    harness: &str,
    version: Option<&str>,
    force: bool,
) -> Result<RemovePreview, OpError> {
    let h = harness_of(harness)?;
    let v = version.map(VersionId::from_run);
    let index = read_index(ctx)?;
    let entry = index.harness(&h).ok_or_else(|| fail(format!("no harness {h} in the index")))?;
    if let Some(v) = &v {
        if index.version(&h, v).is_none() {
            return Err(fail(format!("no version {v} of {h} in the index")));
        }
        if entry.active.as_ref() == Some(v) && !force {
            return Err(fail(format!("{h} {v} is the active version; pass --force to remove it")));
        }
    }
    // Removing the last version leaves the keys with nothing to run, as removing the harness does.
    let whole = v.is_none() || entry.versions.len() == 1;
    let bound = if whole { bound_keys(ctx, &h)? } else { Vec::new() };
    Ok(RemovePreview { bound })
}

/// Removes a version, or the whole harness. The active version needs `force`.
pub fn remove_with(
    ctx: &Ctx,
    log: &mut Vec<String>,
    harness: &str,
    version: Option<&str>,
    force: bool,
) -> Result<Report, OpError> {
    let preview = remove_preview(ctx, harness, version, force)?;
    let h = harness_of(harness)?;
    let v = version.map(VersionId::from_run);
    let done = nullrouter_adapters::remove(ctx.home.path(), &h, v.as_ref(), force).map_err(fail)?;
    let removed: Vec<&str> = done.versions.iter().map(VersionId::as_str).collect();
    let json = json!({"harness": h, "removed": removed, "keys": preview.bound});
    Ok(settled(ctx, log, &format!("{h}: removed {} version(s)", removed.len()), json))
}

pub fn remove(ctx: &Ctx, harness: &str, version: Option<&str>, force: bool) -> Result<Done, OpError> {
    into_done(remove_with(ctx, &mut Vec::new(), harness, version, force))
}

/// Rebuilds approved versions whose module was built for a kit the core no longer runs, from
/// their reviewed source. A task inside the server; `harness` limits it to one.
pub fn rebuild_with(ctx: &Ctx, log: &mut Vec<String>, harness: Option<&str>) -> Result<Report, Box<Failure>> {
    let only = harness.map(harness_of).transpose()?;
    if let Some(h) = &only
        && read_index(ctx)?.harness(h).is_none()
    {
        return Err(fail(format!("no harness {h} in the index")).into());
    }
    let subject = only.as_ref().map_or_else(|| "all".to_owned(), ToString::to_string);
    long(ctx, log, TaskKind::AdapterRebuild, subject, move |ctx, _log| {
        let engine = engine_of(ctx)?;
        let builder = engine.snapshot().settings().adapters.builder.clone();
        let opts = InstallOptions {
            styles: &[],
            builder: builder.as_deref(),
            origin: Origin::Local("kit-upgrade".into()),
            build_timeout: BUILD_TIMEOUT,
        };
        // A reload that cannot reach a server is fine: the next start reads the store.
        let reload_server = || {
            let _ = apply(ctx);
        };
        let done = block_on(nullrouter_adapters::rebuilds_for(ctx.home.path(), &opts, &reload_server, only.as_ref()))?
            .map_err(fail)?;
        let index = read_index(ctx)?;
        let failed: Vec<(String, String)> = index
            .harnesses
            .iter()
            .filter(|h| only.as_ref().is_none_or(|o| &h.name == o))
            .flat_map(|h| {
                h.versions.iter().filter(|v| v.rebuild_failed).map(move |v| (h.name.to_string(), v.id.to_string()))
            })
            .collect();
        let json = json!({"rebuilt": done.iter().map(|(h, v)| json!([h, v])).collect::<Vec<Value>>(),
                          "failed": failed.iter().map(|(h, v)| json!([h, v])).collect::<Vec<Value>>()});
        let mut lines: Vec<String> = done.iter().map(|(h, v)| format!("{h} {v}: rebuilt")).collect();
        lines.extend(failed.iter().map(|(h, v)| format!("{h} {v}: rebuild failed")));
        if lines.is_empty() {
            return Ok(plain("nothing to rebuild".to_owned(), json));
        }
        let text = lines.join("\n");
        if failed.is_empty() {
            return Ok(plain(text, json));
        }
        Err(Box::new(Failure {
            error: OpError { text: text.clone(), exit: 1, field: None },
            json: Some(json),
            out: Some(text),
            tail: None,
        }))
    })
}

pub fn rebuild(ctx: &Ctx, harness: Option<&str>) -> Result<Done, OpError> {
    into_op(rebuild_with(ctx, &mut Vec::new(), harness))
}

/// Sets or clears the model that reviews adapter source.
pub fn review_settings_with(
    ctx: &Ctx,
    log: &mut Vec<String>,
    model: Option<&str>,
    budget: Option<u64>,
    reserve_output: Option<u32>,
    clear: bool,
) -> Result<Report, OpError> {
    let config = if clear {
        None
    } else {
        let (Some(model), Some(budget_tokens)) = (model, budget) else {
            return Err(fail("review-settings needs --model and --budget, or --clear"));
        };
        if model.trim().is_empty() {
            return Err(field_fail("model", "--model must be named and --budget must be above 0"));
        }
        if budget_tokens == 0 {
            return Err(field_fail("budget", "--model must be named and --budget must be above 0"));
        }
        let mut c: ReviewConfig =
            serde_json::from_value(json!({"model": model, "budget_tokens": budget_tokens})).map_err(fail)?;
        if let Some(r) = reserve_output {
            c.reserve_output = r;
        }
        Some(c)
    };
    let store = store_of(ctx)?;
    let mut index = store.load_index().map_err(fail)?;
    index.review.clone_from(&config);
    store.save_index(&index).map_err(fail)?;
    let line = match &config {
        Some(c) => format!("review: {} budget {} reserve {}", c.model, c.budget_tokens, c.reserve_output),
        None => "review settings cleared".to_owned(),
    };
    Ok(settled(ctx, log, &line, json!({"review": config})))
}

pub fn review_settings(
    ctx: &Ctx,
    model: Option<&str>,
    budget: Option<u64>,
    reserve_output: Option<u32>,
    clear: bool,
) -> Result<Done, OpError> {
    into_done(review_settings_with(ctx, &mut Vec::new(), model, budget, reserve_output, clear))
}

/// Fetches, verifies, gates and queues a catalogue version (the newest unless `semver` is named).
/// A task inside the server. The one place the catalogue is contacted (FR-031).
pub fn catalogue_install_with(
    ctx: &Ctx,
    log: &mut Vec<String>,
    harness: &str,
    semver: Option<&str>,
) -> Result<Report, Box<Failure>> {
    let (harness, semver) = (harness.to_owned(), semver.map(str::to_owned));
    let subject = semver.as_ref().map_or_else(|| harness.clone(), |s| format!("{harness} {s}"));
    long(ctx, log, TaskKind::CatalogueInstall, subject, move |ctx, _log| {
        let engine = engine_of(ctx)?;
        let st = engine.snapshot();
        let styles: Vec<&str> = st.styles.keys().map(String::as_str).collect();
        let url = st.settings().adapters.catalogue_url.clone();
        let opts = InstallOptions {
            styles: &styles,
            builder: st.settings().adapters.builder.as_deref(),
            origin: Origin::Local(String::new()),
            build_timeout: BUILD_TIMEOUT,
        };
        let client = Client::new().map_err(|e| catalogue_failure(&e))?;
        let done = block_on(client.install(ctx.home.path(), &url, &harness, semver.as_deref(), opts))?
            .map_err(|e| catalogue_failure(&e))?;
        finish_installed(ctx, &done)
    })
}

pub fn catalogue_install(ctx: &Ctx, harness: &str, semver: Option<&str>) -> Result<Done, OpError> {
    into_op(catalogue_install_with(ctx, &mut Vec::new(), harness, semver))
}

/// Acknowledges one alert by id, or every open one with `all`. Reads and writes the alert log
/// directly; a read creates nothing, so with no `adapters/` there is nothing to acknowledge.
pub fn alerts_ack_with(ctx: &Ctx, log: &mut Vec<String>, id: Option<&str>, all: bool) -> Result<Report, OpError> {
    let _ = log;
    let store = if ctx.home.path().join("adapters").is_dir() { Some(store_of(ctx)?) } else { None };
    if all {
        let n = match &store {
            Some(s) => AlertLog::open(s).ack_all(Timestamp::now()).map_err(fail)?,
            None => 0,
        };
        return Ok(plain(format!("acknowledged {n}"), json!({"acknowledged": n})));
    }
    let Some(raw) = id.filter(|s| !s.is_empty()) else {
        return Err(OpError { text: "nullrouter alerts ack: give an alert id, or --all".into(), exit: 2, field: Some("id") });
    };
    let unknown = || field_fail("id", format!("no open alert {raw}"));
    let Some(id) = AlertId::parse(raw) else { return Err(unknown()) };
    let Some(store) = store else { return Err(unknown()) };
    if !AlertLog::open(&store).ack(&id, Timestamp::now()).map_err(fail)? {
        return Err(unknown());
    }
    Ok(plain(format!("acknowledged {id}"), json!({"acknowledged": 1, "id": id})))
}

pub fn alerts_ack(ctx: &Ctx, id: Option<&str>, all: bool) -> Result<Done, OpError> {
    into_done(alerts_ack_with(ctx, &mut Vec::new(), id, all))
}
