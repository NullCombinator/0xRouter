//! `check`: the registry report, plus the sign-in files (spec 005 T092): token entries without a
//! sign-in account, sign-in accounts without tokens, and file modes; and the routing warnings of
//! spec 006 (T093). Names and modes only; a token is never printed.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use nullrouter_engine::accounts::{self, Accounts};
use nullrouter_engine::files::FileError;
use nullrouter_engine::identity;
use nullrouter_engine::journal::records;
use nullrouter_engine::quota::{history, poll};
use nullrouter_engine::routing::meter;
use nullrouter_engine::tokens::{self, TokenStore};
use nullrouter_registry::OperatorHome;
use nullrouter_registry::schema::{CacheMode, glob_match};
use serde_json::{Value, json};

use super::resolve::note_json;
use super::{Live, View, ViewError, open_registry};

/// The record journal's health, when a server answers (it owns the writer).
pub const NEEDS: &[&str] = &["routing.health"];

/// The `--json` object. `extra` has the text-only facts, each a list of lines the text prints
/// as is: `withheld` (`credential WITHHELD: …`), `notes` (`note: …`) and `mode_lines` (one per
/// `signin.file_modes` entry). The check fails (exit 1) when `skipped`,
/// `dropped_unified_models` or `signin.errors` is non-empty or a file mode is an error.
pub fn build(home: &OperatorHome, _args: &Value, live: &Live) -> Result<View, ViewError> {
    let handle = open_registry(home)?;
    let reg = handle.snapshot();
    let r = reg.report();
    let s = signin_report(handle.home().path());
    let journal = live.answer("routing.health").filter(|a| a["ok"] == true).map(|a| a["journal"].clone());
    let pair = |(p, n): &(String, String)| json!({ "provider": p, "name": n });
    let unmetered = unmetered_windows(&reg);
    let routing = routing_warnings(&reg, handle.home().path());
    let json = json!({
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
        "limits_notes": r.notes.iter().map(note_json).collect::<Vec<_>>(),
        "journal": journal,
        "unmetered_windows": unmetered.iter().map(|(p, w)| json!({ "provider": p, "window": w })).collect::<Vec<_>>(),
        "routing_warnings": routing,
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
    let extra = json!({
        "withheld": r.withheld_credentials.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "notes": r.notes.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "mode_lines": s.modes.iter().map(ModeFinding::line).collect::<Vec<_>>(),
    });
    Ok(View { json, extra })
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

/// Routing declarations that load but can't do what they say (contracts/routing-schema.md):
/// a pay-as-you-go account with no price ranks at price 1, and an `explicit` cache mode does
/// nothing when no endpoint speaks a style that carries cache markers.
fn routing_warnings(reg: &nullrouter_registry::Registry, home: &Path) -> Vec<String> {
    let mut out = Vec::new();
    // An unreadable accounts.toml is reported by the sign-in checks.
    if let Ok(list) = Accounts::load(&home.join(accounts::FILE)) {
        for a in list.iter() {
            let Some(p) = reg.providers().find(|p| p.id == a.provider) else { continue };
            let routing = p.routing();
            let payg = poll::reported(p, a).is_none()
                && !routing.windows.iter().any(|m| meter::capacity_of(m, &a.routing).is_some());
            if payg && routing.prices.is_empty() && a.routing.price.is_none() {
                out.push(format!(
                    "{}/{} is pay-as-you-go with no price in its plugin or account; it ranks as price 1 against other pay-as-you-go accounts",
                    a.provider, a.name
                ));
            }
        }
    }
    for p in reg.providers().filter(|p| p.routing().cache.mode == CacheMode::Explicit) {
        let marked = p
            .endpoints
            .values()
            .flat_map(|e| &e.0)
            .filter_map(|e| e.wire.as_deref())
            .any(|id| reg.style(id).is_some_and(|s| s.carries_cache_markers()));
        if !marked {
            out.push(format!(
                "{} declares cache mode explicit, but none of its endpoints speaks a style with cache markers; its prefixes are never marked",
                p.id
            ));
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
    /// Paths whose mode isn't 0600 (files) or 0700 (directories), sign-in, quota, record and
    /// routing files alike.
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
    check_mode(home.join(records::LOCK_FILE), false, &mut r);
    for top in [history::DIR, "records", "routing"].map(|d| home.join(d)).into_iter().filter(|d| d.is_dir()) {
        let mut stack = vec![top];
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
