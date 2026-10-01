//! US4 s1–s4, SC-003: the validation gate over `tests/gate/`.

use std::fs;
use std::path::{Path, PathBuf};

use zerorouter_registry::validate::{
    GateCtx, ValidationError, check_route_collisions, validate, validate_style, validate_with,
};
use zerorouter_registry::{
    CapabilityKind, OperatorHome, PluginSource, RegistryHandle, bundled_gate_ctx, bundled_sources,
    bundled_style_sources, community, validate_user_plugin,
};

/// Rules whose error must list the allowed values.
const ENUM_RULES: &[&str] = &[
    "bad-category",
    "unknown-capability",
    "unknown-executor-param",
    "unknown-format",
    "unknown-hook",
    "unknown-key",
    "unknown-oauth-param",
    "unknown-quirk",
];

fn corpus(dir: &str) -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gate").join(dir);
    let mut files: Vec<_> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_file()).collect();
    files.sort();
    files
}

fn load(path: &Path) -> zerorouter_registry::ProviderEntity {
    let src = fs::read_to_string(path).unwrap();
    validate_user_plugin(&src, path).unwrap_or_else(|e| panic!("{}: {e:?}", path.display()))
}

fn valid(name: &str) -> zerorouter_registry::ProviderEntity {
    load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gate/valid").join(name))
}

#[test]
fn valid_corpus_is_accepted() {
    let files = corpus("valid");
    assert_eq!(files.len(), 4);
    for f in &files {
        load(f);
    }
    assert!(valid("minimal.toml").transport.is_none());
}

#[test]
fn invalid_corpus_is_rejected_with_one_positioned_error() {
    let files = corpus("invalid");
    assert_eq!(files.len(), 23);
    for path in files {
        let src = fs::read_to_string(&path).unwrap();
        let rule = path.file_stem().unwrap().to_str().unwrap();
        let expect = src.lines().next().and_then(|l| l.strip_prefix("# expect: ")).expect("# expect: line");
        let errors = validate_user_plugin(&src, &path).expect_err(rule);
        assert_eq!(errors.len(), 1, "{rule}: {errors:?}");
        let e = &errors[0];
        let text = e.to_string();
        let file = path.display().to_string();
        assert!(e.line > 0 && e.col > 0, "{rule}: unpositioned: {text}");
        assert!(text.starts_with(&format!("{file}:{}:{}", e.line, e.col)), "{rule}: {text}");
        assert!(text.contains(expect), "{rule}: {text:?} lacks {expect:?}");
        if !e.path.is_root() {
            assert!(text.contains(&format!(" {}: ", e.path)), "{rule}: {text}");
        }
        if ENUM_RULES.contains(&rule) {
            assert!(text.contains("allowed: ") || text.contains("expected one of "), "{rule}: {text}");
        }
    }
}

#[test]
fn sections_are_capabilities() {
    let p = valid("llm-embedding.toml");
    assert!(p.capabilities.contains_key(&CapabilityKind::Llm));
    assert!(p.capabilities.contains_key(&CapabilityKind::Embedding));
    assert_eq!(p.capabilities.len(), 2);

    let home = tempfile::tempdir().unwrap();
    fs::create_dir(home.path().join("plugins")).unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gate/valid/llm-embedding.toml");
    fs::copy(src, home.path().join("plugins/two-kinds.toml")).unwrap();
    let reg = RegistryHandle::open(OperatorHome::new(home.path())).unwrap().snapshot();
    assert!(reg.capability("two-kinds", CapabilityKind::Llm).unwrap().is_some());
    assert!(reg.capability("two-kinds", CapabilityKind::Embedding).unwrap().is_some());
    assert!(reg.capability("two-kinds", CapabilityKind::Tts).unwrap().is_none());
}

#[test]
fn bare_models_get_ids_and_derived_names() {
    let p = valid("bare-models.toml");
    let models = p.models.as_deref().unwrap();
    assert_eq!(models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["m-a", "m-b"]);

    let home = tempfile::tempdir().unwrap();
    fs::create_dir(home.path().join("plugins")).unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gate/valid/bare-models.toml");
    fs::copy(src, home.path().join("plugins/bare.toml")).unwrap();
    // A catalog-only plugin: slice 002's set, with the fit check off.
    let reg = RegistryHandle::open_parity(OperatorHome::new(home.path())).unwrap().snapshot();
    let info = reg.model("bare", "m-a").unwrap();
    assert!(info.declared);
    assert_eq!(info.name, zerorouter_registry::derive_model_name("m-a"));
}

/// FR-005: absent `models` (catalog unknown) differs from `models = []` (offers none).
#[test]
fn catalog_unknown_is_not_catalog_empty() {
    assert!(valid("minimal.toml").models.is_none());
    let (_, zed) = community::COMMUNITY.iter().find(|(f, _)| *f == "zed.toml").unwrap();
    assert!(zed.lines().any(|l| l.trim() == "models = []"), "zed.toml no longer declares an empty catalog");
    let zed = validate(zed, PluginSource::Bundled, "zed.toml").unwrap();
    assert_eq!(zed.models.as_deref(), Some(&[][..]));
}

#[test]
fn every_bundled_and_community_plugin_passes_the_gate() {
    assert_eq!(bundled_sources().len(), 5);
    assert_eq!(community::COMMUNITY.len(), 116);
    // With the bundled styles loaded, in the bundled set's strict mode. (A community plugin's
    // `credential_fallback` may name another community plugin: that is the fit check's.)
    let strict = ctx(true, false);
    for (file, src) in bundled_sources().iter().chain(community::COMMUNITY) {
        validate_with(src, PluginSource::Bundled, file, &strict).unwrap_or_else(|e| panic!("{file}: {e:?}"));
    }
}

/// The plugins kept by hand rather than generated from 9router.
const HAND_MAINTAINED: [&str; 5] =
    ["anthropic.toml", "elevenlabs.toml", "opencode-go.toml", "opencode-zen.toml", "openrouter.toml"];

/// The `# expect:` first line of a corpus file.
fn expect(src: &str) -> &str {
    src.lines().next().and_then(|l| l.strip_prefix("# expect: ")).expect("# expect: line")
}

/// Exactly one positioned error, naming `file` and containing the expected text.
fn one_error(rule: &str, file: &str, want: &str, errors: &[ValidationError]) {
    assert_eq!(errors.len(), 1, "{rule}: {errors:#?}");
    let text = errors[0].to_string();
    assert!(errors[0].line > 0 && errors[0].col > 0, "{rule}: unpositioned: {text}");
    assert!(text.starts_with(file), "{rule}: {text}");
    assert!(text.contains(want), "{rule}: {text:?} lacks {want:?}");
}

fn ctx(strict: bool, allow_private: bool) -> GateCtx {
    bundled_gate_ctx(strict, allow_private).expect("bundled styles load")
}

/// US7, T117: each style case fails with its one expected error; the two collision files
/// pass alone, and together get one collision error each.
#[test]
fn style_corpus_is_rejected_with_one_positioned_error() {
    let files = corpus("invalid/styles");
    assert_eq!(files.len(), 13);
    let mut pair = Vec::new();
    for path in &files {
        let src = fs::read_to_string(path).unwrap();
        let rule = path.file_stem().unwrap().to_str().unwrap();
        let file = path.display().to_string();
        if rule.starts_with("route-collision") {
            let style = validate_style(&src, &file).unwrap_or_else(|e| panic!("{rule}: {e:#?}"));
            pair.push((file, src, style));
            continue;
        }
        one_error(rule, &file, expect(&src), &validate_style(&src, &file).expect_err(rule));
    }
    let pair: Vec<_> = pair.iter().map(|(f, s, st)| (f.as_str(), s.as_str(), st)).collect();
    let errors = check_route_collisions(&pair);
    // One error per file, each at that file's route.
    assert_eq!(errors.len(), 2, "{errors:#?}");
    for ((file, src, _), e) in pair.iter().zip(&errors) {
        one_error("route-collision", file, expect(src), std::slice::from_ref(e));
    }
}

/// US7, T118: each schema-2 case fails with its one expected error, gated as a user plugin
/// is against the bundled styles.
#[test]
fn provider_corpus_is_rejected_with_one_positioned_error() {
    let files = corpus("invalid/providers");
    assert_eq!(files.len(), 16);
    for path in &files {
        let src = fs::read_to_string(path).unwrap();
        let rule = path.file_stem().unwrap().to_str().unwrap();
        let file = path.display().to_string();
        let errors =
            validate_with(&src, PluginSource::User(path.clone()), &file, &ctx(false, false)).err().unwrap_or_default();
        one_error(rule, &file, expect(&src), &errors);
    }
}

#[test]
fn a_private_endpoint_passes_when_the_operator_allows_it() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gate/invalid/providers/url-localhost.toml");
    let src = fs::read_to_string(&path).unwrap();
    let g = validate_with(&src, PluginSource::User(path.clone()), "url-localhost.toml", &ctx(false, true)).unwrap();
    assert!(g.diagnostics.is_empty(), "{:?}", g.diagnostics);
}

/// A floor name in a forwarding list is stripped with a diagnostic, and is an error in
/// strict mode (the bundled plugins' mode).
#[test]
fn a_floor_name_is_stripped_or_refused_in_strict_mode() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gate/strict/forwarding-authorization.toml");
    let src = fs::read_to_string(&path).unwrap();
    let g = validate_with(&src, PluginSource::User(path.clone()), "f.toml", &ctx(false, false)).unwrap();
    let up = &g.entity.forwarding.as_ref().unwrap().to_upstream.headers;
    assert!(up.iter().all(|h| h.name != "authorization"), "{up:?}");
    assert_eq!(g.diagnostics.len(), 1, "{:?}", g.diagnostics);
    assert!(g.diagnostics[0].to_string().contains("entry stripped"), "{:?}", g.diagnostics);
    let errors = validate_with(&src, PluginSource::User(path.clone()), "f.toml", &ctx(true, false)).unwrap_err();
    one_error("forwarding-authorization", "f.toml", expect(&src), &errors);
}

/// US7-4: the shipped styles and the hand-maintained plugins pass in strict mode.
#[test]
fn shipped_styles_and_hand_maintained_plugins_pass_strict() {
    let styles = bundled_style_sources();
    assert_eq!(styles.len(), 4);
    for (file, src) in styles {
        validate_style(src, file).unwrap_or_else(|e| panic!("{file}: {e:#?}"));
    }
    let strict = ctx(true, false);
    for name in HAND_MAINTAINED {
        let (_, src) = bundled_sources().iter().find(|(f, _)| *f == name).unwrap();
        let g = validate_with(src, PluginSource::Bundled, name, &strict).unwrap_or_else(|e| panic!("{name}: {e:#?}"));
        assert!(g.diagnostics.is_empty(), "{name}: {:?}", g.diagnostics);
    }
}
