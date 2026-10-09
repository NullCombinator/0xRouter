//! `combos [NAME]` and `resolve NAME` for a combo through the binary (spec 011 T040,
//! contracts/cli.md § `nullrouter combos`).

use std::path::Path;
use std::process::{Command, Output};

fn nr(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nullrouter")).arg("--home").arg(home).args(args).output().unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).unwrap()
}

/// A home whose `config.toml` is `config` after three unified models on openrouter.
fn home(config: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let unified: String = ["sonnet", "gpt", "glm"]
        .iter()
        .map(|n| {
            let member = format!("{{ provider = \"openrouter\", model = \"x/{n}\" }}");
            format!("[[unified_model]]\nname = \"{n}\"\nmembers = [{member}]\n")
        })
        .collect();
    std::fs::write(dir.path().join("config.toml"), format!("{unified}{config}")).unwrap();
    dir
}

const CODER: &str = "[[combo]]\nname = \"coder\"\nmembers = [\"sonnet\", \"fallback-chain\"]\n\
                     [[combo]]\nname = \"fallback-chain\"\nmembers = [\"gpt\", \"glm\"]\n";

const TREE: &str = "\
combo coder:
  0. sonnet          unified
  1. fallback-chain  combo
     0. gpt          unified
     1. glm          unified
";

#[test]
fn one_combo_prints_its_tree_and_resolve_prints_the_same() {
    let dir = home(CODER);
    let o = nr(dir.path(), &["combos", "coder"]);
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(out(&o), TREE);
    assert_eq!(out(&nr(dir.path(), &["resolve", "coder"])), TREE);

    let json = nr(dir.path(), &["--json", "combos", "coder"]);
    assert_eq!(json.stdout, nr(dir.path(), &["--json", "resolve", "coder"]).stdout);
    let j: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(j["kind"], "combo");
    assert_eq!(j["members"][1]["members"][0]["name"], "gpt");
}

#[test]
fn the_list_shows_every_combo() {
    let dir = home(CODER);
    let text = out(&nr(dir.path(), &["combos"]));
    assert!(text.starts_with(TREE), "{text}");
    assert!(text.contains("combo fallback-chain:\n  0. gpt  unified\n  1. glm  unified\n"), "{text}");
    let j: serde_json::Value = serde_json::from_slice(&nr(dir.path(), &["--json", "combos"]).stdout).unwrap();
    assert_eq!(j["combos"].as_array().unwrap().len(), 2);
    assert_eq!(j["dropped"], serde_json::json!([]));
}

#[test]
fn none_declared_and_not_found() {
    let dir = home("");
    assert_eq!(out(&nr(dir.path(), &["combos"])), "no combos; declare one with [[combo]] in config.toml\n");
    let o = nr(dir.path(), &["combos", "nope"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("not found: no combo named \"nope\""));
}
