//! Deliberate deviations from the 9router oracle, loaded from `tests/parity/deviations.toml`
//! (research R26). A comparison skips exactly the listed fields and fails if a listed
//! field no longer differs, so the list can't go stale.

use std::path::PathBuf;

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    deviation: Vec<Deviation>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Deviation {
    provider: Option<String>,
    style: Option<String>,
    fixture: String,
    /// Dotted path into the compared value.
    field: String,
    reason: String,
}

impl Deviation {
    fn subject(&self) -> &str {
        self.provider.as_deref().or(self.style.as_deref()).unwrap_or_default()
    }
}

pub(crate) struct Deviations(Vec<Deviation>);

impl Deviations {
    pub(crate) fn load() -> Self {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity/deviations.toml");
        let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        Self::parse(&src)
    }

    fn parse(src: &str) -> Self {
        let file: File = toml::from_str(src).unwrap_or_else(|e| panic!("deviations.toml: {e}"));
        for d in &file.deviation {
            assert!(d.provider.is_some() != d.style.is_some(), "{d:?}: set exactly one of provider or style");
            assert!(!d.reason.trim().is_empty(), "{d:?}: a deviation needs a reason");
        }
        Self(file.deviation)
    }

    /// Differences between `got` and `want` for `subject` in `fixture`, outside the listed
    /// deviations, plus every listed deviation whose field no longer differs.
    pub(crate) fn compare(&self, subject: &str, fixture: &str, got: &Value, want: &Value) -> Vec<String> {
        let (mut got, mut want) = (got.clone(), want.clone());
        let mut out = Vec::new();
        for d in self.0.iter().filter(|d| d.subject() == subject && d.fixture == fixture) {
            let (g, w) = (take(&mut got, &d.field), take(&mut want, &d.field));
            if g == w {
                out.push(format!("{subject}: listed deviation `{}` ({}) no longer differs", d.field, d.reason));
            }
        }
        if got != want {
            out.push(format!("{subject}:\n  got  {got}\n  want {want}"));
        }
        out
    }
}

/// Removes and returns the value at a dotted path; `None` when absent.
fn take(v: &mut Value, path: &str) -> Option<Value> {
    let (parent, last) = match path.rsplit_once('.') {
        Some((p, l)) => (p.split('.').try_fold(v, |v, k| v.get_mut(k))?, l),
        None => (v, path),
    };
    parent.as_object_mut()?.remove(last)
}

#[test]
fn skips_listed_fields_and_flags_stale_ones() {
    let d = Deviations::parse(
        r#"
[[deviation]]
provider = "p"
fixture = "providers"
field = "headers.user-agent"
reason = "no User-Agent spoofing"
"#,
    );
    let want = serde_json::json!({ "baseUrl": "u", "headers": { "user-agent": "x", "a": "1" } });
    let got = serde_json::json!({ "baseUrl": "u", "headers": { "a": "1" } });
    assert!(d.compare("p", "providers", &got, &want).is_empty());
    assert!(d.compare("q", "providers", &got, &want)[0].starts_with("q:\n"));
    let stale = d.compare("p", "providers", &want, &want);
    assert!(stale.len() == 1 && stale[0].contains("no longer differs"), "{stale:?}");
    let other = serde_json::json!({ "baseUrl": "v", "headers": { "a": "1" } });
    assert_eq!(d.compare("p", "providers", &other, &want).len(), 1);
}

#[test]
fn deviations_file_parses() {
    Deviations::load();
}
