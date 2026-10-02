//! Model lookup semantics, matching `ref/9router/open-sse/config/providerModels.js`.
//!
//! - A thinking suffix is only a *final* parenthesised group with no parentheses inside,
//!   plus trailing whitespace (FR-019).
//! - Version-separator tolerance turns digit-dash-digit into digit-dot-digit (FR-020).
//! - The upstream ID is the declared `upstream_id`, else the model id, with the request's
//!   suffix, else the declared preset suffix (FR-021).

use std::collections::HashMap;
use std::sync::LazyLock;

use regex_lite::Regex;

use crate::schema::Model;

/// JavaScript's `\s` and `String.prototype.trim` set (WhiteSpace + LineTerminator). Not
/// `char::is_whitespace`, which adds U+0085 and drops U+FEFF.
fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
    )
}

fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_space)
}

/// Byte offset where 9router's `/\([^()]+\)\s*$/` matches, if it does: the nearest `(`
/// before a final `)` with at least one character and no parentheses between them.
/// A backward scan; regex-lite would try every start position.
fn suffix_start(id: &str) -> Option<usize> {
    let body = id.trim_end_matches(is_js_space).strip_suffix(')')?;
    let open = body.rfind(['(', ')'])?;
    (body.as_bytes()[open] == b'(' && open + 1 < body.len()).then_some(open)
}

/// Splits a thinking suffix: `("claude-opus-4", Some("(high)"))`. The base is trimmed
/// only when a suffix is present, as in 9router.
pub fn split_suffix(id: &str) -> (&str, Option<&str>) {
    match suffix_start(id) {
        Some(i) => (js_trim(&id[..i]), Some(&id[i..])),
        None => (id, None),
    }
}

/// `4-5` → `4.5` for every non-overlapping digit-dash-digit, left to right.
pub fn normalise_version_sep(id: &str) -> String {
    let b = id.as_bytes();
    let mut out = String::with_capacity(id.len());
    let mut i = 0;
    while i < b.len() {
        if i + 2 < b.len() && b[i].is_ascii_digit() && b[i + 1] == b'-' && b[i + 2].is_ascii_digit() {
            out.push(b[i] as char);
            out.push('.');
            out.push(b[i + 2] as char);
            i += 3;
        } else {
            // Copy the whole char so multi-byte text survives.
            let len = id[i..].chars().next().map_or(1, char::len_utf8);
            out.push_str(&id[i..i + len]);
            i += len;
        }
    }
    out
}

/// A provider's catalog with an exact-id index.
#[derive(Debug, Clone, Default)]
pub(crate) struct Catalog {
    index: HashMap<Box<str>, usize>,
    tolerant: bool,
}

impl Catalog {
    pub(crate) fn new(models: &[Model], tolerant: bool) -> Self {
        let mut index = HashMap::with_capacity(models.len());
        for (i, m) in models.iter().enumerate() {
            index.entry(m.id.as_str().into()).or_insert(i);
        }
        Self { index, tolerant }
    }

    /// 9router `findModel`: the first model whose id equals `id` or its suffix-stripped
    /// base; then, for tolerant providers only, the version-normalised base.
    pub(crate) fn find<'m>(&self, models: &'m [Model], id: &str) -> Option<&'m Model> {
        let base = strip_suffix(id);
        let exact = self.index.get(id).copied();
        let stripped = self.index.get(base).copied();
        let hit = match (exact, stripped) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        if let Some(i) = hit {
            return models.get(i);
        }
        if !self.tolerant {
            return None;
        }
        let normalised = normalise_version_sep(base);
        if normalised == base {
            return None;
        }
        self.index.get(normalised.as_str()).and_then(|&i| models.get(i))
    }

    /// 9router `getModelUpstreamId`.
    pub(crate) fn upstream_id(&self, models: &[Model], id: &str) -> String {
        self.find_with_upstream(models, id).1
    }

    /// `find(id)` and `upstream_id(id)` in one pass. 9router looks up the full id for the
    /// first and the suffix-free base for the second; without a suffix they are the same.
    pub(crate) fn find_with_upstream<'m>(&self, models: &'m [Model], id: &str) -> (Option<&'m Model>, String) {
        let (base, suffix) = split_suffix(id);
        let by_base = self.find(models, base);
        let found = if suffix.is_none() { by_base } else { self.find(models, id) };
        (found, upstream_id(by_base, base, suffix))
    }
}

/// `id` with a final suffix removed and whitespace trimmed (9router `baseModelId`).
fn strip_suffix(id: &str) -> &str {
    js_trim(suffix_start(id).map_or(id, |i| &id[..i]))
}

/// FR-021. `base`/`suffix` come from [`split_suffix`] on the requested id.
pub fn upstream_id(model: Option<&Model>, base: &str, suffix: Option<&str>) -> String {
    let resolved = model.map(|m| m.upstream_id.as_deref().filter(|u| !u.is_empty()).unwrap_or(&m.id));
    match resolved.filter(|r| !r.is_empty()) {
        Some(resolved) => {
            let (resolved_base, preset) = split_suffix(resolved);
            let mut out = String::with_capacity(resolved_base.len() + 16);
            out.push_str(resolved_base);
            out.push_str(suffix.or(preset).unwrap_or(""));
            out
        }
        None => {
            let mut out = String::with_capacity(base.len() + suffix.map_or(0, str::len));
            out.push_str(base);
            out.push_str(suffix.unwrap_or(""));
            out
        }
    }
}

type NamePattern = (Regex, fn(&regex_lite::Captures<'_>) -> String);

static NAME_PATTERNS: LazyLock<Vec<NamePattern>> = LazyLock::new(|| {
    let p = |re: &str, f: fn(&regex_lite::Captures<'_>) -> String| (Regex::new(re).expect("name pattern"), f);
    vec![
        p(r"(?i)^kimi-k(\d+(?:\.\d+)?)(-thinking)?$", |m| {
            format!("Kimi K{}{}", &m[1], if m.get(2).is_some() { " Thinking" } else { "" })
        }),
        p(r"(?i)^glm-(\d+(?:\.\d+)?)(v)?$", |m| {
            format!("GLM {}{}", &m[1], if m.get(2).is_some() { "V (Vision)" } else { "" })
        }),
        p(r"(?i)^minimax-m(\d+(?:\.\d+)?)$", |m| format!("MiniMax M{}", &m[1])),
        p(r"(?i)^gpt-(.+)$", |m| format!("GPT {}", title_case(&m[1]))),
        p(r"(?i)^gemini-(.+)$", |m| format!("Gemini {}", title_case(&m[1]))),
        p(r"(?i)^grok-(.+)$", |m| format!("Grok {}", title_case(&m[1]))),
        p(r"(?i)^deepseek-(.+)$", |m| format!("DeepSeek {}", title_case(&m[1]))),
        p(r"(?i)^qwen([\d.]+.*)$", |m| format!("Qwen {}", title_case(&m[1]))),
    ]
});

/// `"coder-plus"` → `"Coder Plus"`; words starting with a digit are kept as-is.
fn title_case(s: &str) -> String {
    s.split(|c: char| c == '-' || c == '_' || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(c) if !c.is_ascii_digit() => c.to_uppercase().chain(chars).collect(),
                _ => w.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Display name for a model that declares none (9router `deriveModelName`).
pub fn derive_model_name(id: &str) -> String {
    NAME_PATTERNS.iter().find_map(|(re, f)| re.captures(id).map(|m| f(&m))).unwrap_or_else(|| id.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn models(ids: &[(&str, Option<&str>)]) -> Vec<Model> {
        ids.iter()
            .map(|(id, up)| Model { id: (*id).into(), upstream_id: up.map(Into::into), ..Model::default() })
            .collect()
    }

    /// The scan agrees with 9router's regex. regex-lite's `\s` is ASCII-only, so the
    /// oracle is limited to ASCII inputs; JS-only whitespace is checked separately.
    #[test]
    fn suffix_scan_matches_the_regex() {
        let re = regex_lite::Regex::new(r"\([^()]+\)\s*$").unwrap();
        for id in [
            "m",
            "m(high)",
            "m (high)",
            "m(high) \t",
            "m()",
            "m(a(b))",
            "m(a)(b)",
            "m(a)x",
            "(x)",
            "()",
            ")",
            "(",
            "m(a b)",
            "m((a)",
            "m(a))",
            "m(\n)",
            "a(b)c(d) ",
            "",
        ] {
            assert_eq!(suffix_start(id), re.find(id).map(|m| m.start()), "{id:?}");
        }
        assert_eq!(split_suffix("m(high)\u{3000}"), ("m", Some("(high)\u{3000}")));
        assert_eq!(split_suffix("m\u{FEFF}(high)"), ("m", Some("(high)")));
        assert_eq!(strip_suffix("\u{A0}m\u{A0}"), "m");
    }

    #[test]
    fn thinking_suffix_is_stripped_and_reappended() {
        let m = models(&[("claude-opus-4", Some("claude-opus-4-20250514"))]);
        let c = Catalog::new(&m, false);
        assert_eq!(c.upstream_id(&m, "claude-opus-4(high)"), "claude-opus-4-20250514(high)");
        assert!(c.find(&m, "claude-opus-4(high)").is_some());
    }

    #[test]
    fn preset_suffix_kept_or_replaced() {
        let m = models(&[("opus-think", Some("claude-opus-4(high)"))]);
        let c = Catalog::new(&m, false);
        assert_eq!(c.upstream_id(&m, "opus-think"), "claude-opus-4(high)");
        assert_eq!(c.upstream_id(&m, "opus-think(low)"), "claude-opus-4(low)");
    }

    #[test]
    fn nested_parens_are_not_a_suffix() {
        assert_eq!(split_suffix("m(a(b))"), ("m(a(b))", None));
        let m = models(&[("m", None)]);
        let c = Catalog::new(&m, false);
        assert!(c.find(&m, "m(a(b))").is_none());
        assert_eq!(c.upstream_id(&m, "m(a(b))"), "m(a(b))");
    }

    #[test]
    fn trailing_whitespace_after_suffix() {
        assert_eq!(split_suffix("m (high)  "), ("m", Some("(high)  ")));
        let m = models(&[("m", None)]);
        assert_eq!(Catalog::new(&m, false).upstream_id(&m, "m(high)  "), "m(high)  ");
    }

    #[test]
    fn version_separator_tolerance_only_when_enabled() {
        let m = models(&[("claude-sonnet-4.5", None)]);
        let tolerant = Catalog::new(&m, true);
        assert_eq!(tolerant.upstream_id(&m, "claude-sonnet-4-5(high)"), "claude-sonnet-4.5(high)");
        let exact = Catalog::new(&m, false);
        assert!(exact.find(&m, "claude-sonnet-4-5").is_none());
        assert_eq!(exact.upstream_id(&m, "claude-sonnet-4-5"), "claude-sonnet-4-5");
    }

    #[test]
    fn undeclared_model_gives_base_plus_suffix() {
        let m = models(&[("a", None)]);
        let c = Catalog::new(&m, false);
        assert!(c.find(&m, "brand-new(high)").is_none());
        assert_eq!(c.upstream_id(&m, "brand-new(high)"), "brand-new(high)");
    }

    #[test]
    fn untyped_model_has_no_kind() {
        let m = models(&[("a", None)]);
        assert_eq!(Catalog::new(&m, false).find(&m, "a").unwrap().kind, None);
    }

    #[test]
    fn normalise_is_non_overlapping() {
        assert_eq!(normalise_version_sep("a-4-5-6"), "a-4.5-6");
        assert_eq!(normalise_version_sep("qwen3-coder"), "qwen3-coder");
    }

    #[test]
    fn derived_names() {
        assert_eq!(derive_model_name("gpt-5-codex-mini"), "GPT 5 Codex Mini");
        assert_eq!(derive_model_name("kimi-k2-thinking"), "Kimi K2 Thinking");
        assert_eq!(derive_model_name("glm-4.5v"), "GLM 4.5V (Vision)");
        assert_eq!(derive_model_name("qwen3-coder-plus"), "Qwen 3 Coder Plus");
        assert_eq!(derive_model_name("other"), "other");
    }
}
