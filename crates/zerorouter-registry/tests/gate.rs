//! US4 s1–s4, SC-003: the validation gate over `tests/gate/`.

use std::fs;
use std::path::{Path, PathBuf};

use zerorouter_registry::validate::validate;
use zerorouter_registry::{
    CapabilityKind, OperatorHome, PluginSource, RegistryHandle, bundled_sources, validate_user_plugin,
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
    let mut files: Vec<_> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
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
    let reg = RegistryHandle::open(OperatorHome::new(home.path())).unwrap().snapshot();
    let info = reg.model("bare", "m-a").unwrap();
    assert!(info.declared);
    assert_eq!(info.name, zerorouter_registry::derive_model_name("m-a"));
}

/// FR-005: absent `models` (catalog unknown) differs from `models = []` (offers none).
#[test]
fn catalog_unknown_is_not_catalog_empty() {
    assert!(valid("minimal.toml").models.is_none());
    let (_, zed) = bundled_sources().iter().find(|(f, _)| *f == "zed.toml").unwrap();
    assert!(zed.lines().any(|l| l.trim() == "models = []"), "zed.toml no longer declares an empty catalog");
    let zed = validate(zed, PluginSource::Bundled, "zed.toml").unwrap();
    assert_eq!(zed.models.as_deref(), Some(&[][..]));
}

#[test]
fn every_bundled_plugin_passes_the_gate() {
    assert_eq!(bundled_sources().len(), 121);
    for (file, src) in bundled_sources() {
        validate(src, PluginSource::Bundled, file).unwrap_or_else(|e| panic!("{file}: {e:?}"));
    }
}
