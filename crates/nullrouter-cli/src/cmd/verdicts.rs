//! `nullrouter verdicts` (spec 011 contracts/cli.md § `nullrouter verdicts`): the model verdicts
//! per account and the combo results; the operator's `clear` and `mark`; the `[tests]` settings.
//!
//! `clear` and `mark` go through the running server (`verdicts.set`). With no server they open
//! the engine on the home and write `routing/verdicts.jsonl` themselves, so the verdict carries
//! the account's basis as a server would have written it. `settings` edits `config.toml` and asks
//! the server to reload, as `routing window` does.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, SystemTime};

use clap::{Args as ClapArgs, Subcommand};
use nullrouter_engine::clock;
use nullrouter_engine::state::Engine;
use nullrouter_engine::verdict::{self, Mark, Rejection};
use nullrouter_registry::OperatorHome;
use nullrouter_registry::schema::{OperatorConfig, TEST_TYPES, TestSettings, parse_broken_retest, parse_duration};
use nullrouter_server::operator::{self, CallError};
use nullrouter_server::views;
use serde_json::{Value, json};

/// `verdicts` lists; the subcommands change verdicts or settings.
#[derive(ClapArgs)]
#[command(args_conflicts_with_subcommands = true)]
pub(crate) struct Args {
    #[command(subcommand)]
    command: Option<Command>,
    /// Only this provider's verdicts.
    #[arg(long)]
    provider: Option<String>,
    /// Only this account's verdicts.
    #[arg(long)]
    account: Option<String>,
    /// Only this upstream model's verdicts.
    #[arg(long)]
    model: Option<String>,
    /// Only verdicts in this state.
    #[arg(long, value_parser = ["pass", "broken", "unknown"])]
    state: Option<String>,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Return a model to untested on one account.
    Clear { provider: String, account: String, model: String },
    /// Mark a model BROKEN on one account; routing stops sending it there and no retest runs.
    Mark {
        provider: String,
        account: String,
        model: String,
        /// Why, shown as `set by the operator: <note>`.
        #[arg(long)]
        note: Option<String>,
    },
    /// Show the `[tests]` settings, or set one: `retest 1m,5m,30m,6h`, `broken-retest 24h|on|off`,
    /// `timeout image 10m`, `concurrency 2`; `default` removes the setting.
    Settings {
        #[arg(num_args = 0..=3, value_name = "SETTING [TYPE] VALUE")]
        args: Vec<String>,
    },
}

fn fail(e: impl std::fmt::Display) -> ExitCode {
    eprintln!("{e}");
    ExitCode::from(1)
}

pub(crate) fn run(home: Option<PathBuf>, args: Args, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    match args.command {
        Some(Command::Clear { provider, account, model }) => {
            set(&home, [provider, account, model], Mark::Clear, as_json)
        }
        Some(Command::Mark { provider, account, model, note }) => {
            set(&home, [provider, account, model], Mark::Broken { note }, as_json)
        }
        Some(Command::Settings { args }) if args.is_empty() => show_settings(&home, as_json),
        Some(Command::Settings { args }) => set_setting(&home, &args),
        None => {
            let args =
                json!({"provider": args.provider, "account": args.account, "model": args.model, "state": args.state});
            let view = super::read(&home, views::verdicts::NEEDS, &args, views::verdicts::build)?;
            if as_json {
                println!("{:#}", view.json);
            } else {
                print!("{}", render(&view.json, clock::now()));
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// `clear` or `mark` through the server, or on the engine opened here with none running.
fn set(
    home: &OperatorHome,
    [provider, account, model]: [String; 3],
    mark: Mark,
    as_json: bool,
) -> Result<ExitCode, ExitCode> {
    let (state, note) = match &mark {
        Mark::Broken { note } => ("broken", note.clone()),
        Mark::Clear => ("clear", None),
    };
    let req = json!({"op": "verdicts.set", "provider": provider, "account": account, "model": model,
                     "state": state, "note": note});
    let status = match operator::call(home, &req) {
        Ok(a) if a["ok"] == true => "applied",
        Ok(a) => return Err(fail(a["error"].as_str().unwrap_or("the server refused the request"))),
        Err(CallError::NoServer(_)) => {
            let (engine, _) = Engine::open(home.clone()).map_err(|e| fail(format!("startup failed:\n{e}")))?;
            let done = verdict::mark(&engine, &provider, &account, &model, mark);
            engine.journal.flush_blocking();
            done.map_err(fail)?;
            "saved; applies at next start"
        }
        Err(e) => return Err(fail(e)),
    };
    if as_json {
        let out = json!({"provider": provider, "account": account, "model": model, "state": state, "status": status});
        println!("{out:#}");
    } else if state == "clear" {
        println!("{provider}/{account} {model}: untested: {status}");
    } else {
        println!("{provider}/{account} {model}: BROKEN, set by the operator: {status}");
    }
    Ok(ExitCode::SUCCESS)
}

/// `2026-10-07 09:12` from an RFC 3339 time (UTC).
fn minute(t: &str) -> String {
    t.get(..16).unwrap_or(t).replacen('T', " ", 1)
}

/// The verdict list and the combo results as text (contracts/cli.md).
pub(crate) fn render(view: &Value, now: SystemTime) -> String {
    let verdicts = view["verdicts"].as_array().map_or(&[][..], Vec::as_slice);
    let combos = view["combos"].as_array().map_or(&[][..], Vec::as_slice);
    if verdicts.is_empty() && combos.is_empty() {
        return "no verdicts; every model is untested and routes normally\n".to_owned();
    }
    let mut out = String::new();
    if !verdicts.is_empty() {
        let head = ["provider", "account", "model", "verdict", "since", "source", "reason / next"];
        let mut rows = vec![head.map(str::to_owned).to_vec()];
        for v in verdicts {
            let s = |k: &str| v[k].as_str().unwrap_or_default();
            let state = verdict::State::parse(s("state")).map_or("?", |st| st.label());
            rows.push(vec![
                s("provider").to_owned(),
                s("account").to_owned(),
                s("model").to_owned(),
                state.to_owned(),
                minute(s("at")),
                s("source").to_owned(),
                reason(v, now),
            ]);
        }
        for line in super::accounts::table(&rows) {
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
    if !combos.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        let mut rows = vec![["combo", "verdict", "since", "answered by", "next"].map(str::to_owned).to_vec()];
        for c in combos {
            let s = |k: &str| c[k].as_str().unwrap_or_default();
            let state = verdict::State::parse(s("state")).map_or("?", |st| st.label());
            rows.push(vec![
                s("combo").to_owned(),
                state.to_owned(),
                minute(s("at")),
                c["answered_by"].as_str().unwrap_or("—").to_owned(),
                next(c, now).unwrap_or_default(),
            ]);
        }
        for line in super::accounts::table(&rows) {
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
    out
}

/// `retest 09:43 (step 3)`, or `waiting: <why>` when the retest can't run yet.
fn next(v: &Value, now: SystemTime) -> Option<String> {
    if let Some(why) = v["waiting"].as_str() {
        return Some(format!("waiting: {why}"));
    }
    let at = v["next"].as_str()?;
    let when = match clock::parse_rfc3339(at) {
        Some(t) if t > now => minute(at)[11..].to_owned(),
        _ => "due".to_owned(),
    };
    let step = v["step"].as_u64().map(|s| format!(" (step {})", s + 1)).unwrap_or_default();
    Some(format!("retest {when}{step}"))
}

/// The reason column: what rejected the model, the provider's words, and the next retest.
fn reason(v: &Value, now: SystemTime) -> String {
    let text = v["reason"].as_str().unwrap_or_default();
    let text = match v["rejection"].as_str().and_then(Rejection::parse) {
        Some(r) => format!("{}: {text}", r.describe()),
        None => text.to_owned(),
    };
    match next(v, now) {
        Some(n) if n.starts_with("waiting") => format!("{text}; {n}"),
        Some(n) => format!("{text}; next {n}"),
        None => text,
    }
}

/// `1m`, `30s`, `6h`: the largest whole unit, as `config.toml` takes it.
fn written(d: Duration) -> String {
    match d.as_secs() {
        s if s != 0 && s % 3600 == 0 => format!("{}h", s / 3600),
        s if s != 0 && s % 60 == 0 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

/// `config.toml` as written, or empty when there is none.
fn config_text(home: &OperatorHome) -> Result<String, ExitCode> {
    let path = home.config_file();
    match std::fs::read_to_string(&path) {
        Ok(t) => Ok(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(fail(format!("{}: {e}", path.display()))),
    }
}

/// Each setting with its value now and its default, as `(name, value, default)`.
fn settings_rows(t: &TestSettings) -> Vec<(String, String, String)> {
    let d = TestSettings::default();
    let list = |v: &[Duration]| v.iter().map(|d| written(*d)).collect::<Vec<_>>().join(",");
    let broken = |v: Option<Duration>| v.map_or_else(|| "off".to_owned(), written);
    let mut rows = vec![
        ("retest".to_owned(), list(&t.retest), list(&d.retest)),
        ("broken-retest".to_owned(), broken(t.broken_retest), broken(d.broken_retest)),
        ("concurrency".to_owned(), t.concurrency.to_string(), d.concurrency.to_string()),
    ];
    let (mut now, mut default) = (t.timeout, d.timeout);
    for ty in TEST_TYPES {
        let (v, dv) = (now.get_mut(ty).copied(), default.get_mut(ty).copied());
        rows.push((format!("timeout {ty}"), written(v.unwrap_or_default()), written(dv.unwrap_or_default())));
    }
    rows
}

fn show_settings(home: &OperatorHome, as_json: bool) -> Result<ExitCode, ExitCode> {
    let text = config_text(home)?;
    let config: OperatorConfig = toml::from_str(&text).map_err(|e| fail(format!("config.toml: {e}")))?;
    let rows = settings_rows(&config.tests);
    if as_json {
        let all: serde_json::Map<String, Value> =
            rows.iter().map(|(k, v, d)| (k.clone(), json!({"value": v, "default": d}))).collect();
        println!("{:#}", Value::Object(all));
        return Ok(ExitCode::SUCCESS);
    }
    let mut table = vec![vec!["setting".to_owned(), "value".to_owned(), "default".to_owned()]];
    table.extend(rows.into_iter().map(|(k, v, d)| vec![k, v, d]));
    for line in super::accounts::table(&table) {
        println!("{}", line.trim_end());
    }
    Ok(ExitCode::SUCCESS)
}

const SETTINGS: &str = "retest <steps,…|default>, broken-retest <on|off|duration|default>, \
                        timeout <text|embedding|tts|stt|image|video> <duration|default>, concurrency <1-32|default>";

/// Sets one `[tests]` value in `config.toml`. A value that doesn't parse, or breaks a rule,
/// prints the rule and writes nothing.
fn set_setting(home: &OperatorHome, args: &[String]) -> Result<ExitCode, ExitCode> {
    let usage = || fail(format!("usage: verdicts settings [{SETTINGS}]"));
    // (table, key, TOML value or None for default, what to print)
    let (table, key, value, shown) = match args {
        [name, v] if name == "retest" => {
            let steps: Vec<&str> = v.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
            if v != "default" {
                for s in &steps {
                    parse_duration(s).map_err(|e| fail(format!("tests.retest: {s}: {e}")))?;
                }
            }
            let quoted: Vec<String> = steps.iter().map(|s| format!("\"{s}\"")).collect();
            let toml = (v != "default").then(|| format!("[{}]", quoted.join(", ")));
            ("[tests]", "retest", toml, v.clone())
        }
        [name, v] if name == "broken-retest" => {
            if v != "default" {
                parse_broken_retest(v).map_err(|rule| fail(format!("tests.broken_retest: {rule}")))?;
            }
            ("[tests]", "broken_retest", (v != "default").then(|| format!("\"{v}\"")), v.clone())
        }
        [name, v] if name == "concurrency" => {
            if v != "default" {
                v.parse::<u32>().map_err(|_| fail("tests.concurrency: 1 to 32"))?;
            }
            ("[tests]", "concurrency", (v != "default").then(|| v.clone()), v.clone())
        }
        [name, ty, v] if name == "timeout" => {
            if !TEST_TYPES.contains(&ty.as_str()) {
                return Err(fail(format!("tests.timeout: the type is one of {}", TEST_TYPES.join(", "))));
            }
            if v != "default" {
                parse_duration(v).map_err(|e| fail(format!("tests.timeout.{ty}: {e}")))?;
            }
            ("[tests.timeout]", ty.as_str(), (v != "default").then(|| format!("\"{v}\"")), format!("{ty} {v}"))
        }
        _ => return Err(usage()),
    };
    let path = home.config_file();
    let text = super::routing::edit_key(&config_text(home)?, table, key, value.as_deref());
    // The file must still load and keep the rules: a config the server would refuse is never written.
    let config: OperatorConfig =
        toml::from_str(&text).map_err(|e| fail(format!("{}: the edit would not load: {e}", path.display())))?;
    let broken = config.tests.check();
    if !broken.is_empty() {
        let lines: Vec<String> = broken.iter().map(|(at, rule)| format!("{at}: {rule}")).collect();
        return Err(fail(lines.join("\n")));
    }
    std::fs::create_dir_all(home.path()).map_err(fail)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, &text)
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|e| fail(format!("{}: {e}", path.display())))?;
    let status = super::apply(home).map_err(fail)?;
    let name = args[0].as_str();
    println!("{name} = {shown}: {status}");
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_reads_as_the_contract_shows() {
        let now = clock::parse_rfc3339("2026-10-07T09:20:00Z").unwrap();
        let view = json!({
            "verdicts": [
                {"provider": "anthropic", "account": "max", "model": "claude-opus-4-1", "state": "broken",
                 "rejection": "model_not_available", "reason": "403: not on your plan", "source": "test",
                 "at": "2026-10-07T09:12:03Z"},
                {"provider": "openrouter", "account": "main", "model": "x/y", "state": "unknown",
                 "reason": "503: overloaded", "source": "retest", "at": "2026-10-07T09:13:00Z",
                 "step": 2, "next": "2026-10-07T09:43:00Z"},
                {"provider": "xai", "account": "main", "model": "grok", "state": "unknown",
                 "reason": "timeout after 5 min", "source": "test", "at": "2026-10-07T08:00:00Z",
                 "step": 0, "next": "2026-10-07T08:01:00Z", "waiting": "needs sign-in"},
            ],
            "combos": [{"combo": "coder", "state": "pass", "answered_by": "glm", "at": "2026-10-07T09:20:00Z"}],
        });
        let text = render(&view, now);
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with("provider    account  model"), "{text}");
        assert!(lines[1].ends_with("  test    model not available: 403: not on your plan"), "{text}");
        assert!(lines[2].ends_with("503: overloaded; next retest 09:43 (step 3)"), "{text}");
        assert!(lines[3].ends_with("timeout after 5 min; waiting: needs sign-in"), "{text}");
        assert!(lines[2].contains("UNKNOWN  2026-10-07 09:13  retest"), "{text}");
        assert_eq!(lines[5], "combo  verdict  since             answered by  next");
        assert_eq!(lines[6], "coder  PASS     2026-10-07 09:20  glm");
        assert_eq!(render(&json!({"verdicts": [], "combos": []}), now).lines().count(), 1);
    }

    #[test]
    fn settings_show_values_beside_defaults() {
        let rows = settings_rows(&TestSettings::default());
        assert_eq!(rows[0], ("retest".into(), "1m,5m,30m,6h".into(), "1m,5m,30m,6h".into()));
        assert_eq!(rows[1].1, "off");
        assert!(rows.contains(&("timeout image".into(), "5m".into(), "5m".into())));
    }
}
