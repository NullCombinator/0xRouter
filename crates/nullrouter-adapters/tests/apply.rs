use nullrouter_adapter_kit::{Edit, Edits, Kind, Op, Path, Reason};
use nullrouter_adapters::apply::{apply, changes, check, parse_output, Rule};
use nullrouter_adapters::selector::Selector;
use serde_json::json;

fn sels(v: &[&str]) -> Vec<Selector> {
    v.iter().map(|s| Selector::parse(s).unwrap()).collect()
}

fn p(s: &str) -> Path {
    Path::parse(s).unwrap()
}

fn body() -> serde_json::Value {
    json!({"messages": [{"role": "user", "content": ["a", "b", "c"], "x": 1}], "tools": []})
}

fn rule(edits: Vec<Edit>) -> Rule {
    check(&body(), &sels(&["messages[*].content[*]", "messages[*].x"]), &edits).unwrap_err().rule
}

fn rm(path: &str) -> Edit {
    let mut e = Edits::default();
    e.remove(&p(path), Reason::TargetRejectsField);
    e.edits.remove(0)
}

#[test]
fn valid_edits_pass() {
    check(&body(), &sels(&["messages[*].content[*]"]), &[rm("messages[0].content[1]")]).unwrap();
}

#[test]
fn path_outside_selector() {
    assert_eq!(rule(vec![rm("messages[0].role")]), Rule::OutsideSelector);
}

#[test]
fn path_missing() {
    assert_eq!(rule(vec![rm("messages[0].content[9]")]), Rule::PathMissing);
}

#[test]
fn overlap_and_prefix() {
    assert_eq!(rule(vec![rm("messages[0].content[1]"), rm("messages[0].content[1]")]), Rule::Overlap);
    let mut e = Edits::default();
    e.convert(&p("messages[0].content[1]"), json!({"t": 1}), Reason::FormatConversion);
    let conv = e.edits.remove(0);
    let s = sels(&["messages[*].content[*]", "messages[*].content[*].t"]);
    let err = check(&body(), &s, &[conv, rm("messages[0].content[1]")]).unwrap_err();
    assert_eq!(err.rule, Rule::Overlap);
}

#[test]
fn too_many_edits() {
    let b = json!({"a": (0..2000).collect::<Vec<_>>()});
    let edits: Vec<Edit> = (0..1025).map(|i| rm(&format!("a[{i}]"))).collect();
    assert_eq!(check(&b, &sels(&["a[*]"]), &edits).unwrap_err().rule, Rule::TooManyEdits);
    assert!(check(&b, &sels(&["a[*]"]), &edits[..1024]).is_ok());
}

#[test]
fn value_too_large() {
    let big = "x".repeat(4 * 1024 * 1024 + 1);
    let mut e = Edits::default();
    e.convert(&p("messages[0].x"), json!(big), Reason::FormatConversion);
    assert_eq!(rule(e.edits), Rule::ValueTooLarge);
}

#[test]
fn kind_mismatch() {
    let mut a = rm("messages[0].x");
    a.kind = Kind::Converted;
    assert_eq!(rule(vec![a]), Rule::KindMismatch);
    let b = Edit { op: Op::Replace, path: p("messages[0].x"), kind: Kind::Removed, reason: Reason::FormatConversion, value: Some(json!(1)) };
    assert_eq!(rule(vec![b]), Rule::KindMismatch);
    let c = Edit { op: Op::Replace, path: p("messages[0].x"), kind: Kind::Converted, reason: Reason::FormatConversion, value: None };
    assert_eq!(rule(vec![c]), Rule::KindMismatch);
}

#[test]
fn unknown_reason_and_bad_json() {
    let bad = br#"{"edits":[{"op":"remove","path":"a","kind":"removed","reason":"because"}]}"#;
    assert_eq!(parse_output(bad).unwrap_err().rule, Rule::UnknownReason);
    assert_eq!(parse_output(b"nope").unwrap_err().rule, Rule::NotJson);
    let ok = br#"{"edits":[{"op":"remove","path":"a","kind":"removed","reason":"foreign_block"}]}"#;
    assert_eq!(parse_output(ok).unwrap().edits.len(), 1);
}

#[test]
fn removals_apply_from_the_highest_index_and_leave_the_original() {
    let b = body();
    let out = apply(&b, &[rm("messages[0].content[0]"), rm("messages[0].content[2]")]);
    assert_eq!(out["messages"][0]["content"], json!(["b"]));
    assert_eq!(b, body());
}

#[test]
fn replace_and_changes_hold_no_value() {
    let mut e = Edits::default();
    e.convert(&p("messages[0].x"), json!("secret"), Reason::FormatConversion);
    let out = apply(&body(), &e.edits);
    assert_eq!(out["messages"][0]["x"], json!("secret"));
    let c = changes(&e.edits);
    let s = serde_json::to_string(&c).unwrap();
    assert_eq!(s, r#"[{"path":"messages[0].x","kind":"converted","reason":"format_conversion"}]"#);
}
