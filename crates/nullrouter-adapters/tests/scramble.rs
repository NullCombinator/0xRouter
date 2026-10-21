use std::collections::BTreeSet;

use nullrouter_adapters::scramble::{scramble, ScrambledFile};
use proc_macro2::{TokenStream, TokenTree};
use syn::visit::{self, Visit};

const FIXTURE_SRC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/scramble/src");
const FIXTURE_FILES: [&str; 2] = ["lib.rs", "quirks.rs"];

fn fixture() -> Vec<(String, String)> {
    FIXTURE_FILES
        .iter()
        .map(|name| {
            let text = std::fs::read_to_string(format!("{FIXTURE_SRC}/{name}"))
                .expect("fixture file is readable");
            (format!("src/{name}"), text)
        })
        .collect()
}

fn scrambled() -> Vec<ScrambledFile> {
    scramble(&fixture()).expect("the fixture scrambles")
}

/// Collects every name that the fixture defines: items, fields, variants, bindings,
/// generic parameters, lifetimes and labels. Methods of trait impls are skipped, because
/// their names belong to the trait (kit or std) and are kept by the scrambler.
#[derive(Default)]
struct Defined {
    names: BTreeSet<String>,
    in_trait_impl: bool,
}

impl Defined {
    fn add(&mut self, ident: &syn::Ident) {
        self.names.insert(ident.to_string());
    }
}

impl<'ast> Visit<'ast> for Defined {
    fn visit_item_fn(&mut self, i: &'ast syn::ItemFn) {
        self.add(&i.sig.ident);
        let outer = std::mem::replace(&mut self.in_trait_impl, false);
        visit::visit_item_fn(self, i);
        self.in_trait_impl = outer;
    }

    fn visit_item_struct(&mut self, i: &'ast syn::ItemStruct) {
        self.add(&i.ident);
        visit::visit_item_struct(self, i);
    }

    fn visit_item_enum(&mut self, i: &'ast syn::ItemEnum) {
        self.add(&i.ident);
        visit::visit_item_enum(self, i);
    }

    fn visit_item_const(&mut self, i: &'ast syn::ItemConst) {
        self.add(&i.ident);
        visit::visit_item_const(self, i);
    }

    fn visit_item_static(&mut self, i: &'ast syn::ItemStatic) {
        self.add(&i.ident);
        visit::visit_item_static(self, i);
    }

    fn visit_item_mod(&mut self, i: &'ast syn::ItemMod) {
        self.add(&i.ident);
        visit::visit_item_mod(self, i);
    }

    fn visit_item_trait(&mut self, i: &'ast syn::ItemTrait) {
        self.add(&i.ident);
        visit::visit_item_trait(self, i);
    }

    fn visit_item_type(&mut self, i: &'ast syn::ItemType) {
        self.add(&i.ident);
        visit::visit_item_type(self, i);
    }

    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        let outer = self.in_trait_impl;
        self.in_trait_impl = i.trait_.is_some();
        visit::visit_item_impl(self, i);
        self.in_trait_impl = outer;
    }

    fn visit_impl_item_fn(&mut self, i: &'ast syn::ImplItemFn) {
        if !self.in_trait_impl {
            self.add(&i.sig.ident);
        }
        visit::visit_impl_item_fn(self, i);
    }

    fn visit_field(&mut self, f: &'ast syn::Field) {
        if let Some(ident) = &f.ident {
            self.add(ident);
        }
        visit::visit_field(self, f);
    }

    fn visit_variant(&mut self, v: &'ast syn::Variant) {
        self.add(&v.ident);
        visit::visit_variant(self, v);
    }

    fn visit_pat_ident(&mut self, p: &'ast syn::PatIdent) {
        self.add(&p.ident);
        visit::visit_pat_ident(self, p);
    }

    fn visit_type_param(&mut self, p: &'ast syn::TypeParam) {
        self.add(&p.ident);
        visit::visit_type_param(self, p);
    }

    fn visit_const_param(&mut self, p: &'ast syn::ConstParam) {
        self.add(&p.ident);
        visit::visit_const_param(self, p);
    }

    fn visit_lifetime_param(&mut self, p: &'ast syn::LifetimeParam) {
        self.add(&p.lifetime.ident);
        visit::visit_lifetime_param(self, p);
    }

    fn visit_label(&mut self, l: &'ast syn::Label) {
        self.add(&l.name.ident);
        visit::visit_label(self, l);
    }
}

fn defined_names(files: &[(String, String)]) -> BTreeSet<String> {
    let mut collector = Defined::default();
    for (path, text) in files {
        let file = syn::parse_file(text).unwrap_or_else(|e| panic!("{path} does not parse: {e}"));
        collector.visit_file(&file);
    }
    collector.names
}

/// Gathers every identifier token in a token stream, descending into groups. Lifetimes
/// show up as an identifier after a `'` punct, so `'body` yields `body`.
fn tokens_in(stream: TokenStream, found: &mut BTreeSet<String>) {
    for tree in stream {
        match tree {
            TokenTree::Ident(ident) => {
                found.insert(ident.to_string());
            }
            TokenTree::Group(group) => tokens_in(group.stream(), found),
            TokenTree::Punct(_) | TokenTree::Literal(_) => {}
        }
    }
}

#[test]
fn scrambling_the_fixture_succeeds() {
    let out = scramble(&fixture()).expect("the fixture scrambles");
    assert_eq!(out.len(), 2);
}

#[test]
fn collector_finds_the_names_the_fixture_defines() {
    let names = defined_names(&fixture());
    for expected in [
        "ClaudeQuirks",
        "ThinkingMode",
        "Adaptive",
        "thinking_mode",
        "first_block",
        "body",
        "TBlock",
        "scan",
        "cursor",
        "strip_server_tools",
        "SERVER_TOOL_KINDS",
        "quirks",
    ] {
        assert!(names.contains(expected), "collector missed {expected:?}: {names:?}");
    }
    assert!(!names.contains("on_request"), "trait impl method names belong to the kit");
    assert!(!names.contains("Adapter"), "kit names are not defined by the adapter");
}

#[test]
fn scrambled_text_keeps_no_defined_identifier_as_a_token() {
    let defined = defined_names(&fixture());
    for file in scrambled() {
        let tokens: TokenStream = file
            .text
            .parse()
            .unwrap_or_else(|e| panic!("{} does not tokenize: {e}", file.path));
        let mut found = BTreeSet::new();
        tokens_in(tokens, &mut found);
        let leaked: Vec<&String> = defined.iter().filter(|n| found.contains(*n)).collect();
        assert!(leaked.is_empty(), "{} still has {leaked:?}", file.path);
    }
}

#[test]
fn scrambled_text_has_no_comment_or_doc_markers() {
    for file in scrambled() {
        for marker in ["//", "/*", "///", "#[doc"] {
            assert!(!file.text.contains(marker), "{} still contains {marker:?}", file.path);
        }
    }
}

#[test]
fn kit_and_std_names_and_string_literals_survive() {
    let all: Vec<String> = scrambled().into_iter().map(|f| f.text).collect();
    let all = all.join("\n");
    for kept in [
        "nullrouter_adapter_kit",
        "Adapter",
        "on_request",
        "std",
        "mem",
        "take",
        "String",
        "\"anthropic-beta\"",
    ] {
        assert!(all.contains(kept), "scrambled output lost {kept:?}");
    }
}

#[test]
fn scrambled_files_still_parse() {
    for file in scrambled() {
        syn::parse_file(&file.text).unwrap_or_else(|e| panic!("{} does not parse: {e}", file.path));
    }
}

#[test]
fn scrambling_twice_gives_identical_text() {
    let first = scrambled();
    let second = scrambled();
    assert_eq!(first.len(), second.len());
    for (a, b) in first.iter().zip(&second) {
        assert_eq!(a.path, b.path);
        assert_eq!(a.text, b.text);
    }
}
