//! The validation gate (FR-007 – FR-010). Bundled and user plugins pass the same checks.

use std::collections::HashMap;
use std::ops::Range;

use toml::Spanned;
use toml::de::{DeTable, DeValue};

use super::errors::{FieldPath, Seg, ValidationError, line_col};
use super::secrets::{check_map_key, check_query, check_url};
use crate::schema::{
    CapabilityKind, KNOWN_OAUTH_PARAMS, KNOWN_SECTION_FORMATS, PluginFile, PluginSource, ProviderEntity, Transport,
};

/// Parses and checks one plugin file. Returns every error found, not just the first.
pub fn validate(src: &str, source: PluginSource, file: &str) -> Result<ProviderEntity, Vec<ValidationError>> {
    let plugin = parse::<PluginFile>(src, file)?;
    let errors = semantic_errors(&plugin);
    if errors.is_empty() {
        Ok(ProviderEntity::from_file(plugin, source))
    } else {
        Err(errors.into_iter().map(|(path, rule)| positioned(src, file, path, rule)).collect())
    }
}

/// Deserialises `src` with path-and-span error reporting. Shared with the config loader.
pub(crate) fn parse<T: serde::de::DeserializeOwned>(src: &str, file: &str) -> Result<T, Vec<ValidationError>> {
    let de = toml::de::Deserializer::parse(src).map_err(|e| vec![toml_error(src, file, FieldPath::root(), &e)])?;
    serde_path_to_error::deserialize(de).map_err(|e| {
        let path = FieldPath(
            e.path()
                .iter()
                .filter_map(|s| match s {
                    serde_path_to_error::Segment::Seq { index } => Some(Seg::Index(*index)),
                    serde_path_to_error::Segment::Map { key } => Some(Seg::Key(key.clone())),
                    serde_path_to_error::Segment::Enum { variant } => Some(Seg::Key(variant.clone())),
                    serde_path_to_error::Segment::Unknown => None,
                })
                .collect(),
        );
        vec![toml_error(src, file, path, e.inner())]
    })
}

fn toml_error(src: &str, file: &str, path: FieldPath, e: &toml::de::Error) -> ValidationError {
    let (line, col) = e.span().map_or((0, 0), |s| line_col(src, s.start));
    ValidationError { file: file.to_owned(), line, col, path, rule: e.message().trim().to_owned() }
}

/// An error at `path`, positioned by walking the parsed document.
pub(crate) fn positioned(src: &str, file: &str, path: FieldPath, rule: String) -> ValidationError {
    let (line, col) = locate(src, &path).map_or((0, 0), |s| line_col(src, s.start));
    ValidationError { file: file.to_owned(), line, col, path, rule }
}

/// The span of the deepest existing prefix of `path`: the key for a table entry, the
/// value for an array element.
fn locate(src: &str, path: &FieldPath) -> Option<Range<usize>> {
    let root = DeTable::parse(src).ok()?;
    let mut span = None;
    let mut table: Option<&DeTable<'_>> = Some(root.get_ref());
    let mut array: Option<&[Spanned<DeValue<'_>>]> = None;
    for seg in &path.0 {
        let value = match (seg, table, array) {
            (Seg::Key(k), Some(t), _) => {
                let (key, value) = t.get_key_value(k.as_str())?;
                span = Some(key.span());
                value
            }
            (Seg::Index(i), _, Some(a)) => {
                let value = a.get(*i)?;
                span = Some(value.span());
                value
            }
            _ => break,
        };
        table = value.get_ref().as_table();
        array = value.get_ref().as_array().map(|a| &a[..]);
    }
    span
}

type Found = Vec<(FieldPath, String)>;

fn semantic_errors(p: &PluginFile) -> Found {
    let mut out = Found::new();
    let mut err = |path: FieldPath, rule: String| out.push((path, rule));

    if let Some(v) = p.schema.filter(|v| *v != 1) {
        err(FieldPath::of("schema"), format!("unsupported schema version {v}; expected 1"));
    }
    if !is_token(&p.id) {
        err(FieldPath::of("id"), format!("{:?} must match [a-z0-9][a-z0-9-]*", p.id));
    }
    if let Some(a) = p.alias.as_deref().filter(|a| !is_token(a)) {
        err(FieldPath::of("alias"), format!("{a:?} must match [a-z0-9][a-z0-9-]*"));
    }

    if let Some(t) = &p.transport {
        check_transport(t, &FieldPath::of("transport"), &mut err);
    }
    for (i, t) in p.transports.iter().enumerate() {
        check_transport(t, &FieldPath::of("transports").index(i), &mut err);
    }
    if !p.transports.is_empty() && p.transport.is_none() {
        err(FieldPath::of("transports"), "requires [transport] to be set".into());
    }

    if let Some(o) = &p.oauth {
        let base = FieldPath::of("oauth");
        for (k, url) in o.url_fields() {
            if let Err(rule) = check_url(url) {
                err(base.key(k), rule.into());
            }
        }
        for k in o.params.keys().filter(|k| !KNOWN_OAUTH_PARAMS.contains(&k.as_str())) {
            err(
                base.key("params").key(k.as_str()),
                format!("unknown OAuth parameter; allowed: {}", KNOWN_OAUTH_PARAMS.join(", ")),
            );
        }
    }

    for (kind, section) in &p.capabilities {
        let base = FieldPath::of("capabilities").key(kind.as_str());
        match &section.endpoint {
            None if p.transport.is_none() => err(base, "no endpoint and provider has no transport".into()),
            None => {}
            Some(e) => {
                let base = base.key("endpoint");
                for (k, url) in e.url_fields() {
                    if let Err(rule) = check_url(url) {
                        err(base.key(k), rule.into());
                    }
                }
                check_headers(e.headers.keys(), &base.key("headers"), &mut err);
                if let Some(f) = e.format.as_deref().filter(|f| !KNOWN_SECTION_FORMATS.contains(f)) {
                    err(
                        base.key("format"),
                        format!("unknown format {f:?}; allowed: {}", KNOWN_SECTION_FORMATS.join(", ")),
                    );
                }
            }
        }
    }

    // One id may appear once per kind: gemini-2.5-pro is both an llm and an stt model.
    // Lookup by id returns the first entry, as 9router's `findModel` does.
    let mut seen: HashMap<(&str, Option<CapabilityKind>), usize> = HashMap::new();
    for (i, m) in p.models.iter().flatten().enumerate() {
        let base = FieldPath::of("models").index(i);
        if let Some(&j) = seen.get(&(m.id.as_str(), m.kind)) {
            err(base.key("id"), format!("duplicate of models[{j}]"));
        } else {
            seen.insert((&m.id, m.kind), i);
        }
        for (j, name) in m.params.iter().flatten().enumerate() {
            if check_map_key(name).is_some() {
                err(base.key("params").index(j), format!("secret-like parameter {name:?} not allowed in plugins"));
            }
        }
    }
    out
}

fn check_transport(t: &Transport, base: &FieldPath, err: &mut impl FnMut(FieldPath, String)) {
    for (k, url) in t.url_fields() {
        // An empty base URL means the operator supplies it (azure).
        if k == "base_url" && url.is_empty() {
            continue;
        }
        if let Err(rule) = check_url(url) {
            err(base.key(k), rule.into());
        }
    }
    for (k, v) in [("url_suffix", &t.url_suffix), ("chat_path", &t.chat_path)] {
        if let Some(Err(rule)) = v.as_deref().map(check_query) {
            err(base.key(k), rule.into());
        }
    }
    check_headers(t.headers.iter().flat_map(|h| h.keys()), &base.key("headers"), err);
    if let Some(r) = &t.default_region {
        if !t.regions.as_ref().is_some_and(|m| m.contains_key(r)) {
            err(base.key("default_region"), format!("{r:?} is not a key of regions"));
        }
    }
}

fn check_headers<'a>(
    names: impl Iterator<Item = &'a String>,
    base: &FieldPath,
    err: &mut impl FnMut(FieldPath, String),
) {
    for name in names.filter(|n| check_map_key(n).is_some()) {
        err(base.key(name.as_str()), "credential-bearing header not allowed in plugins".into());
    }
}

/// `[a-z0-9][a-z0-9-]*`
fn is_token(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn errors(src: &str) -> Vec<String> {
        validate(src, PluginSource::Bundled, "t.toml").unwrap_err().iter().map(ToString::to_string).collect()
    }

    #[test]
    fn minimal_is_valid() {
        let e = validate("schema = 1\nid = \"p\"\ncategory = \"apikey\"\n", PluginSource::Bundled, "t.toml").unwrap();
        assert!(e.models.is_none() && e.transport.is_none());
    }

    #[test]
    fn unknown_field_has_path_and_position() {
        let e = errors("id = \"p\"\ncategory = \"apikey\"\n[oauth]\nclient_secret = \"x\"\n");
        assert_eq!(e.len(), 1);
        assert!(e[0].starts_with("t.toml:4:1 oauth"), "{}", e[0]);
        assert!(e[0].contains("client_secret"), "{}", e[0]);
    }

    #[test]
    fn semantic_errors_are_collected_and_positioned() {
        let e = errors(
            "id = \"P\"\ncategory = \"apikey\"\n[transport]\nheaders = { Authorization = \"x\" }\n\
             [[models]]\nid = \"a\"\n[[models]]\nid = \"a\"\n",
        );
        assert_eq!(e.len(), 3, "{e:#?}");
        assert!(e[0].starts_with("t.toml:1:1 id:"), "{}", e[0]);
        assert!(e[1].starts_with("t.toml:4:13 transport.headers.Authorization: credential-bearing"), "{}", e[1]);
        assert!(e[2].contains("models[1].id: duplicate of models[0]"), "{}", e[2]);
    }
}
