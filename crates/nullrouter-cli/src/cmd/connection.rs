//! `nullrouter connection` (spec 013, contracts/cli.md): the effective timeouts of each
//! provider and where each came from, and `set`/`unset` for them in `config.toml`.
//!
//! The retry keys `retries[.STATUS]` and `retry-wait[.STATUS]` edit `[provider.P.retry]`, where
//! a status (or `all`) holds one rule `{ retries, delay_ms }`: unsetting either key removes the
//! rule, and `retry-wait` needs the rule's `retries` to be set first.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;
use nullrouter_registry::OperatorHome;
use nullrouter_registry::schema::{OperatorConfig, parse_duration};
use nullrouter_server::ops::config_text::{edit, write_checked};
use nullrouter_server::operator::{self, CallError};
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
            let said = change(&home, &provider, model.as_deref(), &key, Some(&value)).map_err(fail)?;
            println!("{said}");
        }
        Command::Unset { provider, model, key } => {
            let said = change(&home, &provider, model.as_deref(), &key, None).map_err(fail)?;
            println!("{said}");
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

/// The config key for a CLI key.
fn config_key(key: &str) -> Result<&'static str, String> {
    Ok(match key {
        "connect-timeout" => "connect_timeout_ms",
        "header-timeout" => "header_timeout_ms",
        "first-token-timeout" => "first_token_timeout_ms",
        "stall-timeout" => "stall_timeout_ms",
        "reuse" => "reuse",
        "http2" => "http2",
        _ => {
            return Err(format!(
                "unknown key {key:?}; allowed: connect-timeout, header-timeout, first-token-timeout, stall-timeout, reuse, http2"
            ));
        }
    })
}

/// Which half of a retry rule a key sets.
#[derive(Clone, Copy, PartialEq)]
enum RetryField {
    Retries,
    Wait,
}

/// `retries`, `retries.503`, `retry-wait` and `retry-wait.503` as the field and the table key
/// (`all` or the status). `None` for any other key.
fn retry_key(key: &str) -> Option<Result<(RetryField, String), String>> {
    let (name, status) = key.split_once('.').map_or((key, None), |(n, s)| (n, Some(s)));
    let field = match name {
        "retries" => RetryField::Retries,
        "retry-wait" => RetryField::Wait,
        _ => return None,
    };
    Some(match status {
        None => Ok((field, "all".into())),
        Some(s) if s.len() == 3 && s.bytes().all(|b| b.is_ascii_digit()) => Ok((field, s.into())),
        Some(s) => Err(format!("{key}: {s:?} is not a 3-digit HTTP status")),
    })
}

/// `text` with the retry rule for `status` changed in `[provider.P.retry]`.
fn edit_retry(
    text: &str,
    provider: &str,
    (field, status): (RetryField, &str),
    value: Option<&str>,
) -> Result<(String, bool), String> {
    let header = format!("provider.{provider}.retry");
    let table_key = if status == "all" { "all".to_owned() } else { format!("\"{status}\"") };
    let Some(v) = value else { return Ok(edit(text, &header, &table_key, None)) };
    let config: OperatorConfig = toml::from_str(text).map_err(|e| format!("config.toml: {e}"))?;
    let retry = config.provider.get(provider).map(|p| &p.retry);
    let current = retry.and_then(|r| if status == "all" { r.all } else { r.by_status.get(status).copied() });
    let (retries, delay_ms) = match field {
        RetryField::Retries => {
            let n: u32 = v.parse().map_err(|_| format!("retries takes a count 0-5, not {v:?}"))?;
            (n, current.map_or(0, |c| c.delay_ms))
        }
        RetryField::Wait => {
            let Some(c) = current else {
                return Err(format!("set retries{} first; a wait needs a retry count", dotted(status)));
            };
            let ms = parse_duration(v).map_err(|e| format!("retry-wait: {e}"))?.as_millis();
            (c.retries, u64::try_from(ms).map_err(|_| format!("retry-wait: {v} is too long"))?)
        }
    };
    Ok(edit(text, &header, &table_key, Some(&format!("{{ retries = {retries}, delay_ms = {delay_ms} }}"))))
}

fn dotted(status: &str) -> String {
    if status == "all" { String::new() } else { format!(".{status}") }
}

/// `reuse` and `http2` are switches kept for a provider, never a model.
fn is_switch(key: &str) -> bool {
    matches!(key, "reuse" | "http2")
}

/// `on` or `off`, as the TOML boolean it is written as.
fn parse_switch(key: &str, value: &str) -> Result<String, String> {
    match value {
        "on" => Ok("true".into()),
        "off" => Ok("false".into()),
        _ => Err(format!("{key} takes on or off, not {value:?}")),
    }
}

/// A timeout in ms from `500ms`, `30s`, `5m`; `off` is 0 and only the first token takes it.
fn parse_value(key: &str, value: &str) -> Result<u64, String> {
    if value == "off" {
        return if key == "first-token-timeout" {
            Ok(0)
        } else {
            Err(format!("{key} can't be off; give a duration such as 30s"))
        };
    }
    let ms = parse_duration(value).map_err(|e| format!("{key}: {e}"))?.as_millis();
    u64::try_from(ms).map_err(|_| format!("{key}: {value} is too long"))
}

/// Changes (`Some`) or removes (`None`) one setting in `config.toml`, then asks the server to
/// reload. Nothing is written for an unknown provider or a value the config would refuse.
fn change(
    home: &OperatorHome,
    provider: &str,
    model: Option<&str>,
    key: &str,
    value: Option<&str>,
) -> Result<String, String> {
    let retry = retry_key(key).transpose()?;
    if retry.is_some() && model.is_some() {
        return Err(format!("{key} is set for a provider, not a model"));
    }
    let ckey = if retry.is_some() { "" } else { config_key(key)? };
    if is_switch(key) && model.is_some() {
        return Err(format!("{key} is set for a provider, not a model"));
    }
    let new = match value {
        _ if retry.is_some() => None,
        Some(v) if is_switch(key) => Some(parse_switch(key, v)?),
        Some(v) => Some(parse_value(key, v)?.to_string()),
        None => None,
    };
    let handle = nullrouter_server::views::open_registry(home).map_err(|e| e.message)?;
    let snapshot = handle.snapshot();
    let known: Vec<&str> = snapshot.providers().map(|p| p.id.as_str()).collect();
    if !known.contains(&provider) {
        return Err(format!("no provider {provider:?}; known providers: {}", known.join(", ")));
    }
    let header = match model {
        Some(m) => format!("provider.{provider}.model.\"{}\".connection", m.replace('\\', "\\\\").replace('"', "\\\"")),
        None => format!("provider.{provider}.connection"),
    };
    let path = home.config_file();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let what = format!("{provider}{} {key}", model.map_or(String::new(), |m| format!(" model {m}")));
    let (edited, changed) = match &retry {
        Some((field, status)) => edit_retry(&text, provider, (*field, status.as_str()), value)?,
        None => edit(&text, &header, ckey, new.as_deref()),
    };
    if !changed {
        return Ok(format!("{what}: was not set"));
    }
    write_checked(home, &edited)?;
    let status = crate::cmd::apply(home)?;
    Ok(match value {
        Some(v) => format!("{what} = {v}: {status}"),
        None => format!("{what} unset: {status}"),
    })
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

    #[test]
    fn values_are_durations_and_only_first_token_may_be_off() {
        assert_eq!(parse_value("header-timeout", "500ms"), Ok(500));
        assert_eq!(parse_value("header-timeout", "30s"), Ok(30_000));
        assert_eq!(parse_value("stall-timeout", "5m"), Ok(300_000));
        assert_eq!(parse_value("first-token-timeout", "off"), Ok(0));
        assert!(parse_value("header-timeout", "off").unwrap_err().contains("can't be off"));
        assert!(parse_value("header-timeout", "soon").unwrap_err().contains("header-timeout"));
        assert!(config_key("retries").unwrap_err().contains("allowed"));
    }

    fn home_with_config(config: &str) -> (tempfile::TempDir, OperatorHome) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), config).unwrap();
        let home = OperatorHome::new(dir.path());
        (dir, home)
    }

    #[test]
    fn an_unknown_provider_is_refused_with_the_known_ones_and_nothing_is_written() {
        let (dir, home) = home_with_config("schema = 1\n");
        let e = change(&home, "nope", None, "header-timeout", Some("10s")).unwrap_err();
        assert!(e.contains("no provider \"nope\"") && e.contains("known providers:"), "{e}");
        assert_eq!(std::fs::read_to_string(dir.path().join("config.toml")).unwrap(), "schema = 1\n");
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
