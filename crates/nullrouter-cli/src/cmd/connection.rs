//! `nullrouter connection` (spec 013, contracts/cli.md): the effective timeouts of each
//! provider and where each came from, and `set`/`unset` for them in `config.toml`.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;
use nullrouter_registry::OperatorHome;
use nullrouter_registry::schema::{OperatorConfig, parse_duration};
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
        /// connect-timeout, header-timeout, first-token-timeout or stall-timeout.
        key: String,
        /// A duration (`500ms`, `30s`, `5m`), or `off` for first-token-timeout.
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
        _ => {
            return Err(format!(
                "unknown key {key:?}; allowed: connect-timeout, header-timeout, first-token-timeout, stall-timeout"
            ));
        }
    })
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
    let ckey = config_key(key)?;
    let ms = value.map(|v| parse_value(key, v)).transpose()?;
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
    let (edited, changed) = edit(&text, &header, ckey, ms.map(|v| v.to_string()).as_deref());
    if !changed {
        return Ok(format!("{what}: was not set"));
    }
    // The file must still load: a config the server would refuse is never written.
    toml::from_str::<OperatorConfig>(&edited)
        .map_err(|e| format!("{}: the edit would not load: {e}", path.display()))?;
    std::fs::create_dir_all(home.path()).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, &edited)
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let status = crate::cmd::apply(home)?;
    Ok(match value {
        Some(v) => format!("{what} = {v}: {status}"),
        None => format!("{what} unset: {status}"),
    })
}

/// `text` with `key` set to `value` (or removed) in the table `[header]`; every other line is
/// kept as written. The flag is whether anything changed.
pub(crate) fn edit(text: &str, header: &str, key: &str, value: Option<&str>) -> (String, bool) {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let is_table = |l: &str| l.trim_start().starts_with('[');
    let is_key = |l: &str| l.split_once('=').is_some_and(|(k, _)| k.trim() == key);
    let start = lines.iter().position(|l| l.trim() == format!("[{header}]"));
    let join = |lines: Vec<String>| {
        let mut out = lines.join("\n");
        out.push('\n');
        out
    };
    let Some(start) = start else {
        let Some(v) = value else { return (text.to_owned(), false) };
        let mut out = text.to_owned();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() {
            out.push('\n');
        }
        let _ = write!(out, "[{header}]\n{key} = {v}\n");
        return (out, true);
    };
    let end = lines[start + 1..].iter().position(|l| is_table(l)).map_or(lines.len(), |i| start + 1 + i);
    let at = (start + 1..end).find(|&i| is_key(&lines[i]));
    match (at, value) {
        (Some(i), Some(v)) => lines[i] = format!("{key} = {v}"),
        (None, Some(v)) => {
            // After the table's last line that isn't blank.
            let after = (start + 1..end).rev().find(|&i| !lines[i].trim().is_empty()).map_or(start + 1, |i| i + 1);
            lines.insert(after, format!("{key} = {v}"));
        }
        (None, None) => return (text.to_owned(), false),
        (Some(i), None) => {
            lines.remove(i);
            let end = end - 1;
            // A table with nothing left goes too, with the blank line that followed it.
            if (start + 1..end).all(|j| lines[j].trim().is_empty()) {
                let upto = if lines.get(end).is_some_and(|l| l.trim().is_empty()) { end + 1 } else { end };
                lines.drain(start..upto.min(lines.len()));
            }
        }
    }
    (join(lines), true)
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

    const H: &str = "provider.acme.connection";

    #[test]
    fn a_setting_is_added_replaced_and_given_a_table() {
        assert_eq!(edit("", H, "header_timeout_ms", Some("10000")), ("[provider.acme.connection]\nheader_timeout_ms = 10000\n".into(), true));
        let with = "schema = 1\n[provider.acme.connection]\n# note\nheader_timeout_ms = 5\n\n[server]\nlisten = \"x\"\n";
        assert_eq!(
            edit(with, H, "header_timeout_ms", Some("9")).0,
            "schema = 1\n[provider.acme.connection]\n# note\nheader_timeout_ms = 9\n\n[server]\nlisten = \"x\"\n"
        );
        // A new key goes after the table's last line, ahead of the blank line before the next.
        assert_eq!(
            edit(with, H, "stall_timeout_ms", Some("7")).0,
            "schema = 1\n[provider.acme.connection]\n# note\nheader_timeout_ms = 5\nstall_timeout_ms = 7\n\n[server]\nlisten = \"x\"\n"
        );
        assert_eq!(
            edit("schema = 1", H, "stall_timeout_ms", Some("7")).0,
            "schema = 1\n\n[provider.acme.connection]\nstall_timeout_ms = 7\n"
        );
    }

    #[test]
    fn unsetting_the_last_key_removes_the_table_and_other_tables_stay() {
        let text = "schema = 1\n\n[provider.acme.connection]\nheader_timeout_ms = 5\n\n[server]\nlisten = \"x\"\n";
        assert_eq!(edit(text, H, "header_timeout_ms", None), ("schema = 1\n\n[server]\nlisten = \"x\"\n".into(), true));
        let two = "[provider.acme.connection]\nheader_timeout_ms = 5\nstall_timeout_ms = 7\n";
        assert_eq!(edit(two, H, "header_timeout_ms", None).0, "[provider.acme.connection]\nstall_timeout_ms = 7\n");
        assert_eq!(edit(two, H, "connect_timeout_ms", None), (two.into(), false));
        assert_eq!(edit("", H, "connect_timeout_ms", None), (String::new(), false));
    }

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
        let provider = nullrouter_server::views::open_registry(&home)
            .unwrap()
            .snapshot()
            .providers()
            .next()
            .unwrap()
            .id
            .clone();
        let said = change(&home, &provider, None, "header-timeout", Some("10s")).unwrap();
        assert!(said.ends_with("saved; applies at next start"), "{said}");
        change(&home, &provider, Some("some/model"), "first-token-timeout", Some("5m")).unwrap();
        let text = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        assert!(text.contains("header_timeout_ms = 10000") && text.contains("first_token_timeout_ms = 300000"), "{text}");

        let v = view(&home, Some(&provider)).unwrap();
        assert_eq!(v["providers"][0]["timeouts"]["headers"]["ms"], 10_000);
        let shown = render(&v);
        assert!(shown.contains("header timeout") && shown.contains("10 s") && shown.contains("operator, provider"), "{shown}");
        assert!(shown.contains("model some/model") && shown.contains("300 s") && shown.contains("operator, model"), "{shown}");

        change(&home, &provider, None, "header-timeout", None).unwrap();
        change(&home, &provider, Some("some/model"), "first-token-timeout", None).unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("config.toml")).unwrap().trim(), "schema = 1");
        let e = change(&home, &provider, None, "header-timeout", Some("0ms")).unwrap_err();
        assert!(e.contains("would not load") && e.contains("header_timeout_ms"), "{e}");
    }
}
