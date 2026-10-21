//! Checking an adapter's edits (research R5) and applying them to a copy of the body.

use nullrouter_adapter_kit::{Edit, Edits, Kind, Op, Path, Reason, Seg};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::selector::{Selector, covers};

pub const MAX_EDITS: usize = 1024;
pub const MAX_VALUE_BYTES: usize = 4 * 1024 * 1024;

/// The named rule an invalid output broke (`invalid_output.rule` in the data model).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    NotJson,
    OutsideSelector,
    PathMissing,
    Overlap,
    TooManyEdits,
    ValueTooLarge,
    KindMismatch,
    UnknownReason,
    InputTooLarge,
    OutputTooLarge,
    Undecodable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid adapter output: {rule:?}")]
pub struct InvalidOutput {
    pub rule: Rule,
}

fn bad(rule: Rule) -> InvalidOutput {
    InvalidOutput { rule }
}

/// What a record keeps of an edit: no value, no preview, no length (FR-025).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentChange {
    pub path: String,
    pub kind: Kind,
    pub reason: Reason,
}

const REASONS: [&str; 8] = [
    "target_rejects_field",
    "target_cannot_carry_block",
    "foreign_block",
    "format_conversion",
    "param_unsupported_by_model",
    "empty_after_removal",
    "duplicate_tool",
    "role_not_accepted",
];

/// Parses an adapter's output JSON. An unknown reason code is its own rule.
pub fn parse_output(bytes: &[u8]) -> Result<Edits, InvalidOutput> {
    let v: Value = serde_json::from_slice(bytes).map_err(|_| bad(Rule::NotJson))?;
    if let Some(list) = v.get("edits").and_then(Value::as_array) {
        for e in list {
            if let Some(r) = e.get("reason").and_then(Value::as_str)
                && !REASONS.contains(&r)
            {
                return Err(bad(Rule::UnknownReason));
            }
        }
    }
    serde_json::from_value(v).map_err(|_| bad(Rule::NotJson))
}

fn exists(body: &Value, path: &Path) -> bool {
    let mut cur = body;
    for s in &path.0 {
        cur = match (s, cur) {
            (Seg::Key(k), Value::Object(m)) => match m.get(k) {
                Some(c) => c,
                None => return false,
            },
            (Seg::Index(i), Value::Array(a)) => match a.get(*i) {
                Some(c) => c,
                None => return false,
            },
            _ => return false,
        };
    }
    true
}

/// The R5 checks, in order: count, value size, kind, selector, existence, overlap.
pub fn check(body: &Value, selectors: &[Selector], edits: &[Edit]) -> Result<(), InvalidOutput> {
    if edits.len() > MAX_EDITS {
        return Err(bad(Rule::TooManyEdits));
    }
    for e in edits {
        let kind_ok = match (e.op, e.kind) {
            (Op::Remove, Kind::Removed) => e.value.is_none(),
            (Op::Replace, Kind::Converted) => e.value.is_some(),
            _ => false,
        };
        if !kind_ok {
            return Err(bad(Rule::KindMismatch));
        }
        if let Some(v) = &e.value
            && !serde_json::to_vec(v).is_ok_and(|b| b.len() <= MAX_VALUE_BYTES)
        {
            return Err(bad(Rule::ValueTooLarge));
        }
        if !covers(selectors, &e.path) {
            return Err(bad(Rule::OutsideSelector));
        }
        if !exists(body, &e.path) {
            return Err(bad(Rule::PathMissing));
        }
    }
    let mut paths: Vec<&Path> = edits.iter().map(|e| &e.path).collect();
    paths.sort();
    if paths.windows(2).any(|w| w[0].is_prefix_of(w[1])) {
        return Err(bad(Rule::Overlap));
    }
    Ok(())
}

fn slot<'a>(body: &'a mut Value, path: &Path) -> Option<&'a mut Value> {
    let mut cur = body;
    for s in &path.0 {
        cur = match (s, cur) {
            (Seg::Key(k), Value::Object(m)) => m.get_mut(k)?,
            (Seg::Index(i), Value::Array(a)) => a.get_mut(*i)?,
            _ => return None,
        };
    }
    Some(cur)
}

/// Applies checked edits to a clone. Replacements go first, then removals from the highest
/// path down, so array indices stay valid.
pub fn apply(body: &Value, edits: &[Edit]) -> Value {
    let mut out = body.clone();
    for e in edits.iter().filter(|e| e.op == Op::Replace) {
        if let (Some(s), Some(v)) = (slot(&mut out, &e.path), &e.value) {
            *s = v.clone();
        }
    }
    let mut rms: Vec<&Edit> = edits.iter().filter(|e| e.op == Op::Remove).collect();
    rms.sort_by(|a, b| b.path.cmp(&a.path));
    for e in rms {
        let Some((last, parent)) = e.path.0.split_last() else { continue };
        let Some(p) = slot(&mut out, &Path(parent.to_vec())) else { continue };
        match (last, p) {
            (Seg::Key(k), Value::Object(m)) => {
                m.shift_remove(k);
            }
            (Seg::Index(i), Value::Array(a)) if *i < a.len() => {
                a.remove(*i);
            }
            _ => {}
        }
    }
    out
}

pub fn changes(edits: &[Edit]) -> Vec<ContentChange> {
    edits.iter().map(|e| ContentChange { path: e.path.to_string(), kind: e.kind, reason: e.reason }).collect()
}
