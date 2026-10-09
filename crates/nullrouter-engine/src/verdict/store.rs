//! `routing/verdicts.jsonl` (data-model.md § On disk): one line per change, newest last; the
//! last line per pair (or per combo) wins.
//!
//! Appends go through the journal writer. This file renders the lines, replays the file at
//! start, and renders the whole board for a compaction.

use std::fs;
use std::path::Path;
use std::time::SystemTime;

use serde_json::{Value, json};

use super::{ComboVerdict, Pair, Verdict, Verdicts};

pub const FILE: &str = "verdicts.jsonl";
/// The line type of a pair's verdict.
pub const VERDICT: &str = "verdict";
/// The line type of a combo result.
pub const COMBO: &str = "combo";

/// What the file held.
#[derive(Debug, Default)]
pub struct Replay {
    pub verdicts: Verdicts,
    /// Lines read, malformed ones included.
    pub lines: usize,
    /// Lines skipped because they didn't parse (a torn last line after a crash, say).
    pub malformed: usize,
}

/// The fields of a set line (the writer adds `v` and `t`).
pub fn set_line(pair: &Pair, v: &Verdict) -> Value {
    let mut line = serde_json::to_value(pair).unwrap_or_default();
    if let (Some(line), Value::Object(fields)) = (line.as_object_mut(), serde_json::to_value(v).unwrap_or_default()) {
        line.extend(fields);
    }
    line
}

/// The fields of a line that returns `pair` to untested.
pub fn cleared_line(pair: &Pair, why: &str, at: SystemTime) -> Value {
    json!({
        "provider": pair.provider,
        "account": pair.account,
        "model": pair.model,
        "cleared": true,
        "why": why,
        "at": crate::clock::rfc3339(at),
    })
}

pub fn combo_line(name: &str, v: &ComboVerdict) -> Value {
    let mut line = json!({ "combo": name });
    if let (Some(line), Value::Object(fields)) = (line.as_object_mut(), serde_json::to_value(v).unwrap_or_default()) {
        line.extend(fields);
    }
    line
}

pub fn combo_cleared_line(name: &str, why: &str, at: SystemTime) -> Value {
    json!({ "combo": name, "cleared": true, "why": why, "at": crate::clock::rfc3339(at) })
}

/// Reads `routing/verdicts.jsonl` under `home`; a missing file is empty.
pub fn load(home: &Path) -> Replay {
    let text = fs::read_to_string(home.join("routing").join(FILE)).unwrap_or_default();
    let mut out = Replay::default();
    for raw in text.lines().filter(|l| !l.trim().is_empty()) {
        out.lines += 1;
        if apply(&mut out.verdicts, raw).is_none() {
            out.malformed += 1;
        }
    }
    out
}

fn apply(all: &mut Verdicts, raw: &str) -> Option<()> {
    let v: Value = serde_json::from_str(raw).ok()?;
    let cleared = v["cleared"] == true;
    match v["t"].as_str()? {
        VERDICT => {
            let pair: Pair = serde_json::from_value(v.clone()).ok()?;
            if cleared {
                all.remove(&pair);
            } else {
                all.insert(pair, serde_json::from_value(v).ok()?);
            }
        }
        COMBO => {
            let name = v["combo"].as_str()?.to_owned();
            if cleared {
                all.combos.remove(&name);
            } else {
                all.combos.insert(name, serde_json::from_value(v).ok()?);
            }
        }
        _ => return None,
    }
    Some(())
}

/// The whole board as file content, for a compaction: one set line per live entry.
pub fn render_all(all: &Verdicts) -> String {
    let mut out = String::new();
    let mut push = |t: &str, fields: Value| {
        let mut line = serde_json::Map::new();
        line.insert("v".into(), 1.into());
        line.insert("t".into(), t.into());
        if let Value::Object(f) = fields {
            line.extend(f);
        }
        out.push_str(&Value::Object(line).to_string());
        out.push('\n');
    };
    for (pair, v) in all.all() {
        push(VERDICT, set_line(&pair, v));
    }
    for (name, v) in &all.combos {
        push(COMBO, combo_line(name, v));
    }
    out
}
