//! Unified models and combos: `unified add|set|remove` and `combos add|set|remove`. They edit
//! the `[[unified_model]]` and `[[combo]]` blocks of `config.toml` in place, keep every other
//! line as written, and save only what the loader would accept. The CLI and the dashboard share
//! these.

use nullrouter_registry::schema::OperatorConfig;
use serde_json::json;

use super::accounts::{Report, fail, refused};
use super::config_text::{quoted, write_checked};
use super::{Ctx, Done, OpError, Status, apply};

/// Which kind of routing target an operation is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Unified,
    Combo,
}

impl Kind {
    /// The array of tables in `config.toml`.
    fn table(self) -> &'static str {
        match self {
            Self::Unified => "unified_model",
            Self::Combo => "combo",
        }
    }
}

fn fail_in(field: &'static str, e: impl std::fmt::Display) -> OpError {
    OpError { text: e.to_string(), exit: 1, field: Some(field) }
}

/// Where the block `[[table]]` with `name = "<name>"` is: its first line, and one past its last
/// non-blank line.
fn find_block(lines: &[&str], table: &str, name: &str) -> Option<(usize, usize)> {
    let header = format!("[[{table}]]");
    let is_header = |l: &str| l.trim_start().starts_with('[');
    let mut at = 0;
    while at < lines.len() {
        if lines[at].trim() != header {
            at += 1;
            continue;
        }
        let next = (at + 1..lines.len()).find(|&i| is_header(lines[i])).unwrap_or(lines.len());
        let end = (at..next).rev().find(|&i| !lines[i].trim().is_empty()).map_or(at + 1, |i| i + 1);
        let block = lines[at..end].join("\n");
        let named = toml::from_str::<toml::Table>(&block)
            .ok()
            .and_then(|t| t.get(table)?.as_array()?.first()?.get("name")?.as_str().map(str::to_owned));
        if named.as_deref() == Some(name) {
            return Some((at, end));
        }
        at = next;
    }
    None
}

/// `text` with the block named `name` in `[[table]]` replaced by `block`, removed (`None`), or,
/// when there is none, appended. Every other line is kept as written. The flag is whether
/// anything changed.
pub fn edit_block(text: &str, table: &str, name: &str, block: Option<&str>) -> (String, bool) {
    let mut lines: Vec<&str> = text.lines().collect();
    let found = find_block(&lines, table, name);
    let new_lines: Vec<&str> = block.map(|b| b.lines().collect()).unwrap_or_default();
    match (found, block) {
        (Some((start, end)), Some(_)) => {
            lines.splice(start..end, new_lines);
        }
        (Some((start, end)), None) => {
            // The blank line that followed goes with the block.
            let upto = if lines.get(end).is_some_and(|l| l.trim().is_empty()) { end + 1 } else { end };
            lines.drain(start..upto);
            // A block that was last leaves no separator behind.
            if start >= lines.len() {
                while lines.last().is_some_and(|l| l.trim().is_empty()) {
                    lines.pop();
                }
            }
        }
        (None, None) => return (text.to_owned(), false),
        (None, Some(_)) => {
            while lines.last().is_some_and(|l| l.trim().is_empty()) {
                lines.pop();
            }
            if !lines.is_empty() {
                lines.push("");
            }
            lines.extend(new_lines);
        }
    }
    let mut out = lines.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    (out, true)
}

/// The TOML block for a unified model (`kind` is kept across a `set`).
fn unified_block(name: &str, kind: Option<&str>, members: &[(String, String)]) -> String {
    let mut out = format!("[[unified_model]]\nname = \"{}\"\n", quoted(name));
    if let Some(k) = kind {
        out.push_str(&format!("kind = \"{}\"\n", quoted(k)));
    }
    out.push_str("members = [\n");
    for (p, m) in members {
        out.push_str(&format!("  {{ provider = \"{}\", model = \"{}\" }},\n", quoted(p), quoted(m)));
    }
    out.push_str("]\n");
    out
}

fn combo_block(name: &str, members: &[String]) -> String {
    let list: Vec<String> = members.iter().map(|m| format!("\"{}\"", quoted(m))).collect();
    format!("[[combo]]\nname = \"{}\"\nmembers = [{}]\n", quoted(name), list.join(", "))
}

fn read_config(ctx: &Ctx) -> Result<String, OpError> {
    let path = ctx.home.config_file();
    match std::fs::read_to_string(&path) {
        Ok(t) => Ok(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(fail(format!("{}: {e}", path.display()))),
    }
}

fn parse(text: &str) -> Result<OperatorConfig, OpError> {
    toml::from_str(text).map_err(|e| fail(format!("config.toml does not load: {e}")))
}

/// What a change is: add a new name, set the members of one, or remove one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Change {
    Add,
    Set,
    Remove,
}

/// The combos listing `name` as a member, other than `name` itself.
fn used_by(config: &OperatorConfig, name: &str) -> Option<String> {
    config.combo.iter().find(|c| c.name != name && c.members.iter().any(|m| m == name)).map(|c| c.name.clone())
}

fn change(
    ctx: &Ctx,
    log: &mut Vec<String>,
    kind: Kind,
    op: Change,
    name: &str,
    members: &[String],
) -> Result<Report, OpError> {
    let text = read_config(ctx)?;
    let config = parse(&text)?;
    let of_kind = match kind {
        Kind::Unified => config.unified_model.iter().any(|u| u.name == name),
        Kind::Combo => config.combo.iter().any(|c| c.name == name),
    };
    let taken =
        of_kind || config.unified_model.iter().any(|u| u.name == name) || config.combo.iter().any(|c| c.name == name);
    match op {
        Change::Add if taken => return Err(fail_in("name", format!("{name}: already declared; use set"))),
        Change::Set | Change::Remove if !of_kind => {
            let hint = if op == Change::Set { "; use add" } else { "" };
            return Err(fail_in("name", format!("{name}: not declared{hint}")));
        }
        _ => {}
    }

    let (block, count) = match op {
        Change::Remove => {
            if let Some(c) = used_by(&config, name) {
                return Err(fail_in("name", format!("{name}: combo {c} uses it; remove it from the combo first")));
            }
            (None, 0)
        }
        _ if members.is_empty() => return Err(fail_in("members", format!("{name}: members must not be empty"))),
        _ => match kind {
            Kind::Unified => {
                let parsed: Vec<(String, String)> = members
                    .iter()
                    .map(|m| match m.split_once('/') {
                        Some((p, model)) if !p.is_empty() && !model.is_empty() => Ok((p.to_owned(), model.to_owned())),
                        _ => Err(fail_in("members", format!("{name}: member {m:?} must be provider/model"))),
                    })
                    .collect::<Result<_, _>>()?;
                let kept = config.unified_model.iter().find(|u| u.name == name).and_then(|u| u.kind);
                (Some(unified_block(name, kept.map(|k| k.to_string()).as_deref(), &parsed)), parsed.len())
            }
            Kind::Combo => {
                let known = |m: &str| {
                    m != name
                        && (config.unified_model.iter().any(|u| u.name == m)
                            || config.combo.iter().any(|c| c.name == m))
                };
                if let Some(m) = members.iter().find(|m| !known(m) && m.as_str() != name) {
                    return Err(fail_in(
                        "members",
                        format!("{name}: member {m} is neither a unified model nor a combo"),
                    ));
                }
                (Some(combo_block(name, members)), members.len())
            }
        },
    };

    let (edited, _) = edit_block(&text, kind.table(), name, block.as_deref());
    // The loader's own checks on the edited file, before anything is written.
    nullrouter_registry::check_config_text(&ctx.home, &edited).map_err(|errors| {
        let rules: Vec<String> = errors.iter().map(|e| format!("{name}: {}: {}", e.path, e.rule)).collect();
        fail_in("members", rules.join("\n"))
    })?;
    write_checked(&ctx.home, &edited).map_err(fail)?;

    let applied = match apply(ctx) {
        Ok(a) => a,
        Err(reason) => return Ok(refused(reason)),
    };
    for n in &applied.notes {
        log.push(format!("note: {n}"));
    }
    let status = applied.text;
    let plural = if count == 1 { "member" } else { "members" };
    let what = match op {
        Change::Add => format!("declared ({count} {plural})"),
        Change::Set => format!("updated ({count} {plural})"),
        Change::Remove => "removed".to_owned(),
    };
    let word = match op {
        Change::Add => "declared",
        Change::Set => "updated",
        Change::Remove => "removed",
    };
    Ok(Report {
        done: Done { status: Status::Applied, text: format!("{name}: {what}, {status}"), once: None },
        json: json!({"name": name, "kind": kind.table(), "result": word, "members": count, "status": status}),
        status,
    })
}

/// Declares a new unified model (`provider/model` members) or combo (member names).
pub fn add_with(
    ctx: &Ctx,
    log: &mut Vec<String>,
    kind: Kind,
    name: &str,
    members: &[String],
) -> Result<Report, OpError> {
    change(ctx, log, kind, Change::Add, name, members)
}

/// Replaces the members of a declared unified model or combo.
pub fn set_with(
    ctx: &Ctx,
    log: &mut Vec<String>,
    kind: Kind,
    name: &str,
    members: &[String],
) -> Result<Report, OpError> {
    change(ctx, log, kind, Change::Set, name, members)
}

/// Removes a unified model or combo that no combo lists.
pub fn remove_with(ctx: &Ctx, log: &mut Vec<String>, kind: Kind, name: &str) -> Result<Report, OpError> {
    change(ctx, log, kind, Change::Remove, name, &[])
}

/// The sentence the CLI's `[y/N]` prompt and the dashboard's confirm window show for a removal.
#[must_use]
pub fn remove_question(kind: Kind, name: &str) -> String {
    let op = if kind == Kind::Unified { "unified.remove" } else { "combos.remove" };
    super::describe_loss(op, name).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nullrouter_registry::OperatorHome;

    const TEXT: &str = "schema = 1\n\n# the fast one\n[[combo]]\nname = \"fast\"\nmembers = [\"u\"]\n\n[[unified_model]]\nname = \"u\"\nmembers = [\n  { provider = \"p\", model = \"m\" },\n]\n\n[server]\nlisten = \"x\"\n";

    #[test]
    fn a_block_is_replaced_removed_or_appended_and_the_rest_stays() {
        let (replaced, changed) =
            edit_block(TEXT, "combo", "fast", Some("[[combo]]\nname = \"fast\"\nmembers = [\"v\"]\n"));
        assert!(changed);
        assert_eq!(replaced, TEXT.replace("members = [\"u\"]", "members = [\"v\"]"));
        let (removed, _) = edit_block(TEXT, "unified_model", "u", None);
        assert_eq!(
            removed,
            "schema = 1\n\n# the fast one\n[[combo]]\nname = \"fast\"\nmembers = [\"u\"]\n\n[server]\nlisten = \"x\"\n"
        );
        let (appended, _) = edit_block(TEXT, "combo", "slow", Some("[[combo]]\nname = \"slow\"\nmembers = [\"u\"]\n"));
        assert!(appended.starts_with(TEXT.trim_end()));
        assert!(appended.ends_with("\n\n[[combo]]\nname = \"slow\"\nmembers = [\"u\"]\n"));
        assert_eq!(edit_block(TEXT, "combo", "nope", None), (TEXT.to_owned(), false));
        assert_eq!(edit_block("", "combo", "a", Some("[[combo]]\nname = \"a\"\n")).0, "[[combo]]\nname = \"a\"\n");
    }

    fn home() -> (tempfile::TempDir, Ctx) {
        let dir = tempfile::tempdir().unwrap();
        let ctx = Ctx { home: OperatorHome::new(dir.path()), engine: None };
        (dir, ctx)
    }

    fn members(m: &[&str]) -> Vec<String> {
        m.iter().map(|s| (*s).to_owned()).collect()
    }

    const G: &str = "grok-cli/grok-build";
    const B: &str = "anthropic/claude-3-5-sonnet-20241022";

    fn run(ctx: &Ctx, f: impl FnOnce(&Ctx, &mut Vec<String>) -> Result<Report, OpError>) -> Result<String, OpError> {
        f(ctx, &mut Vec::new()).map(|r| r.done.text)
    }

    fn config(ctx: &Ctx) -> String {
        std::fs::read_to_string(ctx.home.config_file()).unwrap_or_default()
    }

    #[test]
    fn add_set_and_remove_round_trip_and_keep_comments() {
        let (_d, ctx) = home();
        std::fs::write(ctx.home.config_file(), "schema = 1\n# mine\n[server]\nlisten = \"127.0.0.1:1\"\n").unwrap();
        let t = run(&ctx, |c, l| add_with(c, l, Kind::Unified, "u", &members(&[G, B]))).unwrap();
        assert_eq!(t, "u: declared (2 members), saved; applies at next start");
        assert!(
            config(&ctx).starts_with(
                "schema = 1\n# mine\n[server]\nlisten = \"127.0.0.1:1\"\n\n[[unified_model]]\nname = \"u\""
            )
        );
        run(&ctx, |c, l| add_with(c, l, Kind::Combo, "fast", &members(&["u"]))).unwrap();
        let t = run(&ctx, |c, l| set_with(c, l, Kind::Unified, "u", &members(&[G]))).unwrap();
        assert_eq!(t, "u: updated (1 member), saved; applies at next start");
        assert!(config(&ctx).contains("members = [\"u\"]"));
        run(&ctx, |c, l| remove_with(c, l, Kind::Combo, "fast")).unwrap();
        run(&ctx, |c, l| remove_with(c, l, Kind::Unified, "u")).unwrap();
        assert_eq!(config(&ctx), "schema = 1\n# mine\n[server]\nlisten = \"127.0.0.1:1\"\n");
    }

    #[test]
    fn refusals_name_the_reason_and_write_nothing() {
        let (_d, ctx) = home();
        run(&ctx, |c, l| add_with(c, l, Kind::Unified, "u", &members(&[G]))).unwrap();
        run(&ctx, |c, l| add_with(c, l, Kind::Combo, "fast", &members(&["u"]))).unwrap();
        let before = config(&ctx);
        let e = run(&ctx, |c, l| remove_with(c, l, Kind::Unified, "u")).unwrap_err();
        assert_eq!((e.text.as_str(), e.exit), ("u: combo fast uses it; remove it from the combo first", 1));
        let e = run(&ctx, |c, l| add_with(c, l, Kind::Combo, "c2", &members(&["x"]))).unwrap_err();
        assert_eq!(e.text, "c2: member x is neither a unified model nor a combo");
        assert_eq!(e.field, Some("members"));
        let e = run(&ctx, |c, l| add_with(c, l, Kind::Combo, "u", &members(&["u"]))).unwrap_err();
        assert_eq!(e.text, "u: already declared; use set");
        let e = run(&ctx, |c, l| set_with(c, l, Kind::Combo, "zz", &members(&["u"]))).unwrap_err();
        assert_eq!(e.text, "zz: not declared; use add");
        let e = run(&ctx, |c, l| remove_with(c, l, Kind::Unified, "zz")).unwrap_err();
        assert_eq!(e.text, "zz: not declared");
        let e = run(&ctx, |c, l| add_with(c, l, Kind::Unified, "v", &members(&["nope/m"]))).unwrap_err();
        assert!(e.text.contains("unknown provider"), "{}", e.text);
        let e = run(&ctx, |c, l| set_with(c, l, Kind::Combo, "fast", &members(&["fast"]))).unwrap_err();
        assert_eq!(e.field, Some("members"));
        assert_eq!(config(&ctx), before);
    }
}
