//! `nullrouter dashboard log` through the binary (spec 014 T040): newest first, `--limit`, `--json`,
//! and the empty message. The lines are written by the same `change_log::append` the dashboard uses.

use std::path::Path;
use std::process::{Command, Output};

use nullrouter_registry::OperatorHome;
use nullrouter_server::views::change_log::{self, Entry, Outcome};

fn nr(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nullrouter")).arg("--home").arg(home).args(args).output().unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).unwrap()
}

fn entry(i: usize, op: &str, subject: &str, result: Outcome, text: &str) -> Entry {
    Entry {
        at: format!("2026-10-22T14:03:{i:02}+02:00"),
        op: op.into(),
        subject: subject.into(),
        result,
        text: text.into(),
    }
}

/// Three changes, oldest first: a done one with no text, a refusal, and a saved-not-applied one.
fn three(home: &OperatorHome) -> Vec<Entry> {
    let all = vec![
        entry(0, "accounts.add", "anthropic/work0", Outcome::Done, ""),
        entry(1, "keys.revoke", "key-1", Outcome::Refused, "key-1: no such key"),
        entry(2, "proxy.add", "eu", Outcome::SavedNotApplied, "saved but not applied: reload refused"),
    ];
    for e in &all {
        change_log::append(home, e).unwrap();
    }
    all
}

#[test]
fn the_log_prints_newest_first_in_columns() {
    let dir = tempfile::tempdir().unwrap();
    let home = OperatorHome::new(dir.path());
    three(&home);
    let o = nr(dir.path(), &["dashboard", "log"]);
    assert_eq!(o.status.code(), Some(0));
    let text = out(&o);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{text}");
    // `<local time>  <op>  <subject>  <result>[: text]`; the time is `YYYY-MM-DD HH:MM:SS`.
    assert_eq!(&lines[0][4..5], "-");
    assert_eq!(&lines[0][19..21], "  ");
    let saved = "  proxy.add  eu  saved_not_applied: saved but not applied: reload refused";
    assert!(lines[0].ends_with(saved), "{}", lines[0]);
    assert!(lines[1].ends_with("  keys.revoke  key-1  refused: key-1: no such key"), "{}", lines[1]);
    assert!(lines[2].ends_with("  accounts.add  anthropic/work0  done"), "{}", lines[2]);
    assert!(!lines[2].contains("done:"), "{}", lines[2]);
}

#[test]
fn limit_keeps_the_newest() {
    let dir = tempfile::tempdir().unwrap();
    let home = OperatorHome::new(dir.path());
    three(&home);
    let text = out(&nr(dir.path(), &["dashboard", "log", "--limit", "2"]));
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{text}");
    assert!(lines[0].contains("  proxy.add  "), "{text}");
    assert!(lines[1].contains("  keys.revoke  "), "{text}");
    assert!(!text.contains("accounts.add"), "{text}");
}

#[test]
fn json_is_the_entries_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let home = OperatorHome::new(dir.path());
    let all = three(&home);
    let o = nr(dir.path(), &["--json", "dashboard", "log"]);
    assert_eq!(o.status.code(), Some(0));
    let j: Vec<Entry> = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(j, vec![all[2].clone(), all[1].clone(), all[0].clone()]);
    let raw: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(raw[0]["result"], "saved_not_applied");
    assert_eq!(raw[2]["result"], "done");
}

#[test]
fn an_empty_log_says_so_and_exits_0() {
    let dir = tempfile::tempdir().unwrap();
    let o = nr(dir.path(), &["dashboard", "log"]);
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(out(&o), "no changes made from the dashboard yet\n");
}
