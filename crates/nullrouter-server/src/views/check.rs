//! `check`: the registry report, plus the sign-in files (spec 005 T092): token entries without a
//! sign-in account, sign-in accounts without tokens, and file modes; and the routing warnings of
//! spec 006 (T093). Names and modes only; a token is never printed.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use nullrouter_engine::accounts::{self, Accounts};
use nullrouter_engine::connection::pause::ProxyBoard;
use nullrouter_engine::connection::proxy::{self, Proxies};
use nullrouter_engine::files::{self, FileError};
use nullrouter_engine::identity;
use nullrouter_engine::journal::records;
use nullrouter_engine::keys;
use nullrouter_engine::quota::{history, poll};
use nullrouter_engine::routing::meter;
use nullrouter_engine::tokens::{self, TokenStore};
use nullrouter_registry::LoadReport;
use nullrouter_registry::OperatorHome;
use nullrouter_registry::schema::{CacheMode, glob_match};
use serde_json::{Value, json};

use super::resolve::note_json;
use super::{Live, View, ViewError, open_registry};

/// The record journal's health and the address the client listener bound, when a server answers
/// (it owns the writer and the listener).
pub const NEEDS: &[&str] = &["routing.health", "server.status"];

/// The `--json` object. `notices` has every line the text prints after `unified models:`, in
/// print order, each with the page it concerns (spec 009 research R5). The check fails (exit 1)
/// when `skipped`, `dropped_unified_models` or `signin.errors` is non-empty or a file mode is an
/// error.
pub fn build(home: &OperatorHome, _args: &Value, live: &Live) -> Result<View, ViewError> {
    let handle = open_registry(home)?;
    let reg = handle.snapshot();
    let r = reg.report();
    let s = signin_report(handle.home().path());
    let journal = live.answer("routing.health").filter(|a| a["ok"] == true).map(|a| a["journal"].clone());
    let pair = |(p, n): &(String, String)| json!({ "provider": p, "name": n });
    let unmetered = unmetered_windows(&reg);
    let routing = routing_warnings(&reg, handle.home().path());
    let proxies = proxy_findings(&reg, handle.home().path());
    let status = live.answer("server.status").filter(|a| a["ok"] == true);
    let (endpoint, endpoint_source) = endpoint(status, &reg.runtime().server.listen);
    let json = json!({
        "home": handle.home().path(),
        "endpoint": endpoint,
        "endpoint_source": endpoint_source,
        "providers": { "bundled": r.bundled, "user": r.user },
        "unified_models": r.unified_models,
        "pending_conflicts": r.pending_conflicts.iter().map(|c| json!({ "id": c.id, "path": c.path })).collect::<Vec<_>>(),
        "declined": r.declined.iter().map(|c| json!({ "id": c.id, "path": c.path })).collect::<Vec<_>>(),
        "withheld_credentials": r.withheld_credentials.iter()
            .map(|w| json!({ "provider": w.provider, "offending_url": w.offending_url.as_str() })).collect::<Vec<_>>(),
        "skipped": r.skipped.iter().map(|s| json!({
            "path": s.path, "id": s.id, "errors": s.errors.iter().map(ToString::to_string).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "logos_ignored": r.logos_ignored.iter().map(|l| json!({ "id": l.id, "reason": l.reason })).collect::<Vec<_>>(),
        "dropped_unified_models": r.dropped_unified_models.iter()
            .map(|d| json!({ "name": d.name, "provider": d.provider })).collect::<Vec<_>>(),
        "limits_notes": r.notes.iter().map(note_json).collect::<Vec<_>>(),
        "journal": journal,
        "unmetered_windows": unmetered.iter().map(|(p, w)| json!({ "provider": p, "window": w })).collect::<Vec<_>>(),
        "routing_warnings": routing,
        "paused_proxies": proxies.paused.iter().map(|(n, since, _)| json!({ "name": n, "since": since })).collect::<Vec<_>>(),
        "undefined_proxies": proxies.undefined.iter().map(|(who, n)| json!({ "assigned_to": who, "proxy": n })).collect::<Vec<_>>(),
        "notices": notices(handle.home().path(), r, journal.as_ref(), &Findings { unmetered: &unmetered, routing: &routing, proxies: &proxies }, &s, status),
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
    Ok(View::new(json))
}

/// The client endpoint and where it came from (research R6): the address a running server bound
/// (`"server"`), else `config.toml`'s `[server] listen` (`"config"`).
fn endpoint(status: Option<&Value>, configured: &str) -> (String, &'static str) {
    match status.filter(|a| a["ok"] == true).and_then(|a| a["client_listen"].as_str()) {
        Some(bound) => (endpoint_url(bound), "server"),
        None => (endpoint_url(configured), "config"),
    }
}

/// `http://<host:port>/v1` for a listen address. This machine reads it, so an unspecified host
/// (`0.0.0.0`, `::`) is shown as the loopback address of its family.
fn endpoint_url(listen: &str) -> String {
    let (host, port) = listen.rsplit_once(':').unwrap_or((listen, ""));
    let host = match host.trim_start_matches('[').trim_end_matches(']') {
        "0.0.0.0" => "127.0.0.1".to_owned(),
        "::" => "[::1]".to_owned(),
        h if h.contains(':') => format!("[{h}]"),
        h => h.to_owned(),
    };
    format!("http://{host}:{port}/v1")
}

/// The pages a notice can concern (`dashboard-http.md`), by id.
const SUBJECTS: &[&str] = &["endpoint", "providers", "combo", "usage", "quota", "settings"];

/// One line `check` prints after `unified models:` (research R5).
struct Notice {
    level: &'static str,
    subject: &'static str,
    text: String,
}

impl Notice {
    fn new(level: &'static str, subject: &'static str, text: String) -> Self {
        debug_assert!(SUBJECTS.contains(&subject));
        Self { level, subject, text }
    }

    fn json(&self) -> Value {
        json!({ "level": self.level, "subject": self.subject, "text": self.text })
    }
}

/// The page a file-mode finding concerns: the sign-in, quota and routing files belong to Quota
/// Tracker, `keys.toml` to Endpoint & Key; any other file, and any path outside the home, to
/// Settings.
fn mode_subject(home: &Path, path: &Path) -> &'static str {
    let first = path.strip_prefix(home).ok().and_then(|p| p.components().next());
    match first.and_then(|c| c.as_os_str().to_str()) {
        Some(accounts::FILE | tokens::FILE | history::DIR | "routing") => "quota",
        Some(keys::FILE) => "endpoint",
        _ => "settings",
    }
}

/// Every line `check` prints after `unified models:`, in the order it prints them. The CLI's
/// text output is these lines and nothing else, so a page, the panel and `check` can't disagree.
fn notices(
    home: &Path,
    r: &LoadReport,
    journal: Option<&Value>,
    found: &Findings,
    s: &SigninReport,
    status: Option<&Value>,
) -> Vec<Value> {
    let Findings { unmetered, routing, proxies } = *found;
    let mut out = Vec::new();
    for c in &r.pending_conflicts {
        let (id, path) = (&c.id, c.path.display());
        let text = format!(
            "conflict pending: {path} shadows bundled {id}; bundled is active until plugin_decisions.{id} is set"
        );
        out.push(Notice::new("warning", "providers", text));
    }
    for c in &r.declined {
        out.push(Notice::new(
            "warning",
            "providers",
            format!("declined: {} (bundled {} stays active)", c.path.display(), c.id),
        ));
    }
    for w in &r.withheld_credentials {
        out.push(Notice::new("warning", "providers", format!("credential WITHHELD: {w}")));
    }
    for sk in &r.skipped {
        let mut text = format!("skipped: {}", sk.path.display());
        for e in &sk.errors {
            text.push_str(&format!("\n  {e}"));
        }
        out.push(Notice::new("error", "providers", text));
    }
    // A logo that failed the check: the plugin loaded without it (spec 009 research R10).
    for l in &r.logos_ignored {
        out.push(Notice::new("note", "providers", format!("note: logo ignored: {}: {}", l.id, l.reason)));
    }
    for d in &r.dropped_unified_models {
        let text = format!("dropped unified model {}: member provider {} was skipped", d.name, d.provider);
        out.push(Notice::new("error", "combo", text));
    }
    for n in &r.notes {
        out.push(Notice::new("note", "combo", format!("note: {n}")));
    }
    if let Some(j) = journal.filter(|j| j["kept"] == false) {
        let text = format!(
            "warning: records not kept since {} (disk full): {} requests",
            j["since"].as_str().unwrap_or("?"),
            j["unkept_requests"]
        );
        out.push(Notice::new("warning", "usage", text));
    }
    for (provider, window) in unmetered {
        let text = format!(
            "note: {provider} reports window {window}, which no [[routing.window]] meter names; it is paced in its own unit"
        );
        out.push(Notice::new("note", "quota", text));
    }
    for w in routing {
        out.push(Notice::new("warning", "quota", format!("warning: {w}")));
    }
    for (name, since, _) in &proxies.paused {
        let text = format!(
            "error: proxy {name} is paused (unreachable since {since}); fix it, then run nullrouter proxy fixed {name}"
        );
        out.push(Notice::new("error", "providers", text));
    }
    for (who, name) in &proxies.undefined {
        let text = format!("note: {who} is assigned proxy \"{name}\", which isn't defined");
        out.push(Notice::new("note", "providers", text));
    }
    for e in &s.errors {
        out.push(Notice::new("error", "quota", format!("error: {e}")));
    }
    for m in &s.modes {
        let level = if m.fatal { "error" } else { "warning" };
        out.push(Notice::new(level, mode_subject(home, &m.path), format!("{level}: {}", m.line())));
    }
    for (p, n) in &s.orphan_tokens {
        let text =
            format!("warning: tokens.toml has tokens for {p}/{n}, which is not a sign-in account; they are ignored");
        out.push(Notice::new("warning", "quota", text));
    }
    for (p, n) in &s.tokenless {
        let text =
            format!("warning: sign-in account {p}/{n} has no tokens and can't serve; run `{}`", signin_command(p, n));
        out.push(Notice::new("warning", "quota", text));
    }
    // A running server whose dashboard is on but couldn't bind; `error` is "<addr>: <reason>".
    if let Some(d) = status.map(|a| &a["dashboard"]).filter(|d| d["enabled"] == true && d["serving"] == false)
        && let Some(e) = d["error"].as_str()
    {
        out.push(Notice::new("warning", "settings", format!("warning: dashboard not listening: {e}")));
    }
    out.iter().map(Notice::json).collect()
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

/// The routing and proxy findings `notices` prints, bundled to keep its arguments few.
struct Findings<'a> {
    unmetered: &'a [(String, String)],
    routing: &'a [String],
    proxies: &'a ProxyFindings,
}

/// What `check` says about proxies (spec 013, US4): the paused ones, and assignments naming a
/// proxy `proxies.toml` doesn't define.
#[derive(Default)]
struct ProxyFindings {
    /// Name, since, reason.
    paused: Vec<(String, String, String)>,
    /// Who is assigned (`account kiro/old`, `provider kiro`, `all providers`), and the name.
    undefined: Vec<(String, String)>,
}

fn proxy_findings(reg: &nullrouter_registry::Registry, home: &Path) -> ProxyFindings {
    let mut f = ProxyFindings {
        paused: ProxyBoard::open(home).list().into_iter().map(|(n, p)| (n, p.since, p.reason)).collect(),
        undefined: Vec::new(),
    };
    // An unreadable proxies.toml is a startup error `serve` reports; nothing to compare with.
    let Ok(defined) = Proxies::load(&home.join(proxy::FILE)) else { return f };
    let mut check = |who: String, name: Option<&String>| {
        if let Some(n) = name.filter(|n| n.as_str() != proxy::NONE && defined.get(n).is_none()) {
            f.undefined.push((who, n.clone()));
        }
    };
    check("all providers".to_owned(), reg.runtime().connection_proxy.as_ref());
    for p in reg.providers() {
        check(format!("provider {}", p.id), reg.settings(&p.id).connection.proxy.as_ref());
    }
    if let Ok(list) = Accounts::load(&home.join(accounts::FILE)) {
        for a in list.iter() {
            check(format!("account {}/{}", a.provider, a.name), a.proxy.as_ref());
        }
    }
    f
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
    // As `keys.toml`: `serve` refuses a dashboard.toml readable by others (spec 009).
    check_mode(home.join(files::DASHBOARD_FILE), true, &mut r);
    // And a proxies.toml readable by others: it can hold a password.
    check_mode(home.join(proxy::FILE), true, &mut r);
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

#[cfg(test)]
mod tests {
    use nullrouter_engine::testkit::homes;
    use nullrouter_registry::{DroppedUnifiedModel, IgnoredLogo, LimitsNote, PluginConflict, WithheldCredential};

    use super::*;

    #[test]
    fn the_endpoint_follows_the_running_server_else_the_file() {
        let running = json!({"ok": true, "client_listen": "127.0.0.1:9000", "dashboard": {}});
        assert_eq!(endpoint(Some(&running), "127.0.0.1:20129"), ("http://127.0.0.1:9000/v1".into(), "server"));
        assert_eq!(endpoint(None, "127.0.0.1:20129"), ("http://127.0.0.1:20129/v1".into(), "config"));
        let unbound = json!({"ok": true, "client_listen": null, "dashboard": {}});
        assert_eq!(endpoint(Some(&unbound), "127.0.0.1:20129").1, "config");
        let refused = json!({"ok": false, "error": "nope"});
        assert_eq!(endpoint(Some(&refused), "127.0.0.1:1").1, "config");
    }

    #[test]
    fn an_unspecified_host_is_shown_as_loopback() {
        assert_eq!(endpoint_url("0.0.0.0:20129"), "http://127.0.0.1:20129/v1");
        assert_eq!(endpoint_url("[::]:20129"), "http://[::1]:20129/v1");
        assert_eq!(endpoint_url("[::1]:20129"), "http://[::1]:20129/v1");
        assert_eq!(endpoint_url("192.168.1.5:8080"), "http://192.168.1.5:8080/v1");
        assert_eq!(endpoint_url("localhost:20129"), "http://localhost:20129/v1");
    }

    fn level_subject(n: &Value) -> (&str, &str) {
        (n["level"].as_str().unwrap(), n["subject"].as_str().unwrap())
    }

    /// One of every kind `check` prints after `unified models:`, in print order, with the page
    /// and level research R5 gives it.
    #[test]
    fn every_line_kind_has_its_level_and_page() {
        let home = Path::new("/h");
        let report = LoadReport {
            pending_conflicts: vec![PluginConflict { id: "kiro".into(), path: "/h/plugins/kiro.toml".into() }],
            declined: vec![PluginConflict { id: "kiro".into(), path: "/h/plugins/kiro.toml".into() }],
            withheld_credentials: vec![WithheldCredential {
                provider: "gemini-cli".into(),
                offending_url: "https://evil.example/token".parse().unwrap(),
            }],
            logos_ignored: vec![IgnoredLogo { id: "crush".into(), reason: "2700 × 1392 px, over 256 px".into() }],
            dropped_unified_models: vec![DroppedUnifiedModel { name: "lost".into(), provider: "broken".into() }],
            notes: vec![LimitsNote {
                unified: "mixed".into(),
                limit: "context_length".into(),
                values: vec![("a".into(), Some(1)), ("b".into(), None)],
            }],
            ..LoadReport::default()
        };
        let journal = json!({ "kept": false, "since": "2026-10-03T14:00:00Z", "unkept_requests": 3 });
        let modes = [
            ("/h/keys.toml", false),
            ("/h/tokens.toml", true),
            ("/h/quota/xai", false),
            ("/h/config.toml", false),
            ("/elsewhere/file", false),
        ]
        .map(|(path, fatal)| ModeFinding { path: path.into(), mode: 0o644, expected: 0o600, fatal });
        let signin = SigninReport {
            errors: vec!["accounts.toml: bad".into()],
            orphan_tokens: vec![("xai".into(), "orphan".into())],
            tokenless: vec![("xai".into(), "fresh".into())],
            modes: modes.into(),
        };
        let got = notices(
            home,
            &report,
            Some(&journal),
            &Findings {
                unmetered: &[("grok-cli".into(), "prepaid".into())],
                routing: &["anthropic/main is pay-as-you-go".into()],
                proxies: &ProxyFindings::default(),
            },
            &signin,
            None,
        );
        let kinds: Vec<(&str, &str, &str)> = got
            .iter()
            .map(|n| {
                let (level, subject) = level_subject(n);
                (level, subject, n["text"].as_str().unwrap())
            })
            .collect();
        let starts = [
            ("warning", "providers", "conflict pending: /h/plugins/kiro.toml shadows bundled kiro;"),
            ("warning", "providers", "declined: /h/plugins/kiro.toml (bundled kiro stays active)"),
            ("warning", "providers", "credential WITHHELD: OAuth for gemini-cli will not work"),
            ("note", "providers", "note: logo ignored: crush: 2700 × 1392 px, over 256 px"),
            ("error", "combo", "dropped unified model lost: member provider broken was skipped"),
            ("note", "combo", "note: unified model mixed: members differ in context_length: a 1, b undeclared"),
            ("warning", "usage", "warning: records not kept since 2026-10-03T14:00:00Z (disk full): 3 requests"),
            ("note", "quota", "note: grok-cli reports window prepaid, which no [[routing.window]] meter names;"),
            ("warning", "quota", "warning: anthropic/main is pay-as-you-go"),
            ("error", "quota", "error: accounts.toml: bad"),
            ("warning", "endpoint", "warning: /h/keys.toml has mode 644"),
            ("error", "quota", "error: /h/tokens.toml has mode 644"),
            ("warning", "quota", "warning: /h/quota/xai has mode 644"),
            ("warning", "settings", "warning: /h/config.toml has mode 644"),
            ("warning", "settings", "warning: /elsewhere/file has mode 644"),
            ("warning", "quota", "warning: tokens.toml has tokens for xai/orphan,"),
            (
                "warning",
                "quota",
                "warning: sign-in account xai/fresh has no tokens and can't serve; run `nullrouter accounts signin xai fresh`",
            ),
        ];
        assert_eq!(kinds.len(), starts.len(), "{kinds:#?}");
        for ((level, subject, text), (want_level, want_subject, prefix)) in kinds.iter().zip(starts) {
            assert_eq!((*level, *subject), (want_level, want_subject), "{text}");
            assert!(text.starts_with(prefix), "{text:?} should start with {prefix:?}");
        }
    }

    /// `dashboard.toml` with a loose mode is an error `serve` refuses to start with, on Settings.
    #[test]
    fn a_shared_dashboard_file_is_a_settings_error() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let token = files::DashboardToken {
            digest: Some(files::DashboardToken::digest_of("nrd_x")),
            issued: Some("2026-10-05T11:50:00Z".into()),
        };
        token.save(dir.path()).unwrap();
        let home = OperatorHome::new(dir.path());
        let clean = build(&home, &json!({}), &Live::none()).unwrap();
        assert!(!clean.json["notices"].to_string().contains("dashboard.toml"), "0600 is quiet");

        std::fs::set_permissions(dir.path().join(files::DASHBOARD_FILE), std::fs::Permissions::from_mode(0o644))
            .unwrap();
        let view = build(&home, &json!({}), &Live::none()).unwrap();
        let n = view.json["notices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["text"].as_str().unwrap().contains("dashboard.toml"))
            .expect("a finding for dashboard.toml");
        assert_eq!(level_subject(n), ("error", "settings"));
        assert!(n["text"].as_str().unwrap().contains("has mode 644, expected 0600; run `chmod 600"), "{n}");
    }

    #[test]
    fn a_file_outside_the_known_pages_goes_to_settings() {
        let home = Path::new("/h");
        for (path, subject) in [
            ("/h/accounts.toml", "quota"),
            ("/h/routing/anthropic/main.toml", "quota"),
            ("/h/keys.toml", "endpoint"),
            ("/h/dashboard.toml", "settings"),
            ("/h/records/2026-10-01.jsonl", "settings"),
            ("/h/something-new", "settings"),
            ("/not-the-home/keys.toml", "settings"),
        ] {
            assert_eq!(mode_subject(home, Path::new(path)), subject, "{path}");
        }
    }

    /// The `dashboard()` home: one notice per line, grouped as `check` prints them, and a skipped
    /// plugin's indented error lines joined to its `skipped:` line with `\n`.
    #[test]
    fn the_dashboard_home_has_a_notice_per_printed_line() {
        let dir = homes::dashboard();
        let home = OperatorHome::new(dir.path());
        let view = build(&home, &json!({}), &Live::none()).unwrap();
        let got = view.json["notices"].as_array().unwrap();
        for n in got {
            let (level, subject) = level_subject(n);
            assert!(["error", "warning", "note"].contains(&level), "{n}");
            assert!(SUBJECTS.contains(&subject), "{n}");
        }
        let broken = dir.path().join("plugins/broken.toml");
        let skipped = got.iter().find(|n| n["text"].as_str().unwrap().starts_with("skipped: ")).unwrap();
        let lines: Vec<&str> = skipped["text"].as_str().unwrap().split('\n').collect();
        assert_eq!(lines[0], format!("skipped: {}", broken.display()));
        assert!(lines.len() > 1 && lines[1..].iter().all(|l| l.starts_with("  ")), "{lines:?}");
        assert_eq!(level_subject(skipped), ("error", "providers"));

        // The text kinds in print order; each kind's subject.
        let order = [
            ("conflict pending: ", "providers"),
            ("declined: ", "providers"),
            ("skipped: ", "providers"),
            ("note: logo ignored: pixel: not a PNG", "providers"),
            ("dropped unified model ", "combo"),
            ("note: unified model ", "combo"),
            ("note: grok-cli reports window ", "quota"),
            ("warning: anthropic/spare is pay-as-you-go", "quota"),
            ("warning: ", "settings"), // records/2026-10-01.jsonl, mode 644
            ("warning: tokens.toml has tokens for", "quota"),
            ("warning: sign-in account ", "quota"),
        ];
        let mut at = 0;
        for (prefix, subject) in order {
            let pos = got[at..]
                .iter()
                .position(|n| n["text"].as_str().unwrap().starts_with(prefix) && n["subject"] == subject)
                .unwrap_or_else(|| panic!("no {prefix:?} for {subject} after notice {at}: {got:#?}"));
            at += pos + 1;
        }
    }

    /// A running server whose dashboard couldn't bind is a warning on Settings; a bound one, a
    /// disabled one, and no server are quiet.
    #[test]
    fn a_dashboard_that_could_not_bind_is_a_settings_warning() {
        let dir = homes::empty();
        let home = OperatorHome::new(dir.path());
        let with = |dashboard: Value| {
            let mut live = Live { running: true, ..Live::none() };
            live.answers.insert(
                "server.status",
                Some(json!({"ok": true, "client_listen": "127.0.0.1:20129", "dashboard": dashboard})),
            );
            build(&home, &json!({}), &live).unwrap().json["notices"].to_string()
        };
        let failed = with(json!({"enabled": true, "listen": "127.0.0.1:20130", "serving": false,
                                 "error": "127.0.0.1:20130: Address already in use (os error 98)"}));
        assert!(
            failed.contains(r#""subject":"settings","text":"warning: dashboard not listening: 127.0.0.1:20130: Address already in use (os error 98)""#)
                || failed.contains("warning: dashboard not listening: 127.0.0.1:20130: Address already in use"),
            "{failed}"
        );
        for quiet in [
            json!({"enabled": true, "listen": "127.0.0.1:20130", "serving": true, "error": null}),
            json!({"enabled": false, "listen": "127.0.0.1:20130", "serving": false, "error": null}),
        ] {
            assert!(!with(quiet).contains("dashboard not listening"));
        }
        let stopped = build(&home, &json!({}), &Live::none()).unwrap().json["notices"].to_string();
        assert!(!stopped.contains("dashboard not listening"), "{stopped}");
    }
}
