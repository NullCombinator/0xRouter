//! Spec 011 T032, US5: combos in `config.toml`. Each load error with its position, a combo
//! dropped with the unified model it needs, no combos in plugins, and the walk a request takes.

use std::path::Path;
use std::sync::Arc;

use nullrouter_registry::{OperatorHome, Registry, RegistryHandle, Resolution, StartupError, validate_user_plugin};
use tempfile::TempDir;

/// Unified models every test can name: `sonnet` and `gpt` untyped, `embed` an embedding model,
/// `img` an image model.
const UNIFIED: &str = r#"
[[unified_model]]
name = "sonnet"
members = [{ provider = "kr", model = "claude-sonnet-4-5" }]

[[unified_model]]
name = "gpt"
members = [{ provider = "openrouter", model = "openai/gpt-5" }]

[[unified_model]]
name = "embed"
members = [{ provider = "openai", model = "text-embedding-3-large" }]

[[unified_model]]
name = "img"
members = [{ provider = "ag", model = "gemini-3.1-flash-image" }]
"#;

fn open(config: &str) -> (TempDir, Result<Arc<Registry>, StartupError>) {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("config.toml"), format!("{UNIFIED}{config}")).unwrap();
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

/// The line `needle` starts on in the whole `config.toml`.
fn line_of(config: &str, needle: &str) -> usize {
    let whole = format!("{UNIFIED}{config}");
    whole.lines().position(|l| l.contains(needle)).unwrap() + 1
}

#[test]
fn a_name_may_not_clash_with_a_unified_model_or_another_combo() {
    let e = errors("[[combo]]\nname = \"sonnet\"\nmembers = [\"gpt\"]\n");
    // The combo's `name` key: the line after `[[combo]]`.
    let at = format!("config.toml:{}:", UNIFIED.lines().count() + 2);
    assert!(e.contains(&at), "{at}: {e}");
    assert!(e.contains(r#"combo[0].name: name "sonnet" is already a unified model"#), "{e}");

    let e = errors("[[combo]]\nname = \"c\"\nmembers = [\"gpt\"]\n[[combo]]\nname = \"c\"\nmembers = [\"sonnet\"]\n");
    assert!(e.contains(r#"combo[1].name: name "c" is already a combo (combo[0])"#), "{e}");

    for name in ["", "a/b"] {
        let e = errors(&format!("[[combo]]\nname = \"{name}\"\nmembers = [\"gpt\"]\n"));
        assert!(e.contains(r#"combo[0].name: must not be empty or contain "/""#), "{e}");
    }
}

#[test]
fn members_must_be_known_and_not_empty() {
    let e = errors("[[combo]]\nname = \"c\"\nmembers = []\n");
    assert!(e.contains("combo[0].members: must not be empty"), "{e}");

    let config = "[[combo]]\nname = \"c\"\nmembers = [\"gpt\", \"nope\"]\n";
    let e = errors(config);
    assert!(e.contains(&format!("config.toml:{}:", line_of(config, "\"nope\""))), "{e}");
    assert!(e.contains(r#"combo[0].members[1]: unknown unified model or combo "nope""#), "{e}");
}

#[test]
fn a_combo_may_not_contain_itself() {
    let e = errors(
        "[[combo]]\nname = \"coder\"\nmembers = [\"sonnet\", \"fallback-chain\"]\n\
         [[combo]]\nname = \"fallback-chain\"\nmembers = [\"gpt\", \"coder\"]\n",
    );
    assert!(e.contains("combo[0]: contains itself: coder → fallback-chain → coder"), "{e}");
    assert_eq!(e.matches("contains itself").count(), 1, "one cycle, one error: {e}");

    let e = errors("[[combo]]\nname = \"me\"\nmembers = [\"me\"]\n");
    assert!(e.contains("combo[0]: contains itself: me → me"), "{e}");
}

#[test]
fn members_must_agree_on_kind() {
    let e = errors("[[combo]]\nname = \"c\"\nmembers = [\"embed\", \"img\"]\n");
    assert!(e.contains("combo[0].members: members disagree on kind: embed is embedding, img is image"), "{e}");

    // Through a nested combo too.
    let e = errors(
        "[[combo]]\nname = \"a\"\nmembers = [\"embed\", \"b\"]\n[[combo]]\nname = \"b\"\nmembers = [\"img\"]\n",
    );
    assert!(e.contains("combo[0].members: members disagree on kind: embed is embedding, b is image"), "{e}");

    // An untyped member never conflicts; the combo takes its typed members' kind.
    let reg = ok("[[combo]]\nname = \"c\"\nmembers = [\"sonnet\", \"embed\"]\n");
    assert_eq!(reg.combo("c").and_then(|c| c.kind).map(|k| k.as_str()), Some("embedding"));
    let reg = ok("[[combo]]\nname = \"c\"\nmembers = [\"sonnet\", \"gpt\"]\n");
    assert_eq!(reg.combo("c").unwrap().kind, None);
}

#[test]
fn resolve_names_a_combo_and_its_walk_is_depth_first_each_unified_model_once() {
    let reg = ok("[[combo]]\nname = \"coder\"\nmembers = [\"sonnet\", \"fallback-chain\", \"gpt\"]\n\
         [[combo]]\nname = \"fallback-chain\"\nmembers = [\"gpt\", \"sonnet\", \"embed\"]\n");
    let Ok(Resolution::Combo(c)) = reg.resolve("coder") else { panic!("not a combo") };
    assert_eq!(c.members, ["sonnet", "fallback-chain", "gpt"]);
    let walk: Vec<(&str, &str)> = reg.combo_walk(c).map(|(path, u)| (path, u.name.as_str())).collect();
    assert_eq!(
        walk,
        [
            ("coder › sonnet", "sonnet"),
            ("coder › fallback-chain › gpt", "gpt"),
            ("coder › fallback-chain › embed", "embed"),
        ]
    );
    assert!(matches!(reg.resolve("sonnet"), Ok(Resolution::Unified(_))));
    assert_eq!(reg.combos().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["coder", "fallback-chain"]);
}

#[test]
fn a_combo_needing_a_dropped_unified_model_is_dropped_at_startup() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir(home.path().join("plugins")).unwrap();
    std::fs::write(home.path().join("plugins/broken.toml"), "id = \"broken\"\ncategory = \"local\"\n").unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        r#"
[[unified_model]]
name = "lost"
members = [{ provider = "broken", model = "m" }]

[[unified_model]]
name = "fine"
members = [{ provider = "kr", model = "claude-sonnet-4.5" }]

[[combo]]
name = "outer"
members = ["fine", "inner"]

[[combo]]
name = "inner"
members = ["lost"]

[[combo]]
name = "kept"
members = ["fine"]
"#,
    )
    .unwrap();
    let reg = RegistryHandle::open_parity(OperatorHome::new(home.path())).unwrap().snapshot();
    let dropped: Vec<String> = reg.report().dropped_combos.iter().map(ToString::to_string).collect();
    assert_eq!(
        dropped,
        [
            "dropped combo outer: needs unified model lost (dropped)",
            "dropped combo inner: needs unified model lost (dropped)",
        ]
    );
    assert!(reg.combo("outer").is_none() && reg.combo("inner").is_none());
    assert!(matches!(reg.resolve("kept"), Ok(Resolution::Combo(_))));
}

#[test]
fn a_plugin_cannot_declare_a_combo() {
    let src = "schema = 2\nid = \"x\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n\
               [[combo]]\nname = \"c\"\nmembers = [\"m\"]\n";
    let e = validate_user_plugin(src, Path::new("x.toml")).unwrap_err();
    let e: Vec<String> = e.iter().map(ToString::to_string).collect();
    assert!(e.iter().any(|l| l.contains("unknown field `combo`")), "{e:?}");
}
