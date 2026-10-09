//! The style guide's rules (T028, T055, contracts/style-guide.md): every token traces to the 9router
//! line it comes from, outside `.dark {}`; tokens.css is generated from tokens.toml; dashboard.css
//! uses only tokens, keywords, 0, and percentages in a width or flex; and each rule uses only the
//! tokens of the components its selector names. `NR_BLESS=1` rewrites tokens.css.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const WIDTHS: &[&str] = &["width", "max-width", "min-width", "flex", "flex-basis"];

fn style(file: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("style").join(file);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn guide() -> toml::Table {
    style("tokens.toml").parse().expect("tokens.toml parses")
}

struct Token {
    name: String,
    value: String,
    source: String,
    class: Option<String>,
}

fn tokens(guide: &toml::Table) -> Vec<Token> {
    let field = |t: &toml::Value, k: &str| t.get(k).and_then(|v| v.as_str()).map(str::to_owned);
    guide["token"]
        .as_table()
        .expect("[token.*]")
        .iter()
        .map(|(name, t)| Token {
            name: name.clone(),
            value: field(t, "value").unwrap_or_else(|| panic!("{name}: no value")),
            source: field(t, "source").unwrap_or_else(|| panic!("{name}: no source")),
            class: field(t, "class"),
        })
        .collect()
}

/// Each component's `uses`, and its `source`.
fn components(guide: &toml::Table) -> Vec<(String, BTreeSet<String>, String)> {
    guide["component"]
        .as_table()
        .expect("[component.*]")
        .iter()
        .map(|(name, c)| {
            let uses = c["uses"].as_array().expect("uses").iter().map(|u| u.as_str().unwrap().to_owned()).collect();
            (name.clone(), uses, c["source"].as_str().expect("source").to_owned())
        })
        .collect()
}

fn render(tokens: &[Token]) -> String {
    let mut css = String::from("/* Generated from tokens.toml by tests/style_guide.rs (NR_BLESS=1); do not edit. */\n:root {\n");
    for t in tokens {
        css.push_str(&format!("  --{}: {};\n", t.name, t.value));
    }
    css.push_str("}\n");
    css
}

#[test]
fn tokens_css_is_generated_from_tokens_toml() {
    let want = render(&tokens(&guide()));
    if std::env::var_os("NR_BLESS").is_some() {
        fs::write(Path::new(env!("CARGO_MANIFEST_DIR")).join("style/tokens.css"), &want).unwrap();
    }
    assert_eq!(style("tokens.css"), want, "tokens.css is stale: run with NR_BLESS=1");
}

#[test]
fn the_guide_names_its_schema_and_tailwind() {
    let g = guide();
    assert_eq!(g["schema"].as_integer(), Some(1));
    assert!(g["tailwind"].as_str().is_some_and(|t| t.starts_with("4.")), "{:?}", g.get("tailwind"));
}

/// ref/9router, or `None` when it isn't checked out (CI clones it; a bare checkout may not have it).
fn reference() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ref/9router");
    if root.join("src").is_dir() {
        Some(root)
    } else {
        eprintln!("skipped: ref/9router is not checked out");
        None
    }
}

/// `file:line` → the line's text, or why it has none.
fn line(root: &Path, source: &str) -> Result<(String, usize, String), String> {
    let (file, n) = source.rsplit_once(':').ok_or(format!("{source}: not file:line"))?;
    let n: usize = n.parse().map_err(|_| format!("{source}: bad line"))?;
    let text = fs::read_to_string(root.join(file)).map_err(|e| format!("{source}: {e}"))?;
    let at = text.lines().nth(n.wrapping_sub(1)).ok_or(format!("{source}: no such line"))?.to_owned();
    Ok((text, n, at))
}

/// The lines of a stylesheet inside a block whose selector mentions `.dark`.
fn dark_lines(css: &str) -> BTreeSet<usize> {
    let (mut out, mut depth, mut dark_at) = (BTreeSet::new(), 0usize, None);
    for (i, line) in css.lines().enumerate() {
        if dark_at.is_some() {
            out.insert(i + 1);
        }
        for ch in line.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    if dark_at.is_none() && line.contains(".dark") {
                        dark_at = Some(depth);
                        out.insert(i + 1);
                    }
                }
                '}' => {
                    if dark_at == Some(depth) {
                        dark_at = None;
                    }
                    depth = depth.saturating_sub(1);
                }
                _ => {}
            }
        }
    }
    out
}

#[test]
fn every_token_traces_to_a_light_9router_line() {
    let Some(root) = reference() else { return };
    let mut bad = Vec::new();
    for t in tokens(&guide()) {
        match line(&root, &t.source) {
            Err(e) => bad.push(format!("{}: {e}", t.name)),
            Ok((text, n, at)) => {
                let want = t.class.as_deref().unwrap_or(&t.value);
                if !at.contains(want) {
                    bad.push(format!("{}: {} does not contain {want:?}", t.name, t.source));
                }
                if t.source.split(':').next().is_some_and(|f| f.ends_with(".css")) && dark_lines(&text).contains(&n) {
                    bad.push(format!("{}: {} is inside .dark", t.name, t.source));
                }
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn every_component_source_exists() {
    let Some(root) = reference() else { return };
    let bad: Vec<_> = components(&guide())
        .into_iter()
        .filter_map(|(name, _, source)| line(&root, &source).err().map(|e| format!("{name}: {e}")))
        .collect();
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// dashboard.css as (selector, [(property, value)]), comments removed. It has no nested blocks.
fn rules() -> Vec<(String, Vec<(String, String)>)> {
    let mut css = style("dashboard.css");
    while let Some(start) = css.find("/*") {
        let end = css[start..].find("*/").map_or(css.len(), |e| start + e + 2);
        css.replace_range(start..end, "");
    }
    css.split('}')
        .filter(|r| !r.trim().is_empty())
        .map(|r| {
            let (sel, body) = r.split_once('{').unwrap_or_else(|| panic!("not a rule: {r}"));
            let decls = body
                .split(';')
                .filter(|d| !d.trim().is_empty())
                .map(|d| {
                    let (p, v) = d.split_once(':').unwrap_or_else(|| panic!("not a declaration: {d}"));
                    (p.trim().to_owned(), v.trim().to_owned())
                })
                .collect();
            (sel.trim().to_owned(), decls)
        })
        .collect()
}

/// The tokens a value refers to, or the first part of it the rules don't allow.
fn value_vars(property: &str, value: &str) -> Result<Vec<String>, String> {
    let (b, mut i, mut vars) = (value.as_bytes(), 0, Vec::new());
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'-' || c == b'_';
    while i < b.len() {
        let c = b[i];
        if value[i..].starts_with("var(--") {
            let end = value[i..].find(')').ok_or("unclosed var(")? + i;
            vars.push(value[i + 6..end].to_owned());
            i = end + 1;
        } else if c.is_ascii_alphabetic() || (c == b'-' && b.get(i + 1).is_some_and(|n| n.is_ascii_alphabetic())) {
            while i < b.len() && ident(b[i]) {
                i += 1;
            }
        } else if c.is_ascii_digit() || c == b'.' || (c == b'-' && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            let start = i;
            i += 1;
            while i < b.len() && (ident(b[i]) || b[i] == b'.' || b[i] == b'%') {
                i += 1;
            }
            let n = &value[start..i];
            let width = n.ends_with('%') && WIDTHS.contains(&property);
            if n != "0" && !width {
                return Err(format!("{n} (a number: use a token)"));
            }
        } else if c == b'"' {
            i = value[i + 1..].find('"').ok_or("unclosed string")? + i + 2;
        } else if c.is_ascii_whitespace() || b"(),/+*-".contains(&c) {
            i += 1;
        } else {
            return Err(format!("{:?}", &value[i..]));
        }
    }
    Ok(vars)
}

#[test]
fn dashboard_css_uses_only_tokens_keywords_zero_and_widths() {
    let names: BTreeSet<_> = tokens(&guide()).into_iter().map(|t| t.name).collect();
    let mut bad = Vec::new();
    for (sel, decls) in rules() {
        for (p, v) in decls {
            match value_vars(&p, &v) {
                Err(e) => bad.push(format!("{sel} {{ {p}: {v} }}: {e}")),
                Ok(vars) => bad.extend(
                    vars.into_iter().filter(|v| !names.contains(v)).map(|v| format!("{sel}: --{v} is not a token")),
                ),
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// The components a selector names: each class up to `__` or `--`, and `page` for a selector with
/// no class.
fn selector_components(sel: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for one in sel.split(',') {
        let classes: Vec<&str> = one
            .split('.')
            .skip(1)
            .map(|c| c.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')).next().unwrap())
            .collect();
        if classes.is_empty() {
            out.insert("page".to_owned());
        }
        for c in classes {
            let end = [c.find("__"), c.find("--")].into_iter().flatten().min().unwrap_or(c.len());
            out.insert(c[..end].to_owned());
        }
    }
    out
}

#[test]
fn every_rule_uses_only_its_components_tokens() {
    let components = components(&guide());
    let mut bad = Vec::new();
    for (sel, decls) in rules() {
        let mut allowed = BTreeSet::new();
        for name in selector_components(&sel) {
            match components.iter().find(|(c, ..)| *c == name) {
                Some((_, uses, _)) => allowed.extend(uses.iter().cloned()),
                None => bad.push(format!("{sel}: {name} is not a [component.*]")),
            }
        }
        for (p, v) in decls {
            for var in value_vars(&p, &v).unwrap_or_default() {
                if !allowed.contains(&var) {
                    bad.push(format!("{sel} {{ {p} }}: --{var} is not in its components' uses"));
                }
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn every_component_is_used_and_every_use_is_a_token() {
    let g = guide();
    let names: BTreeSet<_> = tokens(&g).into_iter().map(|t| t.name).collect();
    let used: BTreeSet<_> = rules().iter().flat_map(|(sel, _)| selector_components(sel)).collect();
    let mut bad = Vec::new();
    for (name, uses, _) in components(&g) {
        if !used.contains(&name) {
            bad.push(format!("{name}: no rule in dashboard.css"));
        }
        bad.extend(uses.iter().filter(|u| !names.contains(*u)).map(|u| format!("{name}: uses unknown --{u}")));
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn the_value_rules_catch_what_they_forbid() {
    assert!(value_vars("color", "#fff").is_err());
    assert!(value_vars("margin", "4px").is_err());
    assert!(value_vars("height", "50%").is_err());
    assert!(value_vars("color", "red !important").is_err());
    assert_eq!(value_vars("width", "50%"), Ok(vec![]));
    assert_eq!(value_vars("margin", "0 auto"), Ok(vec![]));
    assert_eq!(
        value_vars("bottom", "calc(var(--space-6) + var(--size-14))"),
        Ok(vec!["space-6".to_owned(), "size-14".to_owned()])
    );
    assert_eq!(selector_components(".a__b .c--d:hover, td"), ["a", "c", "page"].map(String::from).into());
}
