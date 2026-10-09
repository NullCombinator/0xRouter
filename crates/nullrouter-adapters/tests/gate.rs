//! The validation gate's corpus (SC-004): one refused case per code in
//! contracts/adapter-package.md, a multi-reason case, and the valid noop package.

use std::fs;
use std::path::{Path, PathBuf};

use nullrouter_adapters::gate::{Reason, check};

const STYLES: [&str; 4] = [
    "anthropic-messages",
    "gemini",
    "openai-chat",
    "openai-responses",
];

/// Every code in the contract's "Gate rules and refusal messages" table.
const CODES: [&str; 21] = [
    "file_not_allowed",
    "binary_file",
    "too_large",
    "manifest_missing",
    "manifest_invalid",
    "harness_invalid",
    "style_unknown",
    "selector_invalid",
    "foreign_dependency",
    "dependency_source",
    "build_script",
    "proc_macro",
    "cargo_table_not_allowed",
    "parse_error",
    "unsafe_code",
    "extern_block",
    "extern_crate",
    "abi_attribute",
    "path_attribute",
    "forbidden_macro",
    "opaque_blob",
];

fn corpus(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/gate")
        .join(relative)
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A valid Rust file over 64 KiB: comments only, so no literal trips another rule.
fn big_source() -> String {
    let mut source = String::from("pub fn filler() {}\n");
    while source.len() < 65 * 1024 {
        source.push_str("// lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do\n");
    }
    source
}

/// Cases git can't hold, or that would bloat the repository, are made here.
fn mutate(code: &str, dir: &Path) {
    match code {
        "file_not_allowed" => {
            std::os::unix::fs::symlink("lib.rs", dir.join("src/link.rs")).unwrap();
        }
        "binary_file" => fs::write(dir.join("src/data.rs"), b"// binary\0\n").unwrap(),
        "too_large" => fs::write(dir.join("src/big.rs"), big_source()).unwrap(),
        _ => {}
    }
}

fn render(reasons: &[Reason]) -> String {
    reasons
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Copies `invalid/<case>` to a temporary directory, applies its mutation, and
/// returns the refusal lines the gate produces for it.
fn refused_as(case: &str) -> String {
    let work = tempfile::tempdir().unwrap();
    let dir = work.path().join(case);
    copy_dir(&corpus(&format!("invalid/{case}")), &dir);
    mutate(case, &dir);
    match check(&dir, &STYLES) {
        Ok(_) => panic!("{case} was accepted"),
        Err(reasons) => render(&reasons),
    }
}

fn expected(case: &str) -> String {
    fs::read_to_string(corpus(&format!("invalid/{case}.expected")))
        .unwrap()
        .trim()
        .to_owned()
}

#[test]
fn the_noop_adapter_passes_the_gate() {
    let gated = match check(&corpus("valid/noop"), &STYLES) {
        Ok(gated) => gated,
        Err(reasons) => panic!("valid/noop refused:\n{}", render(&reasons)),
    };
    assert_eq!(gated.manifest.harness, "noop");
}

#[test]
fn every_refusal_code_has_a_case_and_a_golden_file() {
    for code in CODES.iter().copied().chain(["multi_reason"]) {
        assert!(
            corpus(&format!("invalid/{code}")).is_dir(),
            "missing case directory for {code}"
        );
        assert!(
            corpus(&format!("invalid/{code}.expected")).is_file(),
            "missing golden file for {code}"
        );
    }
}

#[test]
fn a_symlink_is_refused_as_file_not_allowed() {
    assert_eq!(refused_as("file_not_allowed"), expected("file_not_allowed"));
}

#[test]
fn a_nul_byte_file_is_refused_as_binary() {
    assert_eq!(refused_as("binary_file"), expected("binary_file"));
}

#[test]
fn a_rust_file_over_64_kib_is_refused() {
    assert_eq!(refused_as("too_large"), expected("too_large"));
}

#[test]
fn a_package_without_adapter_toml_is_refused() {
    assert_eq!(refused_as("manifest_missing"), expected("manifest_missing"));
}

#[test]
fn an_unknown_adapter_toml_key_is_refused() {
    assert_eq!(refused_as("manifest_invalid"), expected("manifest_invalid"));
}

#[test]
fn a_built_in_harness_name_is_refused() {
    assert_eq!(refused_as("harness_invalid"), expected("harness_invalid"));
}

#[test]
fn an_unloaded_style_is_refused() {
    assert_eq!(refused_as("style_unknown"), expected("style_unknown"));
}

#[test]
fn a_nine_segment_selector_is_refused() {
    assert_eq!(refused_as("selector_invalid"), expected("selector_invalid"));
}

#[test]
fn a_foreign_dependency_is_refused() {
    assert_eq!(refused_as("foreign_dependency"), expected("foreign_dependency"));
}

#[test]
fn a_kit_dependency_with_a_path_is_refused() {
    assert_eq!(refused_as("dependency_source"), expected("dependency_source"));
}

#[test]
fn a_build_key_is_refused() {
    assert_eq!(refused_as("build_script"), expected("build_script"));
}

#[test]
fn a_proc_macro_flag_is_refused() {
    assert_eq!(refused_as("proc_macro"), expected("proc_macro"));
}

#[test]
fn a_profile_table_is_refused() {
    assert_eq!(
        refused_as("cargo_table_not_allowed"),
        expected("cargo_table_not_allowed")
    );
}

#[test]
fn a_file_that_does_not_parse_is_refused() {
    assert_eq!(refused_as("parse_error"), expected("parse_error"));
}

#[test]
fn an_unsafe_fn_is_refused() {
    assert_eq!(refused_as("unsafe_code"), expected("unsafe_code"));
}

#[test]
fn an_extern_block_is_refused() {
    assert_eq!(refused_as("extern_block"), expected("extern_block"));
}

#[test]
fn an_extern_crate_outside_the_allowed_set_is_refused() {
    assert_eq!(refused_as("extern_crate"), expected("extern_crate"));
}

#[test]
fn a_no_mangle_attribute_is_refused() {
    assert_eq!(refused_as("abi_attribute"), expected("abi_attribute"));
}

#[test]
fn a_path_attribute_on_a_module_is_refused() {
    assert_eq!(refused_as("path_attribute"), expected("path_attribute"));
}

#[test]
fn a_forbidden_macro_is_refused() {
    assert_eq!(refused_as("forbidden_macro"), expected("forbidden_macro"));
}

#[test]
fn a_base64_literal_over_256_characters_is_refused() {
    assert_eq!(refused_as("opaque_blob"), expected("opaque_blob"));
}

#[test]
fn several_reasons_are_all_listed_in_one_refusal() {
    assert_eq!(refused_as("multi_reason"), expected("multi_reason"));
}
