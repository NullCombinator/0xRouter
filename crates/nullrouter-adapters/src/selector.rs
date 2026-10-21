//! Selectors: which parts of a body an adapter reads. A restricted path language, so the core
//! can walk a body natively and clone only the matched subtrees.

use nullrouter_adapter_kit::{Part, Path, Seg};
use serde_json::Value;

const MAX_SEGMENTS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    Key(String),
    Index(usize),
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selector(Vec<Step>);

impl Selector {
    /// `$` selects the whole body. Otherwise dotted keys, `[N]` and `[*]`, with `["quoted"]`
    /// keys. At most 8 segments.
    pub fn parse(s: &str) -> Result<Selector, String> {
        if s.is_empty() {
            return Err("empty selector".into());
        }
        // `[*]` isn't a concrete path segment, so swap it out for the parser and put it back.
        const MARK: &str = "__nr_any__";
        let masked = s.replace("[*]", &format!("[\"{MARK}\"]"));
        let path = Path::parse(&masked)?;
        let mut out = Vec::with_capacity(path.0.len());
        for seg in path.0 {
            out.push(match seg {
                Seg::Index(i) => Step::Index(i),
                Seg::Key(k) if k == MARK && s.contains("[*]") => Step::Any,
                Seg::Key(k) => Step::Key(k),
            });
        }
        if out.len() > MAX_SEGMENTS {
            return Err(format!("more than {MAX_SEGMENTS} segments"));
        }
        Ok(Selector(out))
    }
}

fn walk(v: &Value, steps: &[Step], at: &Path, out: &mut Vec<Part>) {
    let Some((first, rest)) = steps.split_first() else {
        out.push(Part { path: at.clone(), value: v.clone() });
        return;
    };
    match (first, v) {
        (Step::Key(k), Value::Object(m)) => {
            if let Some(c) = m.get(k) {
                walk(c, rest, &at.child(k), out);
            }
        }
        (Step::Index(i), Value::Array(a)) => {
            if let Some(c) = a.get(*i) {
                walk(c, rest, &at.index(*i), out);
            }
        }
        (Step::Any, Value::Array(a)) => {
            for (i, c) in a.iter().enumerate() {
                walk(c, rest, &at.index(i), out);
            }
        }
        _ => {}
    }
}

/// Every part of `body` that any selector matches, with its concrete path.
pub fn extract(body: &Value, selectors: &[Selector]) -> Vec<Part> {
    let mut out = Vec::new();
    for s in selectors {
        walk(body, &s.0, &Path::root(), &mut out);
    }
    out
}

fn matches_prefix(s: &Selector, path: &Path) -> bool {
    s.0.len() <= path.0.len()
        && s.0.iter().zip(&path.0).all(|(st, sg)| match (st, sg) {
            (Step::Key(a), Seg::Key(b)) => a == b,
            (Step::Index(a), Seg::Index(b)) => a == b,
            (Step::Any, Seg::Index(_)) => true,
            _ => false,
        })
}

/// True when `path` is at or under a part some selector matches.
pub fn covers(selectors: &[Selector], path: &Path) -> bool {
    selectors.iter().any(|s| matches_prefix(s, path))
}
