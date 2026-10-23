//! The account operations the CLI and the dashboard share (`accounts add`, `priority`,
//! `remove`, `disable`, `enable`, `exclusive`). Nothing here reads stdin, prints or exits: what
//! the CLI prints on stderr comes back in `log`, in order.

use nullrouter_engine::accounts::{self, Account, AccountError, Accounts, SecretSource};
use nullrouter_engine::signin::SignInHttp;
use nullrouter_engine::tasks::{TaskHandle, TaskId, TaskInput, TaskKind};
use nullrouter_engine::{clock, tokens};
use nullrouter_registry::SecretString;
use serde_json::{Value, json};

use super::signin::{EXIT_ENDED, Hooks, SignIn};
use super::{Ctx, Done, OpError, Status, apply};

/// What an operation produced: the result for the caller, the JSON the CLI prints with
/// `--json`, and the status word (`applied`, `saved; applies at next start`, or empty when the
/// running server refused the files and `done.status` is `SavedNotApplied`).
pub struct Report {
    pub done: Done,
    pub json: Value,
    pub status: &'static str,
}

pub(crate) fn fail(e: impl std::fmt::Display) -> OpError {
    OpError { text: e.to_string(), exit: 1, field: None }
}

fn fail_in(field: &'static str, e: impl std::fmt::Display) -> OpError {
    OpError { text: e.to_string(), exit: 1, field: Some(field) }
}

fn open(ctx: &Ctx) -> Result<nullrouter_registry::RegistryHandle, OpError> {
    crate::views::open_registry(&ctx.home).map_err(|e| OpError { text: e.message, exit: e.code, field: None })
}

fn load(ctx: &Ctx) -> Result<Accounts, OpError> {
    Accounts::load(&ctx.home.path().join(accounts::FILE)).map_err(fail)
}

/// A reload the running server refused: the files are saved, the reason is the text.
pub(crate) fn refused(reason: String) -> Report {
    Report {
        done: Done { status: Status::SavedNotApplied { reason: reason.clone() }, text: reason, once: None },
        json: Value::Null,
        status: "",
    }
}

/// The checks `add` makes before anything else (a name and a provider that exists), so the CLI
/// can refuse before it reads a secret from stdin.
pub fn precheck_add(ctx: &Ctx, provider: &str, name: &str) -> Result<(), OpError> {
    load(ctx)?;
    if !accounts::valid_name(name) {
        return Err(fail_in("name", AccountError::BadName(name.into())));
    }
    open(ctx)?.snapshot().provider(provider).map_err(|e| fail_in("provider", e))?;
    Ok(())
}

/// What the account's operation did after the reload, for its line.
struct Tail<'a> {
    provider: &'a str,
    name: &'a str,
    note: Option<&'static str>,
    removed: bool,
    gone: Option<Accounts>,
    added: bool,
}

fn finish(ctx: &Ctx, log: &mut Vec<String>, list: &Accounts, t: Tail<'_>) -> Result<Report, OpError> {
    list.save().map_err(fail)?;
    let applied = match apply(ctx) {
        Ok(a) => a,
        Err(reason) => return Ok(refused(reason)),
    };
    for n in &applied.notes {
        log.push(format!("note: {n}"));
    }
    let status = applied.text;
    let (provider, name) = (t.provider, t.name);
    let home = ctx.home.path();
    // A running server drops the account's fingerprints and ledger entries when it reloads; with
    // none running, the warm file is edited here.
    if t.removed
        && status != "applied"
        && let Err(e) = nullrouter_engine::journal::state::forget_account(home, &format!("{provider}/{name}"))
    {
        log.push(format!("note: the routing state of {provider}/{name} could not be cleaned: {e}"));
    }
    // A running server's reload settles the quota fit; with none running, it is settled here.
    if status != "applied" {
        let fit = if let Some(only) = &t.gone {
            open(ctx).map(|r| nullrouter_engine::quota::fit::account_removed(home, &r.snapshot(), only, provider, name))
        } else if t.added {
            Ok(nullrouter_engine::quota::fit::account_added(home, provider, name))
        } else {
            Ok(Ok(()))
        };
        if let Ok(Err(e)) = fit {
            log.push(format!("note: the quota fit of {provider}/{name} could not be updated: {e}"));
        }
    }
    let text = match t.note {
        Some(note) => format!("{provider}/{name}: {note}; {status}"),
        None => format!("{provider}/{name}: {status}"),
    };
    Ok(Report {
        done: Done { status: Status::Applied, text, once: None },
        json: json!({"provider": provider, "name": name, "note": t.note, "status": status}),
        status,
    })
}

/// Adds an account. `source` says where the secret lives; `secret` is its value now, `None` for
/// an `--env` variable that is not set here.
pub fn add_with(
    ctx: &Ctx,
    log: &mut Vec<String>,
    provider: &str,
    name: &str,
    source: SecretSource,
    secret: Option<SecretString>,
    order: Option<i64>,
) -> Result<Report, OpError> {
    let mut list = load(ctx)?;
    if !accounts::valid_name(name) {
        return Err(fail_in("name", AccountError::BadName(name.into())));
    }
    // The secret may only go to the hosts the provider sends to now (FR-044).
    let reg = open(ctx)?.snapshot();
    let hosts = reg.provider(provider).map(accounts::provider_hosts).map_err(|e| fail_in("provider", e))?;
    // Without `--order`, a new account goes after the provider's others.
    let order =
        order.unwrap_or_else(|| list.iter().filter(|a| a.provider == provider).map(|a| a.order).max().unwrap_or(0));
    let account = Account::key(provider.to_owned(), name.to_owned(), source, secret, order, hosts);
    list.add(account).map_err(fail)?;
    finish(ctx, log, &list, Tail { provider, name, note: None, removed: false, gone: None, added: true })
}

pub fn add(
    ctx: &Ctx,
    provider: &str,
    name: &str,
    source: SecretSource,
    secret: Option<SecretString>,
    order: Option<i64>,
) -> Result<Done, OpError> {
    add_with(ctx, &mut Vec::new(), provider, name, source, secret, order).map(|r| r.done)
}

pub fn priority_with(
    ctx: &Ctx,
    log: &mut Vec<String>,
    provider: &str,
    name: &str,
    priority: &str,
) -> Result<Report, OpError> {
    let mut list = load(ctx)?;
    let n = priority
        .parse::<f64>()
        .map_err(|_| fail_in("priority", format!("{priority:?}: priority must be a number of 0 or more")))?;
    list.set_priority(provider, name, n).map_err(fail)?;
    finish(ctx, log, &list, Tail { provider, name, note: None, removed: false, gone: None, added: false })
}

pub fn priority(ctx: &Ctx, provider: &str, name: &str, priority: &str) -> Result<Done, OpError> {
    priority_with(ctx, &mut Vec::new(), provider, name, priority).map(|r| r.done)
}

pub fn remove_with(ctx: &Ctx, log: &mut Vec<String>, provider: &str, name: &str) -> Result<Report, OpError> {
    let mut list = load(ctx)?;
    let account = list.remove(provider, name).map_err(fail)?;
    // Kept for the quota fit, which needs the account's provider scope.
    let mut only = Accounts::default();
    only.add(account).map_err(fail)?;
    // Its tokens go too, under the writers' lock; its quota history stays (`quota forget`
    // deletes it).
    let note = tokens::remove(ctx.home.path(), provider, name).map_err(fail)?.then_some("removed with its tokens");
    finish(ctx, log, &list, Tail { provider, name, note, removed: true, gone: Some(only), added: false })
}

pub fn remove(ctx: &Ctx, provider: &str, name: &str) -> Result<Done, OpError> {
    remove_with(ctx, &mut Vec::new(), provider, name).map(|r| r.done)
}

/// The sentence the CLI's `[y/N]` prompt and the dashboard's confirm window show for a removal.
#[must_use]
pub fn remove_question(provider: &str, name: &str) -> String {
    super::describe_loss("accounts.remove", &format!("{provider}/{name}")).unwrap_or_default()
}

fn set_disabled_with(
    ctx: &Ctx,
    log: &mut Vec<String>,
    provider: &str,
    name: &str,
    disabled: bool,
) -> Result<Report, OpError> {
    let mut list = load(ctx)?;
    list.set_disabled(provider, name, disabled).map_err(fail)?;
    let mut note = None;
    if !disabled {
        // A refused account is tried again (research R10).
        if tokens::clear_refused(ctx.home.path(), provider, name).map_err(fail)? {
            note = Some("no longer marked refused");
        }
    }
    finish(ctx, log, &list, Tail { provider, name, note, removed: false, gone: None, added: false })
}

pub fn disable_with(ctx: &Ctx, log: &mut Vec<String>, provider: &str, name: &str) -> Result<Report, OpError> {
    set_disabled_with(ctx, log, provider, name, true)
}

pub fn enable_with(ctx: &Ctx, log: &mut Vec<String>, provider: &str, name: &str) -> Result<Report, OpError> {
    set_disabled_with(ctx, log, provider, name, false)
}

pub fn disable(ctx: &Ctx, provider: &str, name: &str) -> Result<Done, OpError> {
    disable_with(ctx, &mut Vec::new(), provider, name).map(|r| r.done)
}

pub fn enable(ctx: &Ctx, provider: &str, name: &str) -> Result<Done, OpError> {
    enable_with(ctx, &mut Vec::new(), provider, name).map(|r| r.done)
}

/// Declares an account in exclusive use (`on`) or not (FR-023); `on` keeps an earlier
/// declaration's start.
pub fn exclusive_with(
    ctx: &Ctx,
    log: &mut Vec<String>,
    provider: &str,
    name: &str,
    on: bool,
) -> Result<Report, OpError> {
    let mut list = load(ctx)?;
    let Some(current) = list.get(provider, name) else {
        return Err(fail(AccountError::NotFound { provider: provider.into(), name: name.into() }));
    };
    let since = if on {
        let reg = open(ctx)?.snapshot();
        let entity = reg.provider(provider).map_err(fail)?;
        if let Some(problem) = current.exclusive_use_problem(entity) {
            return Err(OpError { text: problem.to_string(), exit: 2, field: None });
        }
        Some(current.exclusive_use.unwrap_or_else(clock::now))
    } else {
        None
    };
    list.set_exclusive_use(provider, name, since).map_err(fail)?;
    finish(ctx, log, &list, Tail { provider, name, note: None, removed: false, gone: None, added: false })
}

pub fn exclusive(ctx: &Ctx, provider: &str, name: &str, on: bool) -> Result<Done, OpError> {
    exclusive_with(ctx, &mut Vec::new(), provider, name, on).map(|r| r.done)
}

/// `Write` into a task's progress: one line per `\n`; a prompt without one waits for the rest.
struct Lines<'a> {
    task: &'a TaskHandle,
    buf: Vec<u8>,
}

impl std::io::Write for Lines<'_> {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(b);
        while let Some(i) = self.buf.iter().position(|c| *c == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=i).collect();
            self.task.progress(String::from_utf8_lossy(&line[..i]).into_owned());
        }
        Ok(b.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Starts a sign-in as an engine task (spec 014 R4, R5) and returns at once with its id. The
/// flow is the CLI's ([`SignIn`]) on its own thread; its prompts become progress lines, a code
/// page waits for [`signin_paste`], a device flow shows its code and link as the task's input.
///
/// A provider that declares a terms warning needs `accept_terms`, as the CLI needs
/// `--accept-terms-risk`. `dashboard_return` is the dashboard's origin (`http://host:port`): a
/// loopback redirect then ends with a `303` to `<origin>/providers?window=provider&id=<p>&task=<id>`.
pub fn signin(
    ctx: &Ctx,
    provider: &str,
    name: &str,
    accept_terms: bool,
    dashboard_return: Option<&str>,
) -> Result<Done, OpError> {
    let Some(engine) = ctx.engine.clone() else {
        return Err(fail("sign-in from the dashboard needs the running server"));
    };
    if !accounts::valid_name(name) {
        return Err(fail_in("name", AccountError::BadName(name.into())));
    }
    let reg = engine.snapshot().registry.clone();
    let entity = reg.provider(provider).map_err(|e| fail_in("provider", e))?;
    let Some(decl) = &entity.signin else {
        return Err(fail_in(
            "provider",
            format!("{provider} has no sign-in; add a key with `nullrouter accounts add {provider} {name}`"),
        ));
    };
    if decl.terms_warning && !accept_terms {
        return Err(OpError {
            text: format!("{provider}/{name}: sign-in ended: the terms risk was not accepted; nothing saved"),
            exit: EXIT_ENDED,
            field: Some("accept_terms"),
        });
    }
    let (provider_id, subject) = (entity.id.clone(), format!("{}/{name}", entity.id));
    let task = engine.tasks.start(TaskKind::Signin, subject).map_err(fail)?;
    let id = task.id().clone();
    let return_to = dashboard_return.and_then(|base| {
        let mut u = url::Url::parse(base).ok().filter(|u| matches!(u.scheme(), "http" | "https"))?;
        u.set_path("/providers");
        u.query_pairs_mut()
            .clear()
            .append_pair("window", "provider")
            .append_pair("id", &provider_id)
            .append_pair("task", id.as_str());
        Some(u.to_string())
    });
    let paste = task.paste_receiver();
    let (home, name_owned, thread_id) = (ctx.home.clone(), name.to_owned(), id.clone());
    let spawned = std::thread::Builder::new().name(format!("signin-{id}")).spawn(move || {
        run_signin(&engine, &home, &provider_id, &name_owned, accept_terms, return_to.as_deref(), task, paste);
    });
    spawned.map_err(|e| fail(format!("could not start the sign-in: {e}")))?;
    Ok(Done {
        status: Status::Started { task: thread_id },
        text: format!("{provider}/{name}: sign-in started"),
        once: None,
    })
}

#[allow(clippy::too_many_arguments)]
fn run_signin(
    engine: &std::sync::Arc<nullrouter_engine::state::Engine>,
    home: &nullrouter_registry::OperatorHome,
    provider: &str,
    name: &str,
    accept_terms: bool,
    return_to: Option<&str>,
    task: TaskHandle,
    paste: std::sync::mpsc::Receiver<SecretString>,
) {
    let reg = engine.snapshot().registry.clone();
    let Ok(entity) = reg.provider(provider) else {
        return task.fail(format!("{provider}: no such provider"));
    };
    let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
        return task.fail("could not start the sign-in runtime");
    };
    // The paste channel is std; the driver reads lines from a tokio one. The bridge ends when
    // the task does (its sender drops, so `recv` fails).
    let (tx, mut input) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for code in paste {
            if code.with_exposed(|c| tx.send(c.to_owned())).is_err() {
                break;
            }
        }
    });
    let http = SignInHttp::new(reg.runtime().allow_private_endpoints);
    let job = SignIn { home, provider: entity, name, http: &http, accept_terms_risk: accept_terms, browser: None };
    let on_input = |i: TaskInput| task.set_input(Some(i));
    let hooks = Hooks { engine: Some(engine.clone()), on_input: Some(&on_input), return_to };
    let mut out = Lines { task: &task, buf: Vec::new() };
    let result = rt.block_on(job.run_with(&hooks, &mut input, &mut out, &tokio_util::sync::CancellationToken::new()));
    match result {
        Ok(done) => task.done(done.line()),
        Err(e) => task.fail(e.message),
    }
}

/// Hands the code the operator pasted to a sign-in task that waits for one. Nothing here keeps or
/// shows the code; a task that isn't waiting refuses.
pub fn signin_paste(ctx: &Ctx, task: &TaskId, code: SecretString) -> Result<Done, OpError> {
    let Some(engine) = &ctx.engine else {
        return Err(fail("sign-in from the dashboard needs the running server"));
    };
    engine.tasks.paste(task, code).map_err(fail)?;
    Ok(Done { status: Status::Started { task: task.clone() }, text: "code received".to_owned(), once: None })
}

#[cfg(test)]
mod signin_tests {
    use std::sync::Arc;

    use nullrouter_engine::state::Engine;
    use nullrouter_registry::OperatorHome;

    use super::*;

    fn ctx(dir: &std::path::Path) -> Ctx {
        let (engine, _) = Engine::open(OperatorHome::new(dir)).unwrap();
        Ctx { home: OperatorHome::new(dir), engine: Some(Arc::new(engine)) }
    }

    #[test]
    fn paste_refuses_a_task_that_is_not_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        let task = ctx.engine.as_ref().unwrap().tasks.start(TaskKind::Signin, "xai/main").unwrap();
        let id = task.id().clone();
        let e = signin_paste(&ctx, &id, SecretString::new("sekrit-code")).unwrap_err();
        assert_eq!(e.text, format!("{id}: not waiting for a code"));
        assert!(!format!("{e:?}").contains("sekrit-code"));
        task.done("x");
        assert!(signin_paste(&ctx, &id, SecretString::new("c")).is_err());
    }

    #[test]
    fn signin_needs_terms_and_a_provider_with_a_signin() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        let e = signin(&ctx, "anthropic", "max", false, None).unwrap_err();
        assert_eq!((e.exit, e.field), (5, Some("accept_terms")));
        assert!(signin(&ctx, "opencode-go", "main", true, None).unwrap_err().text.contains("no sign-in"));
        assert!(signin(&ctx, "nope", "main", true, None).is_err());
        assert!(signin(&ctx, "xai", "Bad Name", true, None).is_err());
    }
}
