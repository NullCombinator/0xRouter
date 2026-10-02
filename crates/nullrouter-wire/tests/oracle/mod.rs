//! The 9router translator oracle (T042) and the style-subject rows of
//! `tests/parity/deviations.toml` (research R26), shared by the wire parity tests.
//!
//! A translate deviation names a pair (`style = "claude-to-openai"`, `*` for any side), a
//! case (`fixture`; a last `*` matches any rest, as in `stream-*`) and a field pattern: a dotted path whose
//! `*` segments match any key or index, and whose last segment `**` matches everything under
//! it. A comparison drops exactly the matched leaves on both sides; a row that never matched
//! a real difference in the whole run is stale.

#![allow(dead_code)]

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use nullrouter_registry::validate::validate_style;
use nullrouter_wire::codec::Style;
use serde::Deserialize;
use serde_json::Value;

pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// 9router's format name → 0router's bundled style id.
pub fn style_id(format: &str) -> &'static str {
    match format {
        "openai" => "openai-chat",
        "claude" => "anthropic-messages",
        "openai-responses" => "openai-responses",
        "gemini" => "gemini",
        f => panic!("no bundled style for 9router format {f}"),
    }
}

pub fn bundled(id: &str) -> Style {
    let path = root().join(format!("styles/bundled/{id}.toml"));
    let src = std::fs::read_to_string(&path).unwrap();
    let parsed = validate_style(&src, &path.display().to_string())
        .unwrap_or_else(|e| panic!("{id} fails the style gate: {e:#?}"));
    Style::compile(&parsed).unwrap()
}

/// One oracle file: its pair directory, case name, and `data`.
pub struct Fixture {
    pub pair: String,
    pub case: String,
    pub data: Value,
}

pub fn fixtures() -> Vec<Fixture> {
    let dir = root().join("tests/fixtures/9router/translate");
    let mut out = Vec::new();
    for pair in sorted(&dir) {
        for file in sorted(&pair) {
            let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
            out.push(Fixture {
                pair: pair.file_name().unwrap().to_string_lossy().into_owned(),
                case: file.file_stem().unwrap().to_string_lossy().into_owned(),
                data: v["data"].clone(),
            });
        }
    }
    assert!(!out.is_empty(), "no oracle under {} (run tools/gen-bundled/generate.mjs)", dir.display());
    out
}

fn sorted(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    v.sort();
    v
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    deviation: Vec<Row>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    provider: Option<String>,
    style: Option<String>,
    fixture: String,
    field: String,
    reason: String,
}

pub struct Deviations {
    rows: Vec<Row>,
    used: RefCell<Vec<bool>>,
}

impl Deviations {
    /// The style-subject rows whose fixture is a translate case.
    pub fn load() -> Self {
        let path = root().join("tests/parity/deviations.toml");
        let file: File =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap_or_else(|e| panic!("deviations.toml: {e}"));
        let rows: Vec<Row> =
            file.deviation.into_iter().filter(|r| r.style.as_deref().is_some_and(|s| s.contains("-to-"))).collect();
        for r in &rows {
            assert!(r.provider.is_none(), "{r:?}: set exactly one of provider or style");
            assert!(!r.reason.trim().is_empty(), "{r:?}: a deviation needs a reason");
        }
        let used = RefCell::new(vec![false; rows.len()]);
        Self { rows, used }
    }

    /// The differences between `got` and `want` for `pair`/`case` outside the listed
    /// deviations, one line per leaf.
    pub fn compare(&self, pair: &str, case: &str, got: &Value, want: &Value) -> Vec<String> {
        let (from, to) = pair.split_once("-to-").unwrap();
        let mut out = Vec::new();
        for (path, g, w) in diff(got, want) {
            let hit = self.rows.iter().position(|r| {
                let (rf, rt) = r.style.as_deref().unwrap().split_once("-to-").unwrap();
                (rf == "*" || rf == from)
                    && (rt == "*" || rt == to)
                    && r.fixture.strip_suffix('*').map_or(r.fixture == case, |p| case.starts_with(p))
                    && matches(&r.field, &path)
            });
            match hit {
                Some(i) => self.used.borrow_mut()[i] = true,
                None => out.push(format!("{pair}/{case}: {path}\n    got  {}\n    want {}", show(g), show(w))),
            }
        }
        out
    }

    /// Rows that never matched a difference: each must be removed.
    pub fn stale(&self) -> Vec<String> {
        let used = self.used.borrow();
        self.rows
            .iter()
            .zip(used.iter())
            .filter(|(_, u)| !**u)
            .map(|(r, _)| {
                format!("{} {} `{}` ({}) no longer differs", r.style.as_deref().unwrap(), r.fixture, r.field, r.reason)
            })
            .collect()
    }
}

fn show(v: Option<&Value>) -> String {
    v.map_or_else(|| "(absent)".to_owned(), Value::to_string)
}

/// A `field` pattern matches `path` segment by segment; a last `**` matches any rest.
fn matches(pattern: &str, path: &str) -> bool {
    let mut p = path.split('.');
    for seg in pattern.split('.') {
        if seg == "**" {
            return true;
        }
        if !p.next().is_some_and(|s| seg == "*" || seg == s) {
            return false;
        }
    }
    p.next().is_none()
}

/// Leaf differences as `(dotted path, got, want)`; a missing side is `None`.
pub fn diff<'a>(got: &'a Value, want: &'a Value) -> Vec<(String, Option<&'a Value>, Option<&'a Value>)> {
    let mut out = Vec::new();
    walk(String::new(), Some(got), Some(want), &mut out);
    out
}

fn walk<'a>(
    at: String,
    g: Option<&'a Value>,
    w: Option<&'a Value>,
    out: &mut Vec<(String, Option<&'a Value>, Option<&'a Value>)>,
) {
    let join = |k: &str| if at.is_empty() { k.to_owned() } else { format!("{at}.{k}") };
    match (g, w) {
        (Some(Value::Object(a)), Some(Value::Object(b))) => {
            for k in a.keys().chain(b.keys().filter(|k| !a.contains_key(*k))) {
                walk(join(k), a.get(k), b.get(k), out);
            }
        }
        (Some(Value::Array(a)), Some(Value::Array(b))) => {
            for i in 0..a.len().max(b.len()) {
                walk(join(&i.to_string()), a.get(i), b.get(i), out);
            }
        }
        (g, w) if g != w => out.push((at, g, w)),
        _ => {}
    }
}

#[test]
fn patterns_match_by_segment() {
    assert!(matches("messages.*.cache_control", "messages.2.cache_control"));
    assert!(matches("system.**", "system.0.text"));
    assert!(matches("**", "model"));
    assert!(!matches("system", "system.0.text"));
    assert!(!matches("messages.*.cache_control", "messages.2.content"));
    assert!(!matches("system.0.text", "system"));
}
