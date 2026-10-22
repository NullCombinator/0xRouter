//! `nullrouter connection` (spec 013, contracts/cli.md): the effective timeouts of each
//! provider and where each came from (`show`), and `set`/`unset` for them in `config.toml`.
//! The edits are the shared operations in `nullrouter_server::ops::connection`; this file
//! parses, prints and exits.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;
use nullrouter_registry::OperatorHome;
use nullrouter_server::operator::{self, CallError};
use nullrouter_server::ops::{self, Ctx, Done, OpError, Status};
use serde_json::{Value, json};

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Show each provider's timeouts and where each value came from.
    Show { provider: Option<String> },
    /// Set a timeout for a provider, or for one of its models.
    Set {
        provider: String,
        #[arg(long)]
        model: Option<String>,
        /// connect-timeout, header-timeout, first-token-timeout, stall-timeout, reuse, http2,
        /// retries or retry-wait (the last two take `.STATUS`, for example `retries.503`).
        key: String,
        /// A duration (`500ms`, `30s`, `5m`), `off` for first-token-timeout, `on`/`off` for
        /// reuse and http2, or a count 0-5 for retries.
        value: String,
    },
    /// Remove a setting, so the plugin's value or the built-in default applies again.
    Unset {
        provider: String,
        #[arg(long)]
        model: Option<String>,
        key: String,
    },
}

fn fail(e: impl std::fmt::Display) -> ExitCode {
    eprintln!("{e}");
    ExitCode::from(1)
}

/// Prints the notes, then the result: the text on stdout; the reason and exit 1 when the running
/// server refused the saved files; the error and its exit code otherwise.
fn report(r: Result<Done, OpError>, log: &[String]) -> Result<ExitCode, ExitCode> {
    for l in log {
        eprintln!("{l}");
    }
    let done = r.map_err(|e| {
        eprintln!("{}", e.text);
        ExitCode::from(e.exit)
    })?;
    if let Status::SavedNotApplied { reason } = &done.status {
        return Err(fail(reason));
    }
    println!("{}", done.text);
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn run(home: Option<PathBuf>, cmd: Command, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    match cmd {
        Command::Show { provider } => {
            let view = view(&home, provider.as_deref()).map_err(fail)?;
            if as_json {
                println!("{view:#}");
            } else {
                print!("{}", render(&view));
            }
        }
        Command::Set { provider, model, key, value } => {
            let ctx = Ctx { home, engine: None };
            let mut log = Vec::new();
            let r = ops::connection::set_with(&ctx, &mut log, &provider, model.as_deref(), &key, &value);
            report(r, &log)?;
        }
        Command::Unset { provider, model, key } => {
            let ctx = Ctx { home, engine: None };
            let mut log = Vec::new();
            let r = ops::connection::unset_with(&ctx, &mut log, &provider, model.as_deref(), &key);
            report(r, &log)?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The running server's answer, else the same view read from the files.
fn view(home: &OperatorHome, provider: Option<&str>) -> Result<Value, String> {
    let req = match provider {
        Some(p) => json!({"op": "connection.view", "provider": p}),
        None => json!({"op": "connection.view"}),
    };
    match operator::call(home, &req) {
        Ok(a) if a["ok"] == true => return Ok(a),
        Ok(a) => return Err(a["error"].as_str().unwrap_or("the server refused the request").to_owned()),
        Err(CallError::NoServer(_)) => {}
        Err(e) => return Err(e.to_string()),
    }
    let handle = nullrouter_server::views::open_registry(home).map_err(|e| e.message)?;
    nullrouter_engine::connection::view(&handle.snapshot(), provider)
}

fn source(s: &Value) -> String {
    let by = s["by"].as_str().unwrap_or_default();
    match (by, s["level"].as_str().unwrap_or_default()) {
        ("built_in", "env") => "built-in (environment)".into(),
        ("built_in", _) => "built-in".into(),
        (by, level) => format!("{by}, {level}"),
    }
}

fn duration(t: &Value) -> String {
    match t["ms"].as_u64() {
        None => "off".into(),
        Some(ms) if ms % 1000 == 0 => format!("{} s", ms / 1000),
        Some(ms) => format!("{ms} ms"),
    }
}

fn scrub(v: &Value) -> String {
    v.as_str().unwrap_or("-").chars().filter(|c| !c.is_control()).collect()
}

/// The `show` text for a `connection.view` answer.
fn render(view: &Value) -> String {
    let mut out = String::new();
    let line = |out: &mut String, indent: &str, name: &str, t: &Value| {
        let src = if t["ms"].is_null() { "built-in".to_owned() } else { source(&t["source"]) };
        let _ = writeln!(out, "{indent}{name:<20} {:<9} {src}", duration(t));
    };
    for p in view["providers"].as_array().into_iter().flatten() {
        let _ = writeln!(out, "{}", scrub(&p["id"]));
        for (key, name) in [
            ("connect", "connect timeout"),
            ("headers", "header timeout"),
            ("first_token", "first-token timeout"),
            ("stall", "stall timeout"),
        ] {
            line(&mut out, "  ", name, &p["timeouts"][key]);
        }
        let reuse = if p["reuse"]["value"] == false { "off" } else { "on" };
        let http2 = p["http2"]["value"].as_str().unwrap_or("negotiate");
        let _ = writeln!(out, "  {:<20} {:<9} {}", "reuse", reuse, source(&p["reuse"]["source"]));
        let _ = writeln!(out, "  {:<20} {:<9} {}", "http2", http2, source(&p["http2"]["source"]));
        for r in p["retry"].as_array().into_iter().flatten() {
            let what = format!("retry {}", scrub(&r["status"]));
            let rule = format!("{} x {} ms", r["retries"], r["delay_ms"]);
            let _ = writeln!(out, "  {what:<20} {rule:<9} {}", source(&r["source"]));
        }
        for m in p["models"].as_array().into_iter().flatten() {
            let _ = writeln!(out, "  model {}", scrub(&m["id"]));
            for (key, name) in [
                ("connect", "connect timeout"),
                ("headers", "header timeout"),
                ("first_token", "first-token timeout"),
                ("stall", "stall timeout"),
            ] {
                if m["timeouts"][key] != p["timeouts"][key] {
                    line(&mut out, "    ", name, &m["timeouts"][key]);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `set` or `unset` as the CLI runs it: the text it prints, or the refusal or error it prints
    /// on stderr.
    fn change(
        home: &OperatorHome,
        provider: &str,
        model: Option<&str>,
        key: &str,
        value: Option<&str>,
    ) -> Result<String, String> {
        let ctx = Ctx { home: home.clone(), engine: None };
        let mut log = Vec::new();
        let done = match value {
            Some(v) => ops::connection::set_with(&ctx, &mut log, provider, model, key, v),
            None => ops::connection::unset_with(&ctx, &mut log, provider, model, key),
        };
        match done {
            Ok(Done { status: Status::SavedNotApplied { reason }, .. }) => Err(reason),
            Ok(d) => Ok(d.text),
            Err(e) => Err(e.text),
        }
    }

    fn home_with_config(config: &str) -> (tempfile::TempDir, OperatorHome) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), config).unwrap();
        let home = OperatorHome::new(dir.path());
        (dir, home)
    }

    #[test]
    fn set_and_unset_round_trip_through_the_config_and_the_view() {
        let (dir, home) = home_with_config("schema = 1\n");
        let provider =
            nullrouter_server::views::open_registry(&home).unwrap().snapshot().providers().next().unwrap().id.clone();
        let said = change(&home, &provider, None, "header-timeout", Some("10s")).unwrap();
        assert!(said.ends_with("saved; applies at next start"), "{said}");
        change(&home, &provider, Some("some/model"), "first-token-timeout", Some("5m")).unwrap();
        let text = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        assert!(
            text.contains("header_timeout_ms = 10000") && text.contains("first_token_timeout_ms = 300000"),
            "{text}"
        );

        let v = view(&home, Some(&provider)).unwrap();
        assert_eq!(v["providers"][0]["timeouts"]["headers"]["ms"], 10_000);
        let shown = render(&v);
        assert!(
            shown.contains("header timeout") && shown.contains("10 s") && shown.contains("operator, provider"),
            "{shown}"
        );
        assert!(
            shown.contains("model some/model") && shown.contains("300 s") && shown.contains("operator, model"),
            "{shown}"
        );

        change(&home, &provider, None, "header-timeout", None).unwrap();
        change(&home, &provider, Some("some/model"), "first-token-timeout", None).unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("config.toml")).unwrap().trim(), "schema = 1");
        let e = change(&home, &provider, None, "header-timeout", Some("0ms")).unwrap_err();
        assert!(e.contains("would not load") && e.contains("header_timeout_ms"), "{e}");
    }

    #[test]
    fn retry_keys_edit_one_rule_per_status_and_show_with_their_source() {
        let (dir, home) = home_with_config("schema = 1\n");
        let provider =
            nullrouter_server::views::open_registry(&home).unwrap().snapshot().providers().next().unwrap().id.clone();
        let file = || std::fs::read_to_string(dir.path().join("config.toml")).unwrap();

        let e = change(&home, &provider, None, "retry-wait.503", Some("2s")).unwrap_err();
        assert!(e.contains("set retries.503 first"), "{e}");
        change(&home, &provider, None, "retries.503", Some("3")).unwrap();
        change(&home, &provider, None, "retry-wait.503", Some("2s")).unwrap();
        change(&home, &provider, None, "retries", Some("1")).unwrap();
        let text = file();
        assert!(text.contains("\"503\" = { retries = 3, delay_ms = 2000 }"), "{text}");
        assert!(text.contains("all = { retries = 1, delay_ms = 0 }"), "{text}");

        let shown = render(&view(&home, Some(&provider)).unwrap());
        assert!(
            shown.contains("retry 503") && shown.contains("3 x 2000 ms") && shown.contains("operator, provider"),
            "{shown}"
        );
        assert!(shown.contains("retry all"), "{shown}");

        let e = change(&home, &provider, None, "retries", Some("6")).unwrap_err();
        assert!(e.contains("would not load") && e.contains("0-5"), "{e}");
        assert!(change(&home, &provider, None, "retries.5xx", Some("1")).unwrap_err().contains("3-digit"));
        assert!(change(&home, &provider, Some("m"), "retries", Some("1")).unwrap_err().contains("not a model"));

        change(&home, &provider, None, "retries.503", None).unwrap();
        change(&home, &provider, None, "retries", None).unwrap();
        assert_eq!(file().trim(), "schema = 1");
    }
}
