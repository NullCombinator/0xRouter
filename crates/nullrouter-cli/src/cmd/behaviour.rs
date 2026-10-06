//! `nullrouter behaviour` (contracts/operator-cli.md).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;
use nullrouter_registry::OperatorHome;
use nullrouter_registry::schema::{BreakBehaviour, OperatorConfig};
use nullrouter_server::views;
use serde_json::json;

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Show the operator's request-handling defaults and whether each is the default.
    Show,
    /// Operator default for a stream that breaks after output: `restart` or `error_event`.
    SetBreak { behaviour: String },
}

fn fail(e: impl std::fmt::Display) -> ExitCode {
    eprintln!("{e}");
    ExitCode::from(1)
}

pub(crate) fn run(home: Option<PathBuf>, cmd: Command, as_json: bool) -> Result<ExitCode, ExitCode> {
    let home = home.map_or_else(OperatorHome::resolve, OperatorHome::new);
    let behaviour = match cmd {
        Command::Show => return show(&home, as_json),
        Command::SetBreak { behaviour } => behaviour,
    };
    let b = BreakBehaviour::parse(&behaviour)
        .ok_or_else(|| fail(format!("unknown break behaviour {behaviour:?}; allowed: restart, error_event")))?;
    let path = home.config_file();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(fail(format!("{}: {e}", path.display()))),
    };
    let text = set_break(&text, b);
    // The file must still load: a config the server would refuse is never written.
    toml::from_str::<OperatorConfig>(&text)
        .map_err(|e| fail(format!("{}: the edit would not load: {e}", path.display())))?;
    std::fs::create_dir_all(home.path()).map_err(fail)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, &text)
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|e| fail(format!("{}: {e}", path.display())))?;
    let status = crate::cmd::apply(&home).map_err(fail)?;
    println!("break_behaviour = {}: {status}", b.as_str());
    Ok(ExitCode::SUCCESS)
}

fn show(home: &OperatorHome, as_json: bool) -> Result<ExitCode, ExitCode> {
    let view = super::read(home, views::behaviour::NEEDS, &json!({}), views::behaviour::build)?;
    if as_json {
        println!("{:#}", view.json);
        return Ok(ExitCode::SUCCESS);
    }
    let b = &view.json["break_behaviour"];
    let tag = if b["default"] == true { "  (default)" } else { "" };
    println!("break_behaviour  {}{tag}", b["value"].as_str().unwrap_or_default());
    Ok(ExitCode::SUCCESS)
}

/// `text` with `[pipeline] break_behaviour` set to `b`; every other line kept as written.
fn set_break(text: &str, b: BreakBehaviour) -> String {
    let line = format!("break_behaviour = \"{}\"", b.as_str());
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let header = |l: &str| l.trim_start().starts_with('[');
    let Some(start) = lines.iter().position(|l| l.trim() == "[pipeline]") else {
        let mut out = text.to_owned();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("[pipeline]\n");
        out.push_str(&line);
        out.push('\n');
        return out;
    };
    let end = lines[start + 1..].iter().position(|l| header(l)).map_or(lines.len(), |i| start + 1 + i);
    let key = |l: &str| l.split_once('=').is_some_and(|(k, _)| k.trim() == "break_behaviour");
    match (start + 1..end).find(|&i| key(&lines[i])) {
        Some(i) => lines[i] = line,
        None => lines.insert(start + 1, line),
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_is_added_replaced_or_given_a_table() {
        assert_eq!(set_break("", BreakBehaviour::ErrorEvent), "[pipeline]\nbreak_behaviour = \"error_event\"\n");
        let with = "schema = 1\n[pipeline]\n# note\nbreak_behaviour = \"restart\"\n[server]\nlisten = \"x\"\n";
        assert_eq!(
            set_break(with, BreakBehaviour::ErrorEvent),
            "schema = 1\n[pipeline]\n# note\nbreak_behaviour = \"error_event\"\n[server]\nlisten = \"x\"\n"
        );
        assert_eq!(
            set_break("[pipeline]\n[server]\n", BreakBehaviour::Restart),
            "[pipeline]\nbreak_behaviour = \"restart\"\n[server]\n"
        );
        assert_eq!(
            set_break("schema = 1", BreakBehaviour::Restart),
            "schema = 1\n\n[pipeline]\nbreak_behaviour = \"restart\"\n"
        );
    }
}
