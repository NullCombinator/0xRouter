//! The validation gate for adapter source (research R7, contracts/adapter-package.md).
//!
//! [`check`] reads an unpacked package, never follows a link, and collects every reason it
//! refuses the package, sorted by file and line. Nothing here runs or builds adapter code.

use std::fmt;
use std::fs;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use proc_macro2::{TokenStream, TokenTree};
use syn::punctuated::Punctuated;
use syn::visit::{self, Visit};
use syn::{Attribute, Expr, ExprArray, Lit, Meta, Token};

use crate::HarnessName;
use crate::loader::Manifest;
use crate::selector::Selector;

const KIT: &str = "nullrouter-adapter-kit";
const MAX_TOTAL: u64 = 256 * 1024;
const MAX_FILES: usize = 64;
const MAX_RS: u64 = 64 * 1024;
const MAX_DEPTH: usize = 8;
const MAX_RUN: usize = 256;
const MAX_ARRAY: usize = 256;
const MAX_BYTES: usize = 128;
const MAX_SELECTORS: usize = 32;
const MAX_SEGMENTS: usize = 8;
const FORBIDDEN_MACROS: [&str; 8] =
    ["include", "include_str", "include_bytes", "env", "option_env", "asm", "global_asm", "concat_idents"];

/// One reason the gate refuses a package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reason {
    pub code: &'static str,
    /// Path relative to the package root.
    pub file: String,
    pub line: Option<usize>,
    pub message: String,
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "refused: {} at {}", self.code, self.file)?;
        if let Some(line) = self.line {
            write!(f, ":{line}")?;
        }
        write!(f, ": {}", self.message)
    }
}

/// A package that passed the gate.
#[derive(Debug)]
pub struct Gated {
    pub manifest: Manifest,
    pub package_name: String,
    pub package_version: String,
    /// Every file of the package, relative, sorted.
    pub files: Vec<String>,
}

fn reason(code: &'static str, file: &str, line: Option<usize>, message: impl Into<String>) -> Reason {
    Reason { code, file: file.to_owned(), line, message: message.into() }
}

/// Runs every gate rule on the tree at `root`. `styles` are the loaded client style ids.
pub fn check(root: &Path, styles: &[&str]) -> Result<Gated, Vec<Reason>> {
    let mut out = Vec::new();
    match fs::symlink_metadata(root) {
        Ok(m) if m.is_dir() => {}
        _ => return Err(vec![reason("file_not_allowed", ".", None, "not a directory")]),
    }
    let mut found = Vec::new();
    let mut exceeded = false;
    walk(root, "", 0, &mut found, &mut exceeded, &mut out);
    found.sort_by(|a, b| a.rel.cmp(&b.rel));

    let total: u64 = found.iter().map(|f| f.len).sum();
    if exceeded || found.len() > MAX_FILES {
        out.push(reason("too_large", ".", None, format!("more than {MAX_FILES} files")));
    }
    if total > MAX_TOTAL {
        out.push(reason("too_large", ".", None, "package over 256 KiB"));
    }

    let has = |name: &str| found.iter().any(|f| f.rel == name);
    for required in ["Cargo.toml", "adapter.toml", "src/lib.rs"] {
        if !has(required) {
            out.push(reason("manifest_missing", required, None, "required file is missing"));
        }
    }

    let mut cargo_ids = None;
    let mut manifest = None;
    let mut files = Vec::new();
    for f in &found {
        files.push(f.rel.clone());
        let is_rs = f.rel.starts_with("src/") && f.rel.ends_with(".rs");
        let allowed = is_rs
            || matches!(f.rel.as_str(), "Cargo.toml" | "adapter.toml" | "README.md" | "CHANGELOG.md")
            || (!f.rel.contains('/') && f.rel.starts_with("LICENSE"));
        if !allowed {
            if f.rel == "build.rs" {
                out.push(reason("build_script", &f.rel, None, "build scripts are not allowed"));
            } else {
                out.push(reason("file_not_allowed", &f.rel, None, "file is not part of the package layout"));
            }
            continue;
        }
        if is_rs && f.len > MAX_RS {
            out.push(reason("too_large", &f.rel, None, ".rs file over 64 KiB"));
            continue;
        }
        if f.len > MAX_TOTAL {
            continue;
        }
        let Some(text) = read_text(&f.path, &f.rel, &mut out) else { continue };
        match f.rel.as_str() {
            "Cargo.toml" => cargo_ids = check_cargo(&text, &mut out),
            "adapter.toml" => manifest = check_manifest(&text, styles, &mut out),
            _ if is_rs => check_rust(&text, &f.rel, &mut out),
            _ => {}
        }
    }

    out.sort_by(|a, b| (&a.file, a.line.unwrap_or(0)).cmp(&(&b.file, b.line.unwrap_or(0))));
    match (out.is_empty(), manifest, cargo_ids) {
        (true, Some(manifest), Some((package_name, package_version))) => {
            Ok(Gated { manifest, package_name, package_version, files })
        }
        (true, ..) => Err(vec![reason("manifest_invalid", ".", None, "package could not be read")]),
        _ => Err(out),
    }
}

struct Found {
    rel: String,
    path: PathBuf,
    len: u64,
}

fn walk(dir: &Path, rel: &str, depth: usize, found: &mut Vec<Found>, exceeded: &mut bool, out: &mut Vec<Reason>) {
    let shown = if rel.is_empty() { "." } else { rel };
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => {
            out.push(reason("file_not_allowed", shown, None, "directory cannot be read"));
            return;
        }
    };
    let mut names: Vec<_> = entries.filter_map(Result::ok).map(|e| e.file_name()).collect();
    names.sort();
    for name in names {
        if *exceeded {
            return;
        }
        let child_rel = if rel.is_empty() {
            name.to_string_lossy().into_owned()
        } else {
            format!("{rel}/{}", name.to_string_lossy())
        };
        let path = dir.join(&name);
        let Ok(meta) = fs::symlink_metadata(&path) else {
            out.push(reason("file_not_allowed", &child_rel, None, "file cannot be read"));
            continue;
        };
        let ft = meta.file_type();
        if ft.is_symlink() {
            out.push(reason("file_not_allowed", &child_rel, None, "symlink"));
        } else if ft.is_dir() {
            if depth >= MAX_DEPTH {
                out.push(reason("file_not_allowed", &child_rel, None, "directory nested too deep"));
            } else {
                walk(&path, &child_rel, depth + 1, found, exceeded, out);
            }
        } else if ft.is_file() {
            if meta.nlink() > 1 {
                out.push(reason("file_not_allowed", &child_rel, None, "hard link"));
            } else if found.len() >= MAX_FILES {
                *exceeded = true;
            } else {
                found.push(Found { rel: child_rel, path, len: meta.len() });
            }
        } else {
            out.push(reason("file_not_allowed", &child_rel, None, "special file"));
        }
    }
}

fn read_text(path: &Path, rel: &str, out: &mut Vec<Reason>) -> Option<String> {
    let mut bytes = Vec::new();
    let read = fs::File::open(path).and_then(|f| f.take(MAX_TOTAL + 1).read_to_end(&mut bytes));
    if read.is_err() {
        out.push(reason("file_not_allowed", rel, None, "file cannot be read"));
        return None;
    }
    match String::from_utf8(bytes) {
        Ok(s) if !s.contains('\0') => Some(s),
        _ => {
            out.push(reason("binary_file", rel, None, "file is binary"));
            None
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Cargo.toml

fn check_cargo(text: &str, out: &mut Vec<Reason>) -> Option<(String, String)> {
    const F: &str = "Cargo.toml";
    let table = match text.parse::<toml::Table>() {
        Ok(t) => t,
        Err(e) => {
            out.push(reason("manifest_invalid", F, None, e.message().to_owned()));
            return None;
        }
    };

    for key in ["dependencies", "dev-dependencies", "build-dependencies", "dev_dependencies", "build_dependencies"] {
        match table.get(key) {
            None => {}
            Some(toml::Value::Table(deps)) => {
                for (name, value) in deps {
                    if key == "dependencies" && name == KIT {
                        check_kit_dependency(value, out);
                    } else {
                        out.push(reason("foreign_dependency", F, None, format!("dependency {name:?} is not the kit")));
                    }
                }
            }
            Some(_) => out.push(reason("manifest_invalid", F, None, format!("key {key:?} must be a table"))),
        }
    }
    let kit_present = matches!(table.get("dependencies"), Some(toml::Value::Table(d)) if d.contains_key(KIT));
    if !kit_present {
        out.push(reason("manifest_invalid", F, None, format!("dependency {KIT:?} is required")));
    }

    let mut ids = None;
    match table.get("package") {
        Some(toml::Value::Table(pkg)) => {
            const OK: [&str; 12] = [
                "name",
                "version",
                "edition",
                "license",
                "description",
                "authors",
                "repository",
                "rust-version",
                "readme",
                "homepage",
                "documentation",
                "keywords",
            ];
            for key in pkg.keys() {
                let (code, message) = match key.as_str() {
                    "build" => ("build_script", format!("key {key:?} is not allowed")),
                    "proc-macro" | "proc_macro" => ("proc_macro", format!("key {key:?} is not allowed")),
                    k if OK.contains(&k) => continue,
                    _ => ("cargo_table_not_allowed", format!("key {key:?} is not allowed")),
                };
                out.push(reason(code, F, None, message));
            }
            let text_of = |k: &str| pkg.get(k).and_then(toml::Value::as_str).map(str::to_owned);
            for required in ["name", "version", "edition"] {
                if text_of(required).is_none() {
                    out.push(reason(
                        "manifest_invalid",
                        F,
                        None,
                        format!("key \"package.{required}\" is required and must be a string"),
                    ));
                }
            }
            if let (Some(n), Some(v)) = (text_of("name"), text_of("version")) {
                ids = Some((n, v));
            }
        }
        _ => out.push(reason("manifest_invalid", F, None, "table [package] is required")),
    }

    for (key, value) in &table {
        match key.as_str() {
            "package" | "dependencies" | "dev-dependencies" | "build-dependencies" | "dev_dependencies"
            | "build_dependencies" => {}
            "features" => {
                let only_default = matches!(value, toml::Value::Table(t)
                    if t.iter().all(|(k, v)| k == "default" && matches!(v, toml::Value::Array(a) if a.is_empty())));
                if !only_default {
                    out.push(reason("cargo_table_not_allowed", F, None, "table [features] is not allowed"));
                }
            }
            _ => {
                if let toml::Value::Table(t) = value {
                    for k in t.keys().filter(|k| matches!(k.as_str(), "proc-macro" | "proc_macro")) {
                        out.push(reason("proc_macro", F, None, format!("key {k:?} is not allowed")));
                    }
                }
                let shown =
                    if matches!(value, toml::Value::Array(_)) { format!("[[{key}]]") } else { format!("[{key}]") };
                out.push(reason("cargo_table_not_allowed", F, None, format!("table {shown} is not allowed")));
            }
        }
    }
    ids
}

fn check_kit_dependency(value: &toml::Value, out: &mut Vec<Reason>) {
    const F: &str = "Cargo.toml";
    match value {
        toml::Value::String(req) => {
            if semver::VersionReq::parse(req).is_err() {
                out.push(reason(
                    "manifest_invalid",
                    F,
                    None,
                    format!("dependency {KIT:?} has no valid version requirement"),
                ));
            }
        }
        toml::Value::Table(t) => {
            for key in t.keys().filter(|k| *k != "version") {
                out.push(reason(
                    "dependency_source",
                    F,
                    None,
                    format!("dependency {KIT:?} key {key:?} is not allowed"),
                ));
            }
            if !matches!(t.get("version"), Some(toml::Value::String(_))) {
                out.push(reason(
                    "manifest_invalid",
                    F,
                    None,
                    format!("dependency {KIT:?} needs a plain version requirement"),
                ));
            }
        }
        _ => out.push(reason("manifest_invalid", F, None, format!("dependency {KIT:?} is malformed"))),
    }
}

// ---------------------------------------------------------------------------------------------
// adapter.toml

fn check_manifest(text: &str, styles: &[&str], out: &mut Vec<Reason>) -> Option<Manifest> {
    const F: &str = "adapter.toml";
    let manifest = match Manifest::parse(text) {
        Ok(m) => m,
        Err(message) => {
            let message = match message.strip_prefix("unknown field `").and_then(|r| r.split('`').next()) {
                Some(key) => format!("key {key:?} is not allowed"),
                None => message,
            };
            out.push(reason("manifest_invalid", F, None, message));
            return None;
        }
    };
    let before = out.len();

    match HarnessName::new(&manifest.harness) {
        Err(crate::HarnessNameError::Malformed(h)) => {
            out.push(reason("harness_invalid", F, None, format!("key \"harness\": {h:?} is malformed")));
        }
        Err(crate::HarnessNameError::Reserved(h)) => {
            out.push(reason("harness_invalid", F, None, format!("key \"harness\": {h:?} is reserved")));
        }
        Ok(name) if name.is_builtin() => {
            out.push(reason("harness_invalid", F, None, format!("key \"harness\": {:?} is built in", name.as_str())));
        }
        Ok(_) => {}
    }
    if !styles.contains(&manifest.style.as_str()) {
        out.push(reason(
            "style_unknown",
            F,
            None,
            format!("key \"style\": {:?} is not a loaded style", manifest.style),
        ));
    }
    if semver::VersionReq::parse(&manifest.kit).is_err() {
        out.push(reason(
            "manifest_invalid",
            F,
            None,
            format!("key \"kit\": {:?} is not a version requirement", manifest.kit),
        ));
    }
    if manifest.summary.chars().count() > 200 {
        out.push(reason("manifest_invalid", F, None, "key \"summary\" is over 200 characters"));
    }
    check_selectors("request.selectors", &manifest.request.selectors, 1, out);
    check_selectors("response.selectors", &manifest.response.selectors, 0, out);
    (out.len() == before).then_some(manifest)
}

fn check_selectors(key: &str, list: &[String], min: usize, out: &mut Vec<Reason>) {
    const F: &str = "adapter.toml";
    if list.len() < min || list.len() > MAX_SELECTORS {
        out.push(reason(
            "selector_invalid",
            F,
            None,
            format!("key {key:?} needs {min} to {MAX_SELECTORS} selectors, has {}", list.len()),
        ));
        return;
    }
    for s in list {
        if let Err(e) = Selector::parse(s) {
            let message = if e.starts_with("more than") {
                match segment_count(s) {
                    Some(n) => format!("{s:?} has {n} segments, at most {MAX_SEGMENTS}"),
                    None => format!("{s:?}: {e}"),
                }
            } else {
                format!("{s:?}: {e}")
            };
            out.push(reason("selector_invalid", F, None, format!("key {key:?}: {message}")));
        }
    }
}

fn segment_count(s: &str) -> Option<usize> {
    let masked = s.replace("[*]", "[\"__nr_any__\"]");
    nullrouter_adapter_kit::Path::parse(&masked).ok().map(|p| p.0.len())
}

// ---------------------------------------------------------------------------------------------
// Rust source

fn check_rust(text: &str, rel: &str, out: &mut Vec<Reason>) {
    match syn::parse_file(text) {
        Ok(file) => {
            let mut scan = Scan { file: rel, out };
            scan.visit_file(&file);
        }
        Err(e) => {
            let line = e.span().start().line;
            out.push(reason("parse_error", rel, (line > 0).then_some(line), "does not parse"));
        }
    }
}

struct Scan<'a> {
    file: &'a str,
    out: &'a mut Vec<Reason>,
}

impl Scan<'_> {
    fn add(&mut self, code: &'static str, line: usize, message: impl Into<String>) {
        self.out.push(reason(code, self.file, (line > 0).then_some(line), message));
    }

    fn meta(&mut self, meta: &Meta, line: usize, depth: usize) {
        let name = meta.path().segments.last().map(|s| s.ident.to_string()).unwrap_or_default();
        match name.as_str() {
            "no_mangle" | "export_name" | "link_section" => {
                self.add("abi_attribute", line, format!("attribute #[{name}]"));
            }
            "link" => self.add("extern_block", line, "attribute #[link]"),
            "path" => self.add("path_attribute", line, "attribute #[path]"),
            "unsafe" | "cfg_attr" if depth < MAX_DEPTH => {
                if let Meta::List(list) = meta {
                    let skip = usize::from(name == "cfg_attr");
                    if let Ok(inner) = list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated) {
                        for m in inner.iter().skip(skip) {
                            self.meta(m, line, depth + 1);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn blob_text(&mut self, text: &[u8], line: usize) {
        if longest_b64_run(text) > MAX_RUN {
            self.add("opaque_blob", line, "base64 run over 256 characters");
        }
    }

    fn numbers(&mut self, nums: &[Option<u128>], chars: &str, line: usize) {
        self.blob_text(chars.as_bytes(), line);
        let count = nums.iter().flatten().count();
        if count > MAX_ARRAY {
            self.add("opaque_blob", line, "integer array over 256 elements");
            return;
        }
        let (mut run, mut best) = (0usize, 0usize);
        for n in nums {
            if matches!(n, Some(v) if *v <= 255) {
                run += 1;
                best = best.max(run);
            } else {
                run = 0;
            }
        }
        if best > MAX_BYTES {
            self.add("opaque_blob", line, "integer array encodes over 128 bytes");
        }
    }

    fn tokens(&mut self, ts: TokenStream, depth: usize) {
        if depth > 128 {
            return;
        }
        let toks: Vec<TokenTree> = ts.into_iter().collect();
        for (i, t) in toks.iter().enumerate() {
            match t {
                TokenTree::Ident(id) => {
                    let s = id.to_string();
                    let line = id.span().start().line;
                    if s == "unsafe" {
                        self.add("unsafe_code", line, "unsafe in macro input");
                    }
                    let bang = matches!(toks.get(i + 1), Some(TokenTree::Punct(p)) if p.as_char() == '!');
                    if bang && FORBIDDEN_MACROS.contains(&s.as_str()) {
                        self.add("forbidden_macro", line, format!("macro `{s}!`"));
                    }
                }
                TokenTree::Group(g) => {
                    self.tokens(g.stream(), depth + 1);
                    if g.delimiter() == proc_macro2::Delimiter::Bracket {
                        let line = g.span_open().start().line;
                        let mut nums = Vec::new();
                        let mut chars = String::new();
                        for inner in g.stream() {
                            if let TokenTree::Literal(l) = inner
                                && let Ok(lit) = syn::parse2::<Lit>(TokenStream::from(TokenTree::Literal(l)))
                            {
                                nums.push(lit_number(&lit));
                                if let Lit::Char(c) = &lit {
                                    chars.push(c.value());
                                }
                            }
                        }
                        self.numbers(&nums, &chars, line);
                    }
                }
                TokenTree::Literal(l) => {
                    let line = l.span().start().line;
                    match syn::parse2::<Lit>(TokenStream::from(TokenTree::Literal(l.clone()))) {
                        Ok(Lit::Str(s)) => self.blob_text(s.value().as_bytes(), line),
                        Ok(Lit::ByteStr(b)) => self.blob_text(&b.value(), line),
                        _ => {}
                    }
                }
                TokenTree::Punct(_) => {}
            }
        }
    }
}

fn lit_number(lit: &Lit) -> Option<u128> {
    match lit {
        Lit::Int(i) => i.base10_parse::<u128>().ok(),
        Lit::Byte(b) => Some(u128::from(b.value())),
        _ => None,
    }
}

/// The longest run of characters from the base64 alphabet (standard or URL-safe). Hex is a
/// subset of it, so a hex run over the limit is caught here too.
fn longest_b64_run(text: &[u8]) -> usize {
    let (mut run, mut best) = (0usize, 0usize);
    for b in text {
        if b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'-' | b'_') {
            run += 1;
            best = best.max(run);
        } else {
            run = 0;
        }
    }
    best
}

impl<'ast> Visit<'ast> for Scan<'_> {
    fn visit_expr_unsafe(&mut self, i: &'ast syn::ExprUnsafe) {
        self.add("unsafe_code", i.unsafe_token.span.start().line, "unsafe block");
        visit::visit_expr_unsafe(self, i);
    }

    fn visit_signature(&mut self, i: &'ast syn::Signature) {
        if let Some(u) = &i.unsafety {
            self.add("unsafe_code", u.span.start().line, "unsafe fn");
        }
        visit::visit_signature(self, i);
    }

    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        if let Some(u) = &i.unsafety {
            self.add("unsafe_code", u.span.start().line, "unsafe impl");
        }
        visit::visit_item_impl(self, i);
    }

    fn visit_item_trait(&mut self, i: &'ast syn::ItemTrait) {
        if let Some(u) = &i.unsafety {
            self.add("unsafe_code", u.span.start().line, "unsafe trait");
        }
        visit::visit_item_trait(self, i);
    }

    fn visit_item_foreign_mod(&mut self, i: &'ast syn::ItemForeignMod) {
        self.add("extern_block", i.abi.extern_token.span.start().line, "extern block");
        visit::visit_item_foreign_mod(self, i);
    }

    fn visit_item_extern_crate(&mut self, i: &'ast syn::ItemExternCrate) {
        let name = i.ident.to_string();
        if !matches!(name.as_str(), "std" | "core" | "alloc" | "nullrouter_adapter_kit") {
            self.add("extern_crate", i.extern_token.span.start().line, format!("extern crate {name:?}"));
        }
        visit::visit_item_extern_crate(self, i);
    }

    fn visit_attribute(&mut self, i: &'ast Attribute) {
        let line = i.pound_token.span.start().line;
        self.meta(&i.meta, line, 0);
        visit::visit_attribute(self, i);
    }

    fn visit_macro(&mut self, i: &'ast syn::Macro) {
        if let Some(last) = i.path.segments.last() {
            let name = last.ident.to_string();
            if FORBIDDEN_MACROS.contains(&name.as_str()) {
                self.add("forbidden_macro", last.ident.span().start().line, format!("macro `{name}!`"));
            }
        }
        self.tokens(i.tokens.clone(), 0);
        visit::visit_macro(self, i);
    }

    fn visit_lit_str(&mut self, i: &'ast syn::LitStr) {
        self.blob_text(i.value().as_bytes(), i.span().start().line);
    }

    fn visit_lit_byte_str(&mut self, i: &'ast syn::LitByteStr) {
        self.blob_text(&i.value(), i.span().start().line);
    }

    fn visit_expr_array(&mut self, i: &'ast ExprArray) {
        let mut nums = Vec::with_capacity(i.elems.len());
        let mut chars = String::new();
        for e in &i.elems {
            if let Expr::Lit(l) = e {
                nums.push(lit_number(&l.lit));
                if let Lit::Char(c) = &l.lit {
                    chars.push(c.value());
                }
            } else {
                nums.push(None);
            }
        }
        self.numbers(&nums, &chars, i.bracket_token.span.open().start().line);
        visit::visit_expr_array(self, i);
    }
}
