//! The route table, built from every loaded style's `[[routes]]`.
//!
//! Path patterns: `{name}` matches one non-empty segment and `{name*}` the non-empty rest,
//! `/` included. Literal text may sit on either side of a hole within its segment, so
//! `{model*}:generateContent` works. `{name*}` must be in the last segment.
//!
//! [`RouteTable::candidates`] returns every route matching method, path and header
//! discriminator, discriminated routes first. [`pick`] then applies body discriminators.

use std::sync::Arc;

use axum::http::{HeaderMap, Method};
use serde_json::Value;
use zerorouter_registry::schema::{MatchRule, Route, StyleFile};
use zerorouter_registry::template::FieldPath;
use zerorouter_wire::codec::Style;
use zerorouter_wire::template::select_one;

/// A loaded style: the file (carriers, routes) and its compiled codec.
#[derive(Debug)]
pub struct StyleEntry {
    pub file: StyleFile,
    pub codec: Style,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Hole {
    One(String),
    Rest(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Seg {
    prefix: String,
    hole: Option<Hole>,
    suffix: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern(Vec<Seg>);

pub type Captures = Vec<(String, String)>;

impl Pattern {
    pub fn parse(path: &str) -> Result<Self, String> {
        let body = path.strip_prefix('/').ok_or_else(|| format!("route path `{path}` must start with `/`"))?;
        let parts: Vec<&str> = body.split('/').collect();
        let mut segs = Vec::with_capacity(parts.len());
        for (i, part) in parts.iter().enumerate() {
            let seg = match part.find('{') {
                None => Seg { prefix: (*part).to_owned(), hole: None, suffix: String::new() },
                Some(open) => {
                    let close = part.find('}').filter(|c| *c > open).ok_or_else(|| format!("route path `{path}`: unclosed `{{`"))?;
                    let name = &part[open + 1..close];
                    let hole = match name.strip_suffix('*') {
                        Some(n) if i + 1 == parts.len() => Hole::Rest(n.to_owned()),
                        Some(_) => return Err(format!("route path `{path}`: `{{{name}}}` must be in the last segment")),
                        None => Hole::One(name.to_owned()),
                    };
                    let suffix = &part[close + 1..];
                    if suffix.contains('{') {
                        return Err(format!("route path `{path}`: one hole per segment"));
                    }
                    Seg { prefix: part[..open].to_owned(), hole: Some(hole), suffix: suffix.to_owned() }
                }
            };
            segs.push(seg);
        }
        Ok(Self(segs))
    }

    /// The captures (percent-decoded) if `path` matches.
    pub fn captures(&self, path: &str) -> Option<Captures> {
        let mut rest = path.strip_prefix('/')?;
        let mut caps = Vec::new();
        for (i, seg) in self.0.iter().enumerate() {
            let last = i + 1 == self.0.len();
            let (this, next) = match seg.hole {
                Some(Hole::Rest(_)) => (rest, ""),
                _ if last => (rest, ""),
                _ => rest.split_once('/')?,
            };
            if last && !matches!(seg.hole, Some(Hole::Rest(_))) && this.contains('/') {
                return None;
            }
            let inner = this.strip_prefix(seg.prefix.as_str())?.strip_suffix(seg.suffix.as_str())?;
            match &seg.hole {
                None if inner.is_empty() => {}
                None => return None,
                Some(Hole::One(n) | Hole::Rest(n)) => {
                    if inner.is_empty() {
                        return None;
                    }
                    caps.push((n.clone(), percent_decode(inner)?));
                }
            }
            rest = next;
        }
        Some(caps)
    }
}

/// `%XX` decoding; `None` for a malformed escape or non-UTF-8 result.
pub fn percent_decode(s: &str) -> Option<String> {
    if !s.contains('%') {
        return Some(s.to_owned());
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[derive(Debug)]
pub struct RouteEntry {
    pub style: Arc<StyleEntry>,
    pub route: Route,
    method: Method,
    pattern: Pattern,
}

#[derive(Debug)]
pub struct Matched<'t> {
    pub entry: &'t RouteEntry,
    pub captures: Captures,
}

impl Matched<'_> {
    pub fn capture(&self, name: &str) -> Option<&str> {
        self.captures.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Default)]
pub struct RouteTable {
    routes: Vec<RouteEntry>,
}

fn header_holds(rule: &MatchRule, headers: &HeaderMap) -> bool {
    rule.header_present.as_deref().is_none_or(|h| headers.contains_key(h))
}

fn has_body_rule(rule: &MatchRule) -> bool {
    rule.path_present.is_some() || rule.path_equals.is_some()
}

/// Whether `rule`'s body conditions hold for `body`. A rule with none holds.
pub fn body_rule_holds(rule: &MatchRule, body: &Value) -> bool {
    let at = |p: &str| FieldPath::parse(p).ok().and_then(|fp| select_one(&fp, body).cloned());
    let present = rule.path_present.as_deref().is_none_or(|p| at(p).is_some_and(|v| !v.is_null()));
    let equals = rule.path_equals.as_deref().is_none_or(|pe| match pe {
        [p, want] => at(p).is_some_and(|v| v.as_str().map_or_else(|| &v.to_string() == want, |s| s == want)),
        _ => false,
    });
    present && equals
}

impl RouteTable {
    pub fn build<'a>(styles: impl IntoIterator<Item = &'a StyleFile>) -> Result<Self, String> {
        let mut routes = Vec::new();
        for file in styles {
            let codec = Style::compile(file).map_err(|e| format!("style `{}`: {e}", file.id))?;
            let style = Arc::new(StyleEntry { file: file.clone(), codec });
            for route in &style.file.routes {
                let method = Method::from_bytes(route.method.to_ascii_uppercase().as_bytes())
                    .map_err(|_| format!("style `{}`: bad method `{}`", file.id, route.method))?;
                let pattern = Pattern::parse(&route.path).map_err(|e| format!("style `{}`: {e}", file.id))?;
                routes.push(RouteEntry { style: style.clone(), route: route.clone(), method, pattern });
            }
        }
        Ok(Self { routes })
    }

    pub fn len(&self) -> usize {
        self.routes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }

    /// Routes matching method, path and header discriminator; discriminated routes first,
    /// then in style-file and declaration order.
    pub fn candidates(&self, method: &Method, path: &str, headers: &HeaderMap) -> Vec<Matched<'_>> {
        let mut found: Vec<Matched<'_>> = self
            .routes
            .iter()
            .filter(|e| e.method == *method)
            .filter(|e| e.route.discriminator.as_ref().is_none_or(|d| header_holds(d, headers)))
            .filter_map(|e| Some(Matched { entry: e, captures: e.pattern.captures(path)? }))
            .collect();
        found.sort_by_key(|m| m.entry.route.discriminator.is_none());
        found
    }
}

/// The first candidate whose body discriminator holds (a candidate without one always holds).
pub fn pick<'a, 't>(candidates: &'a [Matched<'t>], body: &Value) -> Option<&'a Matched<'t>> {
    candidates.iter().find(|m| m.entry.route.discriminator.as_ref().is_none_or(|d| !has_body_rule(d) || body_rule_holds(d, body)))
}

/// Whether any candidate needs the body to be chosen.
pub fn needs_body(candidates: &[Matched<'_>]) -> bool {
    candidates.iter().any(|m| m.entry.route.discriminator.as_ref().is_some_and(has_body_rule))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    fn caps(p: &str, path: &str) -> Option<Captures> {
        Pattern::parse(p).unwrap().captures(path)
    }

    fn c(pairs: &[(&str, &str)]) -> Option<Captures> {
        Some(pairs.iter().map(|(a, b)| ((*a).to_owned(), (*b).to_owned())).collect())
    }

    #[test]
    fn patterns_match_segments_rest_and_suffixes() {
        assert_eq!(caps("/v1/chat/completions", "/v1/chat/completions"), c(&[]));
        assert_eq!(caps("/v1/chat/completions", "/v1/chat/completions/x"), None);
        assert_eq!(caps("/v1/videos/{id}", "/v1/videos/abc"), c(&[("id", "abc")]));
        assert_eq!(caps("/v1/videos/{id}", "/v1/videos/a/b"), None);
        assert_eq!(caps("/v1/videos/{id}", "/v1/videos/"), None);
        assert_eq!(caps("/v1/videos/{id}/content", "/v1/videos/abc/content"), c(&[("id", "abc")]));
        assert_eq!(caps("/v1/models/{model*}", "/v1/models/openrouter/openai/gpt-5"), c(&[("model", "openrouter/openai/gpt-5")]));
        assert_eq!(caps("/v1/models/{model*}", "/v1/models"), None);
        let generate = "/v1beta/models/{model*}:generateContent";
        assert_eq!(caps(generate, "/v1beta/models/openrouter/google/gemini-2.5:generateContent"), c(&[("model", "openrouter/google/gemini-2.5")]));
        assert_eq!(caps(generate, "/v1beta/models/x:streamGenerateContent"), None);
        assert_eq!(caps(generate, "/v1beta/models/:generateContent"), None);
        assert_eq!(caps("/v1/models/{model*}", "/v1/models/a%2Fb%20c"), c(&[("model", "a/b c")]));
        assert_eq!(caps("/v1/models/{model*}", "/v1/models/bad%zz"), None);
        assert!(Pattern::parse("/v1/{a*}/b").is_err());
        assert!(Pattern::parse("v1").is_err());
    }

    pub(crate) fn style_with_routes(id: &str, routes: &str) -> StyleFile {
        let src = include_str!("../../zerorouter-wire/tests/fixtures/mini-style.toml");
        let mut v: toml::Table = toml::from_str(src).unwrap();
        v.insert("id".into(), toml::Value::String(id.into()));
        let extra: toml::Table = toml::from_str(routes).unwrap();
        let toml::Value::Array(mine) = v.get_mut("routes").unwrap() else { unreachable!() };
        mine.extend(extra["routes"].as_array().unwrap().iter().cloned());
        let text = toml::to_string(&v).unwrap();
        zerorouter_registry::validate::validate_style(&text, &format!("{id}.toml")).unwrap()
    }

    #[test]
    fn candidates_prefer_satisfied_discriminators_and_bodies_pick() {
        let a = style_with_routes(
            "plain",
            r#"
[[routes]]
method = "GET"
path = "/v1/models"
op = "list_models"
type = "text"
"#,
        );
        let b = style_with_routes(
            "versioned",
            r#"
[[routes]]
method = "GET"
path = "/v1/models"
op = "list_models"
type = "text"
discriminator = { header_present = "anthropic-version" }
"#,
        );
        let t = RouteTable::build([&a, &b]).unwrap();
        let mut h = HeaderMap::new();
        let got = t.candidates(&Method::GET, "/v1/models", &h);
        assert_eq!(got.iter().map(|m| m.entry.style.file.id.as_str()).collect::<Vec<_>>(), ["plain"]);
        h.insert("anthropic-version", "2023-06-01".parse().unwrap());
        let got = t.candidates(&Method::GET, "/v1/models", &h);
        assert_eq!(got.iter().map(|m| m.entry.style.file.id.as_str()).collect::<Vec<_>>(), ["versioned", "plain"]);
        assert!(t.candidates(&Method::POST, "/v1/models", &h).is_empty());
        assert!(t.candidates(&Method::GET, "/nope", &h).is_empty());

        let rule = MatchRule { path_equals: Some(vec!["generationConfig.responseModalities[0]".into(), "IMAGE".into()]), ..Default::default() };
        assert!(body_rule_holds(&rule, &json!({ "generationConfig": { "responseModalities": ["IMAGE"] } })));
        assert!(!body_rule_holds(&rule, &json!({})));
    }
}
