//! The hostile corpus is well formed (spec 004, T065): every case has a module that assembles,
//! a manifest that parses, an expectation the behaviour suite (T066) can read, and a README line.
//! What each case does when it runs is T066's business.

use std::fs;
use std::path::{Path, PathBuf};

use nullrouter_adapters::loader::Manifest;

const OUTCOMES: [&str; 4] = ["not_run", "failed", "blocked", "ran"];
const SIDES: [&str; 3] = ["request", "response", "event"];

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/hostile")
}

fn cases() -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(corpus())
        .expect("the corpus directory")
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

fn read(case: &str, file: &str) -> String {
    let path = corpus().join(case).join(file);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn the_corpus_has_all_seventeen_cases() {
    let want = [
        "add_tool_call_event",
        "add_tool_call_request",
        "add_tool_call_response",
        "add_tool_def",
        "add_unplaced_field",
        "bad_output",
        "change_tool_args",
        "env",
        "file",
        "legit_removal",
        "loop",
        "memory_bomb",
        "net",
        "out_of_selector",
        "rewrite_tool_result",
        "secret_probe",
        "trap",
    ];
    assert_eq!(cases(), want);
}

#[test]
fn every_module_assembles() {
    for case in cases() {
        let text = read(&case, "module.wat");
        let wasm = wat::parse_str(&text).unwrap_or_else(|e| panic!("{case}: module.wat does not assemble: {e}"));
        assert!(wasm.starts_with(b"\0asm"), "{case}: not a wasm module");
    }
}

#[test]
fn every_manifest_parses_and_names_its_case() {
    for case in cases() {
        let manifest = Manifest::parse(&read(&case, "adapter.toml")).unwrap_or_else(|e| panic!("{case}: {e}"));
        assert_eq!(manifest.harness, format!("hostile-{}", case.replace('_', "-")), "{case}");
        assert_eq!(manifest.style, "openai-chat", "{case}");
        assert_eq!(manifest.kit, "1", "{case}");
    }
}

#[test]
fn every_expectation_parses() {
    for case in cases() {
        let expect: toml::Table = read(&case, "expect.toml").parse().unwrap_or_else(|e| panic!("{case}: {e}"));
        let text =
            |key: &str| expect.get(key).and_then(toml::Value::as_str).unwrap_or_else(|| panic!("{case}: no {key}"));
        assert!(OUTCOMES.contains(&text("outcome")), "{case}: outcome");
        assert!(SIDES.contains(&text("side")), "{case}: side");
        assert!(expect.get("suspect").is_some_and(toml::Value::is_bool), "{case}: suspect");
        if text("outcome") != "ran" {
            assert!(!text("detail").is_empty(), "{case}: detail");
        }
        let input = text("input");
        serde_json::from_str::<serde_json::Value>(&read(&case, input))
            .unwrap_or_else(|e| panic!("{case}: {input} is not JSON: {e}"));
    }
}

#[test]
fn the_secret_probe_names_its_sentinel() {
    let expect: toml::Table = read("secret_probe", "expect.toml").parse().unwrap();
    assert_eq!(expect.get("sentinel").and_then(toml::Value::as_str), Some("NR-SENTINEL-"));
}

#[test]
fn the_readme_names_every_case() {
    let readme = read_readme();
    for case in cases() {
        assert!(readme.contains(&format!("`{case}`")), "README.md does not list {case}");
    }
}

fn read_readme() -> String {
    fs::read_to_string(corpus().join("README.md")).expect("README.md")
}
