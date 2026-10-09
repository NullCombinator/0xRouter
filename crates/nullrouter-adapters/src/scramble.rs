//! The scrambler (research R10): what an LLM reviewer sees instead of the adapter source.
//!
//! Every name the adapter defines is renamed to `v1`, `v2`, ... in first-seen order, with one
//! map for the whole package. Comments and `#[doc]` attributes are dropped. Kit, `std`, `core`
//! and `alloc` paths, and string literals, are kept, since the reviewer needs them to judge
//! what the adapter does. The source is parsed, never run.
//!
//! Renaming goes by name, not by scope: a defined name is replaced wherever it occurs, except
//! inside a path that starts at a kept root. That over-renames at worst (a method call that
//! shares a name with a defined field), and never leaves an author-chosen name behind.
//!
//! Macro bodies are token streams. Inside them every identifier in the map is renamed, kept
//! paths are skipped, and names introduced there (`let x`, `fn f`, `$meta`, lifetimes) join the
//! map, so a definition hidden in a `macro_rules!` body does not leak either. Names inside
//! string literals, including inline `{name}` format captures, are left as written.

use std::collections::{HashMap, HashSet};

use proc_macro2::{Delimiter, Group, Ident, Spacing, TokenStream, TokenTree};
use serde::Serialize;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::visit_mut::{self, VisitMut};

/// Path roots whose names the reviewer keeps.
const KEEP_ROOTS: &[&str] = &["std", "core", "alloc", "nullrouter_adapter_kit", "serde_json", "serde"];

/// Names that are never renamed: keywords and prelude constructors.
const KEEP_NAMES: &[&str] = &[
    "_", "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
    "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move",
    "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true",
    "type", "unsafe", "use", "where", "while", "union", "macro_rules", "None", "Some", "Ok", "Err",
];

/// Keywords whose next identifier, inside a macro body, is a new name.
const DEFINERS: &[&str] = &["fn", "struct", "enum", "const", "static", "mod", "trait", "type", "union", "let"];

/// Why a package could not be scrambled.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScrambleError {
    #[error("{path}: {message}")]
    Parse { path: String, message: String },
}

/// One top-level item's place before and after scrambling. Kept on the operator's side
/// (`review.json`), never sent to the reviewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LineMapEntry {
    /// 1-based line in the scrambled text.
    pub scrambled_line: u32,
    /// 1-based line in the original file (0 when the span is unknown).
    pub original_line: u32,
}

/// One scrambled source file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScrambledFile {
    /// The path as given, such as `src/lib.rs`.
    pub path: String,
    /// The scrambled text, printed by `prettyplease`.
    pub text: String,
    /// Where each top-level item moved.
    pub line_map: Vec<LineMapEntry>,
}

/// Scrambles a package: `(path, text)` pairs in, one [`ScrambledFile`] per file out, in the
/// same order. The same input always gives the same output.
pub fn scramble(files: &[(String, String)]) -> Result<Vec<ScrambledFile>, ScrambleError> {
    let mut parsed = Vec::with_capacity(files.len());
    for (path, text) in files {
        parsed.push(parse(path, text)?);
    }

    let mut collector = Collector::default();
    for file in &parsed {
        collector.visit_file(file);
    }
    let map = collector.into_map();

    let mut out = Vec::with_capacity(parsed.len());
    let mut renamer = Renamer { map };
    for ((path, _), mut file) in files.iter().zip(parsed) {
        let starts: Vec<u32> = file.items.iter().map(original_line).collect();
        renamer.visit_file_mut(&mut file);
        let text = prettyplease::unparse(&file);
        let line_map = line_map(&file, &text, &starts);
        out.push(ScrambledFile { path: path.clone(), text, line_map });
    }
    Ok(out)
}

fn parse(path: &str, text: &str) -> Result<syn::File, ScrambleError> {
    let err = |message: String| ScrambleError::Parse { path: path.to_owned(), message };
    let tokens: TokenStream = text.parse().map_err(|e| err(format!("{e}")))?;
    syn::parse2(strip_docs(tokens)).map_err(|e| err(e.to_string()))
}

fn original_line(item: &syn::Item) -> u32 {
    u32::try_from(item.span().start().line).unwrap_or(0)
}

/// Finds each top-level item's printed form in the whole text.
fn line_map(file: &syn::File, whole: &str, starts: &[u32]) -> Vec<LineMapEntry> {
    let mut entries = Vec::new();
    let mut cursor = 0usize;
    for (item, original) in file.items.iter().zip(starts) {
        let single = syn::File { shebang: None, attrs: Vec::new(), items: vec![item.clone()] };
        let piece = prettyplease::unparse(&single);
        let piece = piece.trim_end();
        let Some(rest) = whole.get(cursor..) else { break };
        let Some(found) = rest.find(piece) else { continue };
        let at = cursor + found;
        let line = whole.get(..at).map_or(0, |s| s.matches('\n').count()) + 1;
        entries.push(LineMapEntry {
            scrambled_line: u32::try_from(line).unwrap_or(u32::MAX),
            original_line: *original,
        });
        cursor = at + piece.len();
    }
    entries
}

// --- doc stripping, on tokens so that every node kind is covered ---

fn is_punct(tree: Option<&TokenTree>, c: char) -> bool {
    matches!(tree, Some(TokenTree::Punct(p)) if p.as_char() == c)
}

fn mentions_doc(stream: TokenStream) -> bool {
    stream.into_iter().any(|t| match t {
        TokenTree::Ident(i) => i == "doc",
        TokenTree::Group(g) => mentions_doc(g.stream()),
        _ => false,
    })
}

fn is_doc_attr(inner: TokenStream) -> bool {
    let mut it = inner.clone().into_iter();
    match it.next() {
        Some(TokenTree::Ident(i)) if i == "doc" => true,
        Some(TokenTree::Ident(i)) if i == "cfg_attr" => mentions_doc(inner),
        _ => false,
    }
}

/// Removes `#[doc ...]` and `#![doc ...]` (what `///` and `//!` lex to), at any depth.
fn strip_docs(stream: TokenStream) -> TokenStream {
    let trees: Vec<TokenTree> = stream.into_iter().collect();
    let mut out: Vec<TokenTree> = Vec::with_capacity(trees.len());
    let mut i = 0;
    while let Some(tree) = trees.get(i) {
        if is_punct(Some(tree), '#') {
            let j = if is_punct(trees.get(i + 1), '!') { i + 2 } else { i + 1 };
            if let Some(TokenTree::Group(g)) = trees.get(j) {
                if g.delimiter() == Delimiter::Bracket && is_doc_attr(g.stream()) {
                    i = j + 1;
                    continue;
                }
            }
        }
        match tree {
            TokenTree::Group(g) => {
                let mut ng = Group::new(g.delimiter(), strip_docs(g.stream()));
                ng.set_span(g.span());
                out.push(TokenTree::Group(ng));
            }
            other => out.push(other.clone()),
        }
        i += 1;
    }
    out.into_iter().collect()
}

// --- pass 1: what the adapter defines ---

#[derive(Default)]
struct Collector {
    order: Vec<String>,
    defined: HashSet<String>,
    seen: HashSet<String>,
    in_trait_impl: bool,
}

impl Collector {
    fn add(&mut self, name: &str) {
        if KEEP_NAMES.contains(&name) || KEEP_ROOTS.contains(&name) {
            return;
        }
        if self.defined.insert(name.to_owned()) {
            self.order.push(name.to_owned());
        }
    }

    fn add_ident(&mut self, ident: &Ident) {
        self.add(&ident.to_string());
    }

    fn tokens(&mut self, stream: TokenStream, in_macro_rules: bool) {
        let trees: Vec<TokenTree> = stream.into_iter().collect();
        for (i, tree) in trees.iter().enumerate() {
            match tree {
                TokenTree::Group(g) => self.tokens(g.stream(), in_macro_rules),
                TokenTree::Ident(id) => {
                    let name = id.to_string();
                    self.seen.insert(name.clone());
                    let prev = i.checked_sub(1).and_then(|p| trees.get(p));
                    let prev2 = i.checked_sub(2).and_then(|p| trees.get(p));
                    let introduced = match prev {
                        Some(TokenTree::Punct(p)) => {
                            (p.as_char() == '$' && in_macro_rules)
                                || (p.as_char() == '\'' && p.spacing() == Spacing::Joint)
                        }
                        Some(TokenTree::Ident(kw)) => {
                            let kw = kw.to_string();
                            DEFINERS.contains(&kw.as_str())
                                || ((kw == "mut" || kw == "ref")
                                    && matches!(prev2, Some(TokenTree::Ident(k))
                                        if ["let", "ref", "mut", "static"].iter().any(|w| k == w)))
                        }
                        _ => false,
                    };
                    if introduced {
                        self.add(&name);
                    }
                }
                _ => {}
            }
        }
    }

    /// Hands out `v1`, `v2`, ... in first-seen order, skipping any `vN` the source already
    /// uses for a name that is not being renamed.
    fn into_map(self) -> HashMap<String, String> {
        let mut map = HashMap::with_capacity(self.order.len());
        let mut n = 0usize;
        for name in &self.order {
            loop {
                n += 1;
                let candidate = format!("v{n}");
                if !self.seen.contains(&candidate) || self.defined.contains(&candidate) {
                    map.insert(name.clone(), candidate);
                    break;
                }
            }
        }
        map
    }
}

impl<'ast> Visit<'ast> for Collector {
    fn visit_ident(&mut self, i: &'ast Ident) {
        self.seen.insert(i.to_string());
    }

    fn visit_item_fn(&mut self, i: &'ast syn::ItemFn) {
        self.add_ident(&i.sig.ident);
        let outer = std::mem::replace(&mut self.in_trait_impl, false);
        visit::visit_item_fn(self, i);
        self.in_trait_impl = outer;
    }

    fn visit_item_struct(&mut self, i: &'ast syn::ItemStruct) {
        self.add_ident(&i.ident);
        visit::visit_item_struct(self, i);
    }

    fn visit_item_enum(&mut self, i: &'ast syn::ItemEnum) {
        self.add_ident(&i.ident);
        visit::visit_item_enum(self, i);
    }

    fn visit_item_union(&mut self, i: &'ast syn::ItemUnion) {
        self.add_ident(&i.ident);
        visit::visit_item_union(self, i);
    }

    fn visit_item_const(&mut self, i: &'ast syn::ItemConst) {
        self.add_ident(&i.ident);
        visit::visit_item_const(self, i);
    }

    fn visit_item_static(&mut self, i: &'ast syn::ItemStatic) {
        self.add_ident(&i.ident);
        visit::visit_item_static(self, i);
    }

    fn visit_item_mod(&mut self, i: &'ast syn::ItemMod) {
        self.add_ident(&i.ident);
        visit::visit_item_mod(self, i);
    }

    fn visit_item_trait(&mut self, i: &'ast syn::ItemTrait) {
        self.add_ident(&i.ident);
        visit::visit_item_trait(self, i);
    }

    fn visit_item_trait_alias(&mut self, i: &'ast syn::ItemTraitAlias) {
        self.add_ident(&i.ident);
        visit::visit_item_trait_alias(self, i);
    }

    fn visit_item_type(&mut self, i: &'ast syn::ItemType) {
        self.add_ident(&i.ident);
        visit::visit_item_type(self, i);
    }

    fn visit_item_macro(&mut self, i: &'ast syn::ItemMacro) {
        if let Some(ident) = &i.ident {
            self.add_ident(ident);
        }
        visit::visit_item_macro(self, i);
    }

    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        let outer = self.in_trait_impl;
        self.in_trait_impl = i.trait_.is_some();
        visit::visit_item_impl(self, i);
        self.in_trait_impl = outer;
    }

    // Methods of a trait impl belong to the trait. If the trait is the adapter's own, its
    // definition put the name in the map, and the by-name rename keeps both sides in step.
    fn visit_impl_item_fn(&mut self, i: &'ast syn::ImplItemFn) {
        if !self.in_trait_impl {
            self.add_ident(&i.sig.ident);
        }
        visit::visit_impl_item_fn(self, i);
    }

    fn visit_impl_item_const(&mut self, i: &'ast syn::ImplItemConst) {
        if !self.in_trait_impl {
            self.add_ident(&i.ident);
        }
        visit::visit_impl_item_const(self, i);
    }

    fn visit_impl_item_type(&mut self, i: &'ast syn::ImplItemType) {
        if !self.in_trait_impl {
            self.add_ident(&i.ident);
        }
        visit::visit_impl_item_type(self, i);
    }

    fn visit_trait_item_fn(&mut self, i: &'ast syn::TraitItemFn) {
        self.add_ident(&i.sig.ident);
        visit::visit_trait_item_fn(self, i);
    }

    fn visit_trait_item_const(&mut self, i: &'ast syn::TraitItemConst) {
        self.add_ident(&i.ident);
        visit::visit_trait_item_const(self, i);
    }

    fn visit_trait_item_type(&mut self, i: &'ast syn::TraitItemType) {
        self.add_ident(&i.ident);
        visit::visit_trait_item_type(self, i);
    }

    fn visit_field(&mut self, f: &'ast syn::Field) {
        if let Some(ident) = &f.ident {
            self.add_ident(ident);
        }
        visit::visit_field(self, f);
    }

    fn visit_variant(&mut self, v: &'ast syn::Variant) {
        self.add_ident(&v.ident);
        visit::visit_variant(self, v);
    }

    fn visit_pat_ident(&mut self, p: &'ast syn::PatIdent) {
        self.add_ident(&p.ident);
        visit::visit_pat_ident(self, p);
    }

    fn visit_type_param(&mut self, p: &'ast syn::TypeParam) {
        self.add_ident(&p.ident);
        visit::visit_type_param(self, p);
    }

    fn visit_const_param(&mut self, p: &'ast syn::ConstParam) {
        self.add_ident(&p.ident);
        visit::visit_const_param(self, p);
    }

    fn visit_lifetime_param(&mut self, p: &'ast syn::LifetimeParam) {
        self.add_ident(&p.lifetime.ident);
        visit::visit_lifetime_param(self, p);
    }

    fn visit_label(&mut self, l: &'ast syn::Label) {
        self.add_ident(&l.name.ident);
        visit::visit_label(self, l);
    }

    fn visit_use_rename(&mut self, r: &'ast syn::UseRename) {
        self.add_ident(&r.rename);
        visit::visit_use_rename(self, r);
    }

    fn visit_macro(&mut self, m: &'ast syn::Macro) {
        visit::visit_macro(self, m);
        self.tokens(m.tokens.clone(), m.path.is_ident("macro_rules"));
    }
}

// --- pass 2: rename ---

struct Renamer {
    map: HashMap<String, String>,
}

fn is_keep_root(ident: &Ident) -> bool {
    KEEP_ROOTS.iter().any(|r| ident == r)
}

fn is_path_sep(trees: &[TokenTree], i: usize) -> bool {
    matches!(trees.get(i), Some(TokenTree::Punct(p)) if p.as_char() == ':' && p.spacing() == Spacing::Joint)
        && is_punct(trees.get(i + 1), ':')
}

/// `$name:frag`: the fragment specifier stays, whatever the adapter defines.
fn is_fragment_spec(trees: &[TokenTree], i: usize) -> bool {
    i >= 3
        && is_punct(trees.get(i - 1), ':')
        && matches!(trees.get(i - 2), Some(TokenTree::Ident(_)))
        && is_punct(trees.get(i - 3), '$')
}

impl Renamer {
    fn rename(&self, ident: &mut Ident) {
        if let Some(new) = self.map.get(&ident.to_string()) {
            *ident = Ident::new(new, ident.span());
        }
    }

    fn tokens(&self, stream: TokenStream) -> TokenStream {
        let trees: Vec<TokenTree> = stream.into_iter().collect();
        let mut out: Vec<TokenTree> = Vec::with_capacity(trees.len());
        let mut i = 0;
        while let Some(tree) = trees.get(i) {
            match tree {
                TokenTree::Ident(id) if is_keep_root(id) && is_path_sep(&trees, i + 1) => {
                    // A kept path runs unchanged to its last segment.
                    out.push(tree.clone());
                    i += 1;
                    while is_path_sep(&trees, i) {
                        out.extend(trees.get(i..i + 2).unwrap_or_default().iter().cloned());
                        i += 2;
                        match trees.get(i) {
                            Some(next @ TokenTree::Ident(_)) => {
                                out.push(next.clone());
                                i += 1;
                            }
                            _ => break,
                        }
                    }
                    continue;
                }
                TokenTree::Ident(id) => {
                    let mut id = id.clone();
                    if !is_fragment_spec(&trees, i) {
                        self.rename(&mut id);
                    }
                    out.push(TokenTree::Ident(id));
                }
                TokenTree::Group(g) => {
                    let mut ng = Group::new(g.delimiter(), self.tokens(g.stream()));
                    ng.set_span(g.span());
                    out.push(TokenTree::Group(ng));
                }
                other @ (TokenTree::Punct(_) | TokenTree::Literal(_)) => out.push(other.clone()),
            }
            i += 1;
        }
        out.into_iter().collect()
    }

    fn use_tree(&mut self, tree: &mut syn::UseTree, kept: bool, top: bool) {
        match tree {
            syn::UseTree::Path(p) => {
                let k = kept || (top && is_keep_root(&p.ident));
                if !k {
                    self.rename(&mut p.ident);
                }
                self.use_tree(&mut p.tree, k, false);
            }
            syn::UseTree::Name(n) => {
                if !(kept || (top && is_keep_root(&n.ident))) {
                    self.rename(&mut n.ident);
                }
            }
            syn::UseTree::Rename(r) => {
                if !(kept || (top && is_keep_root(&r.ident))) {
                    self.rename(&mut r.ident);
                }
                self.rename(&mut r.rename);
            }
            syn::UseTree::Glob(_) => {}
            syn::UseTree::Group(g) => {
                for item in &mut g.items {
                    self.use_tree(item, kept, top);
                }
            }
        }
    }
}

impl VisitMut for Renamer {
    fn visit_ident_mut(&mut self, i: &mut Ident) {
        self.rename(i);
    }

    fn visit_path_mut(&mut self, p: &mut syn::Path) {
        let kept = p.segments.first().is_some_and(|s| is_keep_root(&s.ident));
        for seg in &mut p.segments {
            if !kept {
                self.rename(&mut seg.ident);
            }
            self.visit_path_arguments_mut(&mut seg.arguments);
        }
    }

    fn visit_item_use_mut(&mut self, i: &mut syn::ItemUse) {
        self.use_tree(&mut i.tree, false, true);
    }

    fn visit_macro_mut(&mut self, m: &mut syn::Macro) {
        self.visit_path_mut(&mut m.path);
        m.tokens = self.tokens(std::mem::take(&mut m.tokens));
    }

    fn visit_meta_list_mut(&mut self, m: &mut syn::MetaList) {
        visit_mut::visit_meta_list_mut(self, m);
        m.tokens = self.tokens(std::mem::take(&mut m.tokens));
    }
}

