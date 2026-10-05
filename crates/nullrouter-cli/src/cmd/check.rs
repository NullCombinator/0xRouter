//! `nullrouter check`: the registry report, plus the sign-in files (spec 005 T092): token
//! entries without a sign-in account, sign-in accounts without tokens, and file modes.
//! Names and modes only; a token is never printed.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use nullrouter_engine::accounts::{self, Accounts};
use nullrouter_engine::files::FileError;
use nullrouter_engine::identity;
use nullrouter_engine::quota::history;
use nullrouter_engine::tokens::{self, TokenStore};
use nullrouter_registry::schema::glob_match;
use serde_json::json;

pub(crate) fn run(home: Option<PathBuf>, as_json: bool) -> Result<ExitCode, ExitCode> {
    let handle = crate::open(home)?;
    let reg = handle.snapshot();
    let r = reg.report();
    let s = signin_report(handle.home().path());
    // The record journal's health, when a server answers (it owns the writer).
    let journal = nullrouter_server::operator::call(&nullrouter_registry::OperatorHome::new(handle.home().path()), &json!({"op": "routing.health"}))
        .ok()
        .filter(|a| a["ok"] == true)
        .map(|a| a["journal"].clone());
    let journal_line = journal.as_ref().filter(|j| j["kept"] == false).map(|j| {
        format!(
            "records not kept since {} (disk full): {} requests",
            j["since"].as_str().unwrap_or("?"),
            j["unkept_requests"]
        )
    });
    let pair = |(p, n): &(String, String)| json!({ "provider": p, "name": n });
    let unmetered = unmetered_windows(&reg);

    if as_json {
        let out = json!({
            "home": handle.home().path(),
            "providers": { "bundled": r.bundled, "user": r.user },
            "unified_models": r.unified_models,
            "pending_conflicts": r.pending_conflicts.iter().map(|c| json!({ "id": c.id, "path": c.path })).collect::<Vec<_>>(),
            "declined": r.declined.iter().map(|c| json!({ "id": c.id, "path": c.path })).collect::<Vec<_>>(),
            "withheld_credentials": r.withheld_credentials.iter()
                .map(|w| json!({ "provider": w.provider, "offending_url": w.offending_url.as_str() })).collect::<Vec<_>>(),
            "skipped": r.skipped.iter().map(|s| json!({
                "path": s.path, "id": s.id, "errors": s.errors.iter().map(ToString::to_string).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "dropped_unified_models": r.dropped_unified_models.iter()
                .map(|d| json!({ "name": d.name, "provider": d.provider })).collect::<Vec<_>>(),
            "limits_notes": r.notes.iter().map(crate::cmd::resolve::note_json).collect::<Vec<_>>(),
            "journal": journal,
            "unmetered_windows": unmetered.iter().map(|(p, w)| json!({ "provider": p, "window": w })).collect::<Vec<_>>(),
            "signin": {
                "errors": s.errors,
                "tokens_without_account": s.orphan_tokens.iter().map(pair).collect::<Vec<_>>(),
                "accounts_without_tokens": s.tokenless.iter().map(|a| {
                    let mut v = pair(a);
                    v["fix"] = json!(signin_command(&a.0, &a.1));
                    v
                }).collect::<Vec<_>>(),
                "file_modes": s.modes.iter().map(|m| json!({
                    "path": m.path, "mode": format!("{:o}", m.mode), "expected": format!("{:o}", m.expected),
                    "error": m.fatal,
                })).collect::<Vec<_>>(),
            },
        });
        println!("{out:#}");
    } else {
        println!("home: {}", handle.home().path().display());
        println!("providers: {} ({} bundled, {} user)", r.bundled + r.user, r.bundled, r.user);
        println!("unified models: {}", r.unified_models);
        for c in &r.pending_conflicts {
            println!(
                "conflict pending: {} shadows bundled {}; bundled is active until plugin_decisions.{} is set",
                c.path.display(),
                c.id,
                c.id
            );
        }
        for c in &r.declined {
            println!("declined: {} (bundled {} stays active)", c.path.display(), c.id);
        }
        for w in &r.withheld_credentials {
            println!("credential WITHHELD: {w}");
        }
        for s in &r.skipped {
            println!("skipped: {}", s.path.display());
            for e in &s.errors {
                println!("  {e}");
            }
        }
        for d in &r.dropped_unified_models {
            println!("dropped unified model {}: member provider {} was skipped", d.name, d.provider);
        }
        for n in &r.notes {
            println!("note: {n}");
        }
        if let Some(line) = &journal_line {
            println!("warning: {line}");
        }
        for (p, w) in &unmetered {
            println!("note: {p} reports window {w}, which no [[routing.window]] meter names; it is paced in its own unit");
        }
        for e in &s.errors {
            println!("error: {e}");
        }
        for m in &s.modes {
            println!("{}: {}", if m.fatal { "error" } else { "warning" }, m.line());
        }
        for (p, n) in &s.orphan_tokens {
            println!("warning: tokens.toml has tokens for {p}/{n}, which is not a sign-in account; they are ignored");
        }
        for (p, n) in &s.tokenless {
            println!("warning: sign-in account {p}/{n} has no tokens and can't serve; run `{}`", signin_command(p, n));
        }
    }
    let errors = !r.skipped.is_empty()
        || !r.dropped_unified_models.is_empty()
        || !s.errors.is_empty()
        || s.modes.iter().any(|m| m.fatal);
    Ok(ExitCode::from(u8::from(errors)))
}

/// The windows a provider's `[quota]` reports by a fixed name that none of its
/// `[[routing.window]]` meters matches (spec 006 T079): they are paced in the unit the provider
/// reports, which is enough between accounts of one provider. Names built from the response
/// (`{1}`, `{path}`) can't be checked here.
fn unmetered_windows(reg: &nullrouter_registry::Registry) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for p in reg.providers() {
        let Some(q) = &p.quota else { continue };
        let meters = p.routing().windows;
        for source in q.sources() {
            let names = source.windows.iter().map(|w| w.name.clone()).chain(source.name.clone());
            for name in names.filter(|n| !n.contains('{')) {
                let metered = meters.iter().any(|m| m.name == name || glob_match(&m.name, &name));
                if !metered && !out.iter().any(|(id, n)| *id == p.id && *n == name) {
                    out.push((p.id.clone(), name));
                }
            }
        }
    }
    out
}

/// The sign-in files' findings (spec 005 T092). Built from names and modes only: no token
/// is ever read into the report.
#[derive(Default)]
struct SigninReport {
    /// Files `serve` refuses to start with.
    errors: Vec<String>,
    /// `tokens.toml` entries with no matching sign-in account: ignored.
    orphan_tokens: Vec<(String, String)>,
    /// Sign-in accounts with no token entry: they can't serve.
    tokenless: Vec<(String, String)>,
    /// Paths whose mode isn't 0600 (files) or 0700 (directories).
    modes: Vec<ModeFinding>,
}

struct ModeFinding {
    path: PathBuf,
    mode: u32,
    expected: u32,
    /// `serve` refuses to start with it.
    fatal: bool,
}

impl ModeFinding {
    fn line(&self) -> String {
        let (want, fix) = if self.expected == 0o700 { ("0700", "chmod 700") } else { ("0600", "chmod 600") };
        let tail = if self.fatal { "; `serve` refuses to start" } else { "" };
        format!(
            "{} has mode {:o}, expected {want}; run `{fix} {}`{tail}",
            self.path.display(),
            self.mode,
            self.path.display()
        )
    }
}

fn mode_of(path: &Path) -> Option<(u32, bool)> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    Some((meta.permissions().mode() & 0o777, meta.is_dir()))
}

fn signin_report(home: &Path) -> SigninReport {
    let mut r = SigninReport::default();

    let check_mode = |path: PathBuf, fatal: bool, r: &mut SigninReport| {
        if let Some((mode, is_dir)) = mode_of(&path) {
            let expected = if is_dir { 0o700 } else { 0o600 };
            if mode != expected {
                r.modes.push(ModeFinding { path, mode, expected, fatal: fatal && mode & 0o077 != 0 });
            }
        }
    };
    // `serve` refuses a tokens.toml readable by others (as accounts.toml).
    check_mode(tokens::path(home), true, &mut r);
    check_mode(home.join(tokens::LOCK), false, &mut r);
    check_mode(home.join(identity::INSTALL_ID_FILE), false, &mut r);
    let quota = home.join(history::DIR);
    if quota.is_dir() {
        let mut stack = vec![quota];
        while let Some(dir) = stack.pop() {
            check_mode(dir.clone(), false, &mut r);
            let Ok(read) = std::fs::read_dir(&dir) else { continue };
            let mut children: Vec<PathBuf> = read.filter_map(|e| e.ok().map(|e| e.path())).collect();
            children.sort();
            // Popped last-first, so push in reverse: the walk reports in path order.
            for child in children.into_iter().rev() {
                match std::fs::symlink_metadata(&child) {
                    Ok(m) if m.is_dir() => stack.push(child),
                    Ok(_) => check_mode(child, false, &mut r),
                    Err(_) => {}
                }
            }
        }
    }

    let list = match Accounts::load(&home.join(accounts::FILE)) {
        Ok(l) => Some(l),
        Err(e) => {
            r.errors.push(format!("{}; `serve` refuses to start", e));
            None
        }
    };
    let store = match TokenStore::load(home) {
        Ok(s) => Some(s),
        Err(FileError::NotPrivate { .. }) => None, // already reported above
        Err(e) => {
            r.errors.push(format!("{}; `serve` refuses to start", e));
            None
        }
    };
    if let (Some(list), Some(store)) = (list, store) {
        for e in &store.entries {
            if !list.get(&e.provider, &e.name).is_some_and(accounts::Account::is_signin) {
                r.orphan_tokens.push((e.provider.clone(), e.name.clone()));
            }
        }
        for a in list.iter().filter(|a| a.is_signin()) {
            if store.get(&a.provider, &a.name).is_none() {
                r.tokenless.push((a.provider.clone(), a.name.clone()));
            }
        }
    }
    r
}

fn signin_command(provider: &str, name: &str) -> String {
    format!("nullrouter accounts signin {provider} {name}")
}
