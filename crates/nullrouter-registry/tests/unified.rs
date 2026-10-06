//! US3: unified models, per-provider settings, and target classification.

use std::sync::Arc;

use nullrouter_registry::{NotFound, OperatorHome, Registry, RegistryHandle, Resolution, StartupError};
use tempfile::TempDir;

fn open(config: &str) -> (TempDir, Result<Arc<Registry>, StartupError>) {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("config.toml"), config).unwrap();
    let reg = RegistryHandle::open_parity(OperatorHome::new(home.path())).map(|h| h.snapshot());
    (home, reg)
}

fn ok(config: &str) -> Arc<Registry> {
    open(config).1.unwrap_or_else(|e| panic!("{e}"))
}

/// Every rendered error, one per line.
fn errors(config: &str) -> String {
    match open(config).1 {
        Ok(_) => panic!("config was accepted:\n{config}"),
        Err(e) => e.to_string(),
    }
}

const SONNET: &str = r#"
schema = 1

[[unified_model]]
name = "sonnet"
members = [
  { provider = "kr", model = "claude-sonnet-4-5" },
  { provider = "openrouter", model = "anthropic/claude-sonnet-4.5" },
]
"#;

#[test]
fn s1_members_resolve_in_declaration_order() {
    let reg = ok(SONNET);
    let Ok(Resolution::Unified(u)) = reg.resolve("sonnet") else { panic!("not unified") };
    let got: Vec<_> =
        u.members.iter().map(|m| (m.provider.as_str(), m.requested.as_str(), m.upstream_id.as_str())).collect();
    assert_eq!(
        got,
        [
            ("kiro", "claude-sonnet-4-5", "claude-sonnet-4.5"),
            ("openrouter", "anthropic/claude-sonnet-4.5", "anthropic/claude-sonnet-4.5"),
        ]
    );
    // openrouter is passthrough: its member is accepted but not in the catalog.
    assert_eq!(u.members.iter().map(|m| m.catalogued).collect::<Vec<_>>(), [true, false]);
    assert_eq!(reg.unified_models().count(), 1);
    assert_eq!(reg.report().unified_models, 1);
}

#[test]
fn s2_unknown_provider_or_uncatalogued_model_is_named() {
    let e = errors(
        r#"
[[unified_model]]
name = "x"
members = [{ provider = "kr", model = "claude-sonnet-4-5" }, { provider = "xx", model = "m" }]
"#,
    );
    assert!(e.contains(r#"unified_model[0].members[1].provider: unknown provider "xx""#), "{e}");

    let e = errors(
        r#"
[[unified_model]]
name = "x"
members = [{ provider = "claude", model = "foo" }]
"#,
    );
    assert!(e.contains(r#"unified_model[0].members[0].model: "foo" is not declared by provider "claude""#), "{e}");

    // A passthrough provider accepts any model.
    let reg = ok(r#"
[[unified_model]]
name = "x"
members = [{ provider = "openrouter", model = "brand/new-model" }]
"#);
    assert_eq!(reg.unified_model("x").unwrap().members[0].upstream_id, "brand/new-model");
}

#[test]
fn s3_kind_conflicts() {
    let e = errors(
        r#"
[[unified_model]]
name = "x"
kind = "llm"
members = [{ provider = "openai", model = "text-embedding-3-large" }]
"#,
    );
    assert!(
        e.contains(r#"unified_model[0].members[0]: kind "embedding" conflicts with unified_model kind "llm""#),
        "{e}"
    );

    let e = errors(
        r#"
[[unified_model]]
name = "x"
members = [
  { provider = "openai", model = "text-embedding-3-large" },
  { provider = "ag", model = "gemini-3.1-flash-image" },
]
"#,
    );
    assert!(
        e.contains(r#"unified_model[0].members[1]: kind "image" conflicts with members[0] kind "embedding""#),
        "{e}"
    );

    // An untyped member (kiro's models carry no kind) never conflicts.
    let reg = ok(r#"
[[unified_model]]
name = "x"
kind = "embedding"
members = [
  { provider = "openai", model = "text-embedding-3-large" },
  { provider = "kr", model = "claude-sonnet-4.5" },
]
"#);
    assert_eq!(reg.unified_model("x").unwrap().members.len(), 2);
}

#[test]
fn s4_bare_model_name_is_never_a_provider_model() {
    let reg = ok(SONNET);
    assert_eq!(
        reg.resolve("claude-sonnet-4.5").unwrap_err(),
        NotFound::UnifiedModel { name: "claude-sonnet-4.5".into() }
    );
}

#[test]
fn s5_direct_target() {
    let reg = ok(SONNET);
    let Ok(Resolution::Direct { provider, requested, upstream_id, catalogued }) = reg.resolve("kr/claude-sonnet-4-5")
    else {
        panic!("not direct")
    };
    assert_eq!(
        (provider.id.as_str(), requested, upstream_id.as_str(), catalogued),
        ("kiro", "claude-sonnet-4-5", "claude-sonnet-4.5", true)
    );
}

#[test]
fn s6_uncatalogued_model_passes_through_unchanged() {
    let reg = ok("");
    let Ok(Resolution::Direct { provider, requested, upstream_id, catalogued }) =
        reg.resolve("openai/brand-new-model(high)")
    else {
        panic!("not direct")
    };
    assert_eq!(
        (provider.id.as_str(), requested, upstream_id.as_str(), catalogued),
        ("openai", "brand-new-model(high)", "brand-new-model(high)", false)
    );
}

#[test]
fn s7_uncatalogued_models_can_be_disabled() {
    let reg = ok("[provider.openai]\nallow_uncatalogued_models = false\n");
    assert!(!reg.settings("openai").allow_uncatalogued_models);
    assert!(reg.settings("anthropic").allow_uncatalogued_models);
    assert_eq!(
        reg.resolve("openai/brand-new-model").unwrap_err(),
        NotFound::Model { provider: "openai".into(), model: "brand-new-model".into() }
    );
    assert!(matches!(reg.resolve("openai/text-embedding-3-large"), Ok(Resolution::Direct { catalogued: true, .. })));
    // A live list (spec 005 T100) the caller supplies names more; static results don't move.
    let live = |p: &str, m: &str| p == "openai" && m == "brand-new-model";
    let Ok(Resolution::Direct { upstream_id, catalogued, .. }) = reg.resolve_with("openai/brand-new-model", live)
    else {
        panic!("a live id resolves")
    };
    assert_eq!((upstream_id.as_str(), catalogued), ("brand-new-model", false));
    assert!(reg.resolve_with("openai/other-model", live).is_err());
    assert_eq!(reg.resolve_with("openai/text-embedding-3-large", live), reg.resolve("openai/text-embedding-3-large"));

    let reg = ok("[provider.openrouter]\nallow_uncatalogued_models = false\n");
    // Passthrough is not subject to the setting.
    assert!(matches!(reg.resolve("openrouter/brand/new"), Ok(Resolution::Direct { catalogued: false, .. })));
}

#[test]
fn invalid_names_and_members() {
    let e =
        errors("[[unified_model]]\nname = \"a/b\"\nmembers = [{ provider = \"kr\", model = \"claude-sonnet-4.5\" }]\n");
    assert!(
        e.contains(r#"config.toml:2"#)
            && e.contains(r#"unified_model[0].name: must be non-empty and must not contain "/""#),
        "{e}"
    );

    let e = errors(
        r#"
[[unified_model]]
name = "a"
members = [{ provider = "kr", model = "claude-sonnet-4.5" }]
[[unified_model]]
name = "a"
members = [{ provider = "kr", model = "claude-sonnet-4.5" }]
"#,
    );
    assert!(e.contains("unified_model[1].name: duplicate of unified_model[0]"), "{e}");

    let e = errors(
        r#"
[[unified_model]]
name = "a"
members = [{ provider = "kr", model = "claude-sonnet-4.5" }, { provider = "kiro", model = "claude-sonnet-4-5" }]
"#,
    );
    assert!(e.contains(r#"unified_model[0].members[1]: provider "kiro" already a member"#), "{e}");

    let e = errors("[[unified_model]]\nname = \"a\"\nmembers = []\n");
    assert!(e.contains("unified_model[0].members: must not be empty"), "{e}");

    let e = errors("[provider.nope]\nallow_uncatalogued_models = false\n");
    assert!(e.contains("provider.nope: unknown provider"), "{e}");

    let e = errors("[plugin_decisions]\nfoo = \"replace\"\n");
    assert!(e.contains("plugin_decisions.foo: "), "{e}");

    let e = errors("surprise = 1\n");
    assert!(e.contains("surprise"), "{e}");
}

#[test]
fn every_error_is_reported() {
    let e = errors(
        r#"
[[unified_model]]
name = ""
members = []
[provider.nope]
"#,
    );
    assert_eq!(e.lines().count(), 3, "{e}");
}

#[test]
fn target_shapes() {
    let reg = ok("");
    let Ok(Resolution::Direct { provider, requested, .. }) = reg.resolve("openrouter/meta-llama/llama-3") else {
        panic!("not direct")
    };
    assert_eq!((provider.id.as_str(), requested), ("openrouter", "meta-llama/llama-3"));
    for t in ["", "/gpt-4o", "openai/"] {
        assert_eq!(reg.resolve(t).unwrap_err(), NotFound::EmptyTarget, "{t:?}");
    }
    assert_eq!(reg.resolve("nope/x").unwrap_err(), NotFound::Provider { token: "nope".into() });
}

#[test]
fn missing_config_means_no_unified_models() {
    let home = tempfile::tempdir().unwrap();
    let reg = RegistryHandle::open_parity(OperatorHome::new(home.path())).unwrap().snapshot();
    assert_eq!(reg.unified_models().count(), 0);
    assert!(reg.settings("openai").allow_uncatalogued_models);
}

/// Slice 006: `[routing.amortization_for]` keys must name a unified model or a known
/// `provider/model`; a direct target is stored under the provider's id.
#[test]
fn amortization_for_names_known_targets() {
    let config = format!(
        "{SONNET}\n[routing]\namortization = \"2h\"\n[routing.amortization_for]\nsonnet = \"1h\"\n\"kr/claude-sonnet-4-5\" = \"24h\"\n"
    );
    let r = ok(&config).runtime().routing.clone();
    assert_eq!(r.amortization.as_secs(), 7200);
    assert_eq!(r.amortization_for["sonnet"].as_secs(), 3600);
    assert_eq!(r.amortization_for["kiro/claude-sonnet-4-5"].as_secs(), 86_400);

    let bad = format!("{SONNET}\n[routing.amortization_for]\nnope = \"1h\"\n");
    let e = errors(&bad);
    assert!(e.contains("routing.amortization_for.nope") && e.contains("names no unified model"), "{e}");
    let bad = format!("{SONNET}\n[routing.amortization_for]\n\"kr/no-such-model\" = \"1h\"\n");
    assert!(errors(&bad).contains("is not declared by provider"), "{}", errors(&bad));
}

// US8: a unified model whose members' limits differ loads, and the report notes it (R14).

const MIXED: &str = r#"
schema = 1

[[unified_model]]
name = "mixed"
members = [
  { provider = "grok-cli", model = "grok-build" },
  { provider = "bluesminds", model = "claude-sonnet-4-5" },
  { provider = "kr", model = "claude-sonnet-4.5" },
]
"#;

#[test]
fn n1_members_with_different_limits_produce_a_note_and_still_load() {
    let reg = ok(MIXED);
    assert_eq!(reg.unified_model("mixed").unwrap().members.len(), 3);
    let notes: Vec<String> = reg.report().notes.iter().map(ToString::to_string).collect();
    assert_eq!(
        notes,
        [
            "unified model mixed: members differ in context_length: grok-cli 500000, bluesminds 200000, kiro undeclared",
            "unified model mixed: members differ in max_output_tokens: grok-cli 64000, bluesminds undeclared, kiro undeclared",
        ]
    );
    let n = &reg.report().notes[0];
    assert_eq!((n.unified.as_str(), n.limit.as_str()), ("mixed", "context_length"));
    assert_eq!(n.values[2], ("kiro".to_owned(), None));
}

#[test]
fn n2_equal_or_wholly_undeclared_limits_are_not_noted() {
    // One member: nothing to differ from.
    let reg = ok(r#"
[[unified_model]]
name = "solo"
members = [{ provider = "grok-cli", model = "grok-build" }]
"#);
    assert!(reg.report().notes.is_empty());
    // No member declares a limit.
    let reg = ok(r#"
[[unified_model]]
name = "blank"
members = [{ provider = "kr", model = "claude-sonnet-4.5" }, { provider = "openrouter", model = "a/b" }]
"#);
    assert!(reg.report().notes.is_empty(), "{:?}", reg.report().notes);
}

#[test]
fn n3_a_reload_recomputes_the_notes() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("config.toml"), MIXED).unwrap();
    let handle = RegistryHandle::open_parity(OperatorHome::new(home.path())).unwrap();
    assert_eq!(handle.snapshot().report().notes.len(), 2);
    std::fs::write(
        home.path().join("config.toml"),
        "[[unified_model]]\nname = \"mixed\"\nmembers = [{ provider = \"grok-cli\", model = \"grok-build\" }]\n",
    )
    .unwrap();
    let report = handle.reload().unwrap();
    assert!(report.notes.is_empty());
    assert!(handle.snapshot().report().notes.is_empty());
}
