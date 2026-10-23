//! The custom compatible provider (spec 014, data-model.md § Custom compatible provider): a
//! schema-2 plugin built from a few form fields, for a server that speaks the Chat Completions
//! or the Messages wire. The result is plugin data; the caller gates it before writing it.

use std::fmt;

/// The wire the server speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    OpenaiChat,
    AnthropicMessages,
}

impl Style {
    /// `openai`, `openai-chat`, `anthropic` or `anthropic-messages`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "openai" | "openai-chat" => Some(Self::OpenaiChat),
            "anthropic" | "anthropic-messages" => Some(Self::AnthropicMessages),
            _ => None,
        }
    }

    #[must_use]
    pub fn wire(self) -> &'static str {
        match self {
            Self::OpenaiChat => "openai-chat",
            Self::AnthropicMessages => "anthropic-messages",
        }
    }
}

/// The form fields.
#[derive(Debug, Clone)]
pub struct Spec<'a> {
    pub id: &'a str,
    /// Display name; the id when empty.
    pub name: &'a str,
    pub style: Style,
    pub base_url: &'a str,
    pub models: &'a [String],
}

/// A field that can't make a plugin, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid {
    pub field: &'static str,
    pub text: String,
}

impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

fn bad(field: &'static str, text: impl Into<String>) -> Invalid {
    Invalid { field, text: text.into() }
}

/// Lowercase letters, digits and dashes, 1 to 32 characters, not starting or ending with a dash.
pub fn check_id(id: &str) -> Result<(), Invalid> {
    let ok = !id.is_empty()
        && id.len() <= 32
        && !id.starts_with('-')
        && !id.ends_with('-')
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(bad("id", format!("{id:?} is not an id: use lowercase letters, digits and dashes, up to 32 characters")))
    }
}

/// `url` for an error text: scheme, host, port and path only. Userinfo, query and fragment are
/// dropped, since they may hold a credential; a URL that doesn't parse is not shown at all.
pub fn redact(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(mut u) if u.host_str().is_some() => {
            let _ = u.set_username("");
            let _ = u.set_password(None);
            u.set_query(None);
            u.set_fragment(None);
            let shown = u.to_string();
            // `Url` adds a `/` the operator didn't type to a URL with no path.
            if u.path() == "/" && !url.trim_end_matches(['?', '#']).contains("/?") && !url.ends_with('/') {
                shown.trim_end_matches('/').to_owned()
            } else {
                shown
            }
        }
        _ => "(not shown)".to_owned(),
    }
}

/// Whether `url` carries userinfo, a query or a fragment, where a credential may sit. A URL that
/// doesn't parse counts when it holds `@` or `?`.
pub fn has_secret_parts(url: &str) -> bool {
    match url::Url::parse(url) {
        Ok(u) => !u.username().is_empty() || u.password().is_some() || u.query().is_some() || u.fragment().is_some(),
        Err(_) => url.contains('@') || url.contains('?') || url.contains('#'),
    }
}

/// The endpoint URL for `base_url`: a URL already ending in the endpoint is kept; otherwise
/// the endpoint is appended, after `/v1` when the URL has no path.
pub fn endpoint_url(style: Style, base_url: &str) -> Result<String, Invalid> {
    let url = url::Url::parse(base_url)
        .map_err(|e| bad("base_url", format!("base_url {} doesn't parse: {e}", redact(base_url))))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(bad("base_url", format!("base_url {} must be an http or https URL", redact(base_url))));
    }
    if url.query().is_some() || url.fragment().is_some() || !url.username().is_empty() || url.password().is_some() {
        return Err(bad(
            "base_url",
            format!("base_url {} must have no credentials, query or fragment", redact(base_url)),
        ));
    }
    let tail = match style {
        Style::OpenaiChat => "/chat/completions",
        Style::AnthropicMessages => "/messages",
    };
    let path = url.path().trim_end_matches('/');
    let mut out = url.clone();
    if path.ends_with(tail) {
        out.set_path(path);
    } else if path.is_empty() {
        out.set_path(&format!("/v1{tail}"));
    } else {
        out.set_path(&format!("{path}{tail}"));
    }
    Ok(out.to_string())
}

fn q(s: &str) -> String {
    toml::Value::String(s.to_owned()).to_string()
}

/// The schema-2 plugin source for `spec`.
pub fn build(spec: &Spec<'_>) -> Result<String, Invalid> {
    check_id(spec.id)?;
    let name = if spec.name.trim().is_empty() { spec.id } else { spec.name.trim() };
    if name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err(bad("name", "the name must be up to 80 characters, on one line"));
    }
    let url = endpoint_url(spec.style, spec.base_url)?;
    let n = spec.models.len();
    if !(1..=200).contains(&n) {
        return Err(bad("models", format!("give 1 to 200 model ids, not {n}")));
    }
    for (i, m) in spec.models.iter().enumerate() {
        if m.trim().is_empty() || m.chars().any(char::is_control) || m.len() > 200 {
            return Err(bad("models", format!("model id {m:?} is empty, has a control character or is too long")));
        }
        if spec.models[..i].contains(m) {
            return Err(bad("models", format!("model id {m:?} is listed twice")));
        }
    }
    let (header, scheme, extra) = match spec.style {
        Style::OpenaiChat => ("Authorization", "bearer", ""),
        Style::AnthropicMessages => ("x-api-key", "raw", "headers = { \"anthropic-version\" = \"2023-06-01\" }\n"),
    };
    let mut out = format!(
        "# Custom compatible provider, written by 0router (schema 2).\nschema = 2\nid = {id}\ncategory = \"apikey\"\n\n\
         [display]\nname = {name}\n\n[auth]\nkind = \"apikey\"\nheader = \"{header}\"\nscheme = \"{scheme}\"\n\n\
         [endpoints.text]\nurl = {url}\nwire = \"{wire}\"\n{extra}",
        id = q(spec.id),
        name = q(name),
        url = q(&url),
        wire = spec.style.wire(),
    );
    for m in spec.models {
        out.push_str(&format!("\n[[models]]\nid = {}\nname = {}\n", q(m), q(m)));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OperatorHome, check_user_plugin};
    use std::path::Path;

    fn spec<'a>(id: &'a str, style: Style, url: &'a str, models: &'a [String]) -> Spec<'a> {
        Spec { id, name: "", style, base_url: url, models }
    }

    fn models() -> Vec<String> {
        vec!["m1".into(), "m2".into()]
    }

    #[test]
    fn both_styles_build_a_plugin_the_gate_accepts() {
        let m = models();
        for (style, url, want, header) in [
            (
                Style::OpenaiChat,
                "https://llm.example.com",
                "https://llm.example.com/v1/chat/completions",
                "Authorization",
            ),
            (
                Style::OpenaiChat,
                "https://llm.example.com/api/",
                "https://llm.example.com/api/chat/completions",
                "Authorization",
            ),
            (
                Style::AnthropicMessages,
                "https://llm.example.com/v1",
                "https://llm.example.com/v1/messages",
                "x-api-key",
            ),
        ] {
            let src = build(&spec("my-llm", style, url, &m)).unwrap();
            assert!(src.contains(want) && src.contains(header), "{src}");
            let verdict = check_user_plugin(&src, Path::new("plugins/my-llm.toml"), false).unwrap();
            assert!(verdict.fits(), "{src}");
        }
    }

    #[test]
    fn the_built_plugin_loads_with_its_models() {
        let dir = tempfile::tempdir().unwrap();
        let home = OperatorHome::new(dir.path().to_owned());
        std::fs::create_dir_all(home.plugins_dir()).unwrap();
        let m = models();
        let src = build(&spec("my-llm", Style::OpenaiChat, "https://llm.example.com/v1", &m)).unwrap();
        std::fs::write(home.plugins_dir().join("my-llm.toml"), src).unwrap();
        let reg = crate::RegistryHandle::open(home).unwrap();
        assert!(reg.snapshot().provider("my-llm").is_ok());
    }

    #[test]
    fn bad_fields_name_their_field() {
        let m = models();
        let field = |s: Spec<'_>| build(&s).unwrap_err().field;
        assert_eq!(field(spec("Bad_ID", Style::OpenaiChat, "https://a.b", &m)), "id");
        assert_eq!(field(spec(&"a".repeat(33), Style::OpenaiChat, "https://a.b", &m)), "id");
        assert_eq!(field(spec("x", Style::OpenaiChat, "not a url", &m)), "base_url");
        assert_eq!(field(spec("x", Style::OpenaiChat, "ftp://a.b", &m)), "base_url");
        assert_eq!(field(spec("x", Style::OpenaiChat, "https://a.b", &[])), "models");
        let many: Vec<String> = (0..201).map(|i| format!("m{i}")).collect();
        assert_eq!(field(spec("x", Style::OpenaiChat, "https://a.b", &many)), "models");
        let dup = vec!["a".to_owned(), "a".to_owned()];
        assert_eq!(field(spec("x", Style::OpenaiChat, "https://a.b", &dup)), "models");
    }

    #[test]
    fn quotes_in_a_name_or_model_stay_data() {
        let m = vec!["a\"b".to_owned()];
        let s = Spec { id: "x", name: "N\"ame", style: Style::OpenaiChat, base_url: "https://a.b", models: &m };
        let src = build(&s).unwrap();
        assert!(check_user_plugin(&src, Path::new("x.toml"), false).unwrap().fits(), "{src}");
    }
}
