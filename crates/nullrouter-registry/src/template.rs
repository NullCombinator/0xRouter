//! Typed templates and field selectors for API-style and plugin files
//! (contracts/api-style-schema.md § Mapping vocabulary).
//!
//! A template is a JSON-shaped TOML value whose strings may hold placeholders:
//! - `"{p}"`: the whole string is a placeholder and keeps the value's type;
//! - `"{p?}"`: as above, and the key is omitted when the value is absent;
//! - `"x{p}y"`: string interpolation;
//! - `"{null}"`: JSON null (TOML has none);
//! - `{{` and `}}`: literal braces.
//!
//! There are no expressions, conditionals or loops. Each context allows a fixed set of
//! placeholder names; `{account.*}` and `{secret.*}` are never allowed. `nullrouter-wire`
//! renders and reverse-matches the parsed tree.

use std::fmt;

/// A parsed template.
#[derive(Debug, Clone, PartialEq)]
pub enum Template {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// A literal string, braces unescaped.
    Str(String),
    /// A string mixing literals and placeholders.
    Interp(Vec<Piece>),
    /// A whole-string placeholder.
    Hole {
        name: String,
        optional: bool,
    },
    Array(Vec<Template>),
    Object(Vec<(String, Template)>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    Lit(String),
    Hole(String),
}

/// Why a template string or selector was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateError {
    /// Dotted location inside the template, e.g. `delta.text`; empty at the root.
    pub at: String,
    pub rule: String,
}

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.at.is_empty() { f.write_str(&self.rule) } else { write!(f, "{}: {}", self.at, self.rule) }
    }
}

/// The placeholder names one context allows. An entry ending in `.*` allows any name
/// under that prefix.
#[derive(Debug, Clone, Copy)]
pub struct PlaceholderSet(pub &'static [&'static str]);

impl PlaceholderSet {
    pub fn allows(&self, name: &str) -> bool {
        self.0.iter().any(|p| match p.strip_suffix(".*") {
            Some(prefix) => name.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('.') && rest.len() > 1),
            None => *p == name,
        })
    }
}

impl Template {
    /// Parses a TOML value. Placeholder names are checked for syntax only; use
    /// [`Template::check`] for the context's set.
    pub fn parse(v: &toml::Value) -> Result<Self, TemplateError> {
        parse_at(v, "")
    }

    /// Rejects any placeholder outside `set`, and always `{account.*}` and `{secret.*}`.
    pub fn check(&self, set: PlaceholderSet) -> Result<(), TemplateError> {
        let mut first = None;
        self.walk_holes(&mut |at, name| {
            if first.is_none()
                && let Some(rule) = hole_rule(name, set)
            {
                first = Some(TemplateError { at: at.to_owned(), rule });
            }
        });
        first.map_or(Ok(()), Err)
    }

    /// Every placeholder name, in document order.
    pub fn holes(&self) -> Vec<&str> {
        let mut out = Vec::new();
        self.collect_holes(&mut out);
        out
    }

    fn collect_holes<'a>(&'a self, out: &mut Vec<&'a str>) {
        match self {
            Self::Hole { name, .. } => out.push(name),
            Self::Interp(pieces) => out.extend(pieces.iter().filter_map(|p| match p {
                Piece::Hole(n) => Some(n.as_str()),
                Piece::Lit(_) => None,
            })),
            Self::Array(items) => items.iter().for_each(|t| t.collect_holes(out)),
            Self::Object(fields) => fields.iter().for_each(|(_, t)| t.collect_holes(out)),
            _ => {}
        }
    }

    fn walk_holes(&self, f: &mut impl FnMut(&str, &str)) {
        fn go(t: &Template, at: &str, f: &mut impl FnMut(&str, &str)) {
            match t {
                Template::Hole { name, .. } => f(at, name),
                Template::Interp(pieces) => {
                    for p in pieces {
                        if let Piece::Hole(n) = p {
                            f(at, n);
                        }
                    }
                }
                Template::Array(items) => {
                    for (i, t) in items.iter().enumerate() {
                        go(t, &format!("{at}[{i}]"), f);
                    }
                }
                Template::Object(fields) => {
                    for (k, t) in fields {
                        go(t, &join(at, k), f);
                    }
                }
                _ => {}
            }
        }
        go(self, "", f);
    }

    /// True if the template contains `name` as a placeholder.
    pub fn mentions(&self, name: &str) -> bool {
        self.holes().contains(&name)
    }
}

fn hole_rule(name: &str, set: PlaceholderSet) -> Option<String> {
    if name.starts_with("account.") || name.starts_with("secret.") || name == "account" || name == "secret" {
        return Some(format!("placeholder {{{name}}} is not allowed: templates never see accounts or secrets"));
    }
    (!set.allows(name)).then(|| format!("unknown placeholder {{{name}}}"))
}

fn join(at: &str, k: &str) -> String {
    if at.is_empty() { k.to_owned() } else { format!("{at}.{k}") }
}

fn parse_at(v: &toml::Value, at: &str) -> Result<Template, TemplateError> {
    Ok(match v {
        toml::Value::Boolean(b) => Template::Bool(*b),
        toml::Value::Integer(i) => Template::Int(*i),
        toml::Value::Float(x) => Template::Float(*x),
        toml::Value::String(s) => parse_str(s).map_err(|rule| TemplateError { at: at.to_owned(), rule })?,
        toml::Value::Array(items) => Template::Array(
            items.iter().enumerate().map(|(i, t)| parse_at(t, &format!("{at}[{i}]"))).collect::<Result<_, _>>()?,
        ),
        toml::Value::Table(t) => Template::Object(
            t.iter().map(|(k, v)| Ok((k.clone(), parse_at(v, &join(at, k))?))).collect::<Result<_, _>>()?,
        ),
        toml::Value::Datetime(_) => {
            return Err(TemplateError { at: at.to_owned(), rule: "dates are not allowed in templates".into() });
        }
    })
}

/// Parses one template string.
pub fn parse_str(s: &str) -> Result<Template, String> {
    let mut pieces: Vec<Piece> = Vec::new();
    let mut lit = String::new();
    let mut optional_whole = None;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '{' if chars.peek().is_some_and(|&(_, n)| n == '{') => {
                chars.next();
                lit.push('{');
            }
            '}' if chars.peek().is_some_and(|&(_, n)| n == '}') => {
                chars.next();
                lit.push('}');
            }
            '{' => {
                let close = s[i..].find('}').ok_or("unclosed `{` in template")? + i;
                let inner = &s[i + 1..close];
                let (name, optional) = match inner.strip_suffix('?') {
                    Some(n) => (n, true),
                    None => (inner, false),
                };
                check_name(name)?;
                if optional {
                    optional_whole = Some(i);
                }
                if !lit.is_empty() {
                    pieces.push(Piece::Lit(std::mem::take(&mut lit)));
                }
                pieces.push(Piece::Hole(name.to_owned()));
                while chars.peek().is_some_and(|&(j, _)| j <= close) {
                    chars.next();
                }
            }
            _ => lit.push(c),
        }
    }
    if !lit.is_empty() {
        pieces.push(Piece::Lit(lit));
    }
    match pieces.as_slice() {
        [] => Ok(Template::Str(String::new())),
        [Piece::Lit(l)] => Ok(Template::Str(l.clone())),
        [Piece::Hole(n)] if n == "null" => {
            if optional_whole.is_some() {
                Err("{null?} is not a placeholder".into())
            } else {
                Ok(Template::Null)
            }
        }
        [Piece::Hole(n)] => Ok(Template::Hole { name: n.clone(), optional: optional_whole.is_some() }),
        _ if optional_whole.is_some() => Err("an optional placeholder `{p?}` must be the whole string".into()),
        _ if pieces.iter().any(|p| matches!(p, Piece::Hole(n) if n == "null")) => {
            Err("{null} must be the whole string".into())
        }
        _ => Ok(Template::Interp(pieces)),
    }
}

fn check_name(name: &str) -> Result<(), String> {
    let ok_seg = |seg: &str| {
        !seg.is_empty()
            && seg.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            && !seg.starts_with(|c: char| c.is_ascii_digit())
    };
    if name.is_empty() {
        return Err("empty placeholder `{}`".into());
    }
    if name.split('.').all(ok_seg) {
        return Ok(());
    }
    if name.chars().any(|c| "+-*/%()<>=!&|?: ,'\"[]".contains(c)) {
        return Err(format!("`{{{name}}}`: expressions are not allowed in templates"));
    }
    Err(format!("`{{{name}}}` is not a placeholder name (lowercase dotted segments)"))
}

/// A read-only selector such as `a.b[0].c` or `a[*].b`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FieldPath(pub Vec<PathSeg>);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PathSeg {
    Key(String),
    Index(usize),
    /// `[*]`: every element.
    Each,
}

impl FieldPath {
    pub fn parse(s: &str) -> Result<Self, String> {
        if s.is_empty() {
            return Err("empty field path".into());
        }
        let mut segs = Vec::new();
        for part in s.split('.') {
            let (key, mut rest) = match part.find('[') {
                Some(i) => (&part[..i], &part[i..]),
                None => (part, ""),
            };
            if key.is_empty() && segs.is_empty() && rest.is_empty() {
                return Err(format!("field path `{s}` has an empty segment"));
            }
            if !key.is_empty() {
                if key.contains([']', '{', '}', ' ']) {
                    return Err(format!("field path `{s}`: bad segment `{key}`"));
                }
                segs.push(PathSeg::Key(key.to_owned()));
            } else if rest.is_empty() {
                return Err(format!("field path `{s}` has an empty segment"));
            }
            while let Some(r) = rest.strip_prefix('[') {
                let close = r.find(']').ok_or_else(|| format!("field path `{s}`: unclosed `[`"))?;
                let idx = &r[..close];
                segs.push(if idx == "*" {
                    PathSeg::Each
                } else {
                    PathSeg::Index(idx.parse().map_err(|_| format!("field path `{s}`: `[{idx}]` is not an index"))?)
                });
                rest = &r[close + 1..];
            }
            if !rest.is_empty() {
                return Err(format!("field path `{s}`: unexpected `{rest}`"));
            }
        }
        Ok(Self(segs))
    }

    pub fn has_each(&self) -> bool {
        self.0.contains(&PathSeg::Each)
    }
}

impl fmt::Display for FieldPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, s) in self.0.iter().enumerate() {
            match s {
                PathSeg::Key(k) if i == 0 => f.write_str(k)?,
                PathSeg::Key(k) => write!(f, ".{k}")?,
                PathSeg::Index(n) => write!(f, "[{n}]")?,
                PathSeg::Each => f.write_str("[*]")?,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(src: &str) -> Template {
        let v: toml::Value = toml::from_str(&format!("v = {src}")).unwrap();
        Template::parse(&v["v"]).unwrap()
    }

    #[test]
    fn whole_placeholder_keeps_type() {
        assert_eq!(t(r#""{block.index}""#), Template::Hole { name: "block.index".into(), optional: false });
        assert_eq!(t(r#""{usage.input?}""#), Template::Hole { name: "usage.input".into(), optional: true });
        assert_eq!(t(r#""{null}""#), Template::Null);
    }

    #[test]
    fn interpolation_and_escapes() {
        assert_eq!(
            t(r#""data:{media.mime};base64,{media.data}""#),
            Template::Interp(vec![
                Piece::Lit("data:".into()),
                Piece::Hole("media.mime".into()),
                Piece::Lit(";base64,".into()),
                Piece::Hole("media.data".into()),
            ])
        );
        assert_eq!(t(r#""{{literal}}""#), Template::Str("{literal}".into()));
    }

    #[test]
    fn objects_and_arrays() {
        let tpl = t(r#"{ type = "text", text = "{part.text}", n = 1, xs = [true] }"#);
        assert_eq!(tpl.holes(), vec!["part.text"]);
        assert!(matches!(tpl, Template::Object(ref f) if f.len() == 4));
    }

    #[test]
    fn rejects_expressions_and_bad_names() {
        for (src, want) in [
            (r#""{a+b}""#, "expressions are not allowed"),
            (r#""{a b}""#, "expressions are not allowed"),
            (r#""x{p?}""#, "must be the whole string"),
            (r#""{Upper}""#, "not a placeholder name"),
            (r#""{open""#, "unclosed"),
            (r#""{}""#, "empty placeholder"),
        ] {
            let v: toml::Value = toml::from_str(&format!("v = {src}")).unwrap();
            let err = Template::parse(&v["v"]).unwrap_err();
            assert!(err.rule.contains(want), "{src}: {err}");
        }
    }

    #[test]
    fn check_against_context() {
        const SET: PlaceholderSet = PlaceholderSet(&["part.text", "input.*"]);
        assert!(t(r#"{ a = "{part.text}", b = "{input.audio}" }"#).check(SET).is_ok());
        let err = t(r#"{ a = { b = "{request.api_key}" } }"#).check(SET).unwrap_err();
        assert_eq!(err.to_string(), "a.b: unknown placeholder {request.api_key}");
        let err = t(r#""{secret.value}""#).check(PlaceholderSet(&["secret.*"])).unwrap_err();
        assert!(err.rule.contains("never see accounts or secrets"), "{err}");
        assert!(!SET.allows("input"));
    }

    #[test]
    fn field_paths() {
        let s = FieldPath::parse("choices[0].delta.tool_calls[*].function").unwrap();
        assert_eq!(s.to_string(), "choices[0].delta.tool_calls[*].function");
        assert!(s.has_each());
        assert_eq!(FieldPath::parse("a").unwrap().0, vec![PathSeg::Key("a".into())]);
        for bad in ["", "a..b", "a[x]", "a[0", "a{b}"] {
            assert!(FieldPath::parse(bad).is_err(), "{bad}");
        }
    }
}
