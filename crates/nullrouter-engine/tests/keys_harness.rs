//! A key can name the harness adapter its requests run through (spec 004, FR-001). The key's
//! `adapter` is separate from its display-only `harness` tag (spec 010 R7).

use std::path::Path;

use nullrouter_engine::keys::{HarnessName, Keys};

fn name(s: &str) -> Result<HarnessName, String> {
    HarnessName::new(s).map_err(|e| e.to_string())
}

#[test]
fn the_adapter_round_trips_through_keys_toml() {
    let dir = tempfile::tempdir().unwrap();
    let mut keys = Keys::load(&dir.path().join("keys.toml")).unwrap();
    keys.issue("laptop", None).unwrap();
    keys.set_adapter("laptop", Some(name("hermes").unwrap())).unwrap();
    keys.save().unwrap();
    let text = std::fs::read_to_string(&keys.path).unwrap();
    assert!(text.contains("adapter = \"hermes\""), "{text}");

    let mut again = Keys::load(&keys.path).unwrap();
    assert_eq!(again.iter().next().unwrap().adapter.as_ref().map(HarnessName::as_str), Some("hermes"));
    again.set_adapter("laptop", None).unwrap();
    assert!(!again.to_toml().contains("adapter"), "a cleared binding is not written");
}

#[test]
fn files_without_an_adapter_still_load() {
    let text = "schema = 1\n[[key]]\nid = \"ak_aaaaaaaa\"\nname = \"old\"\ndigest = \"sha256:00\"\nlast4 = \"abcd\"\ncreated = \"2026-10-01T00:00:00Z\"\n";
    let keys = Keys::parse(text, Path::new("keys.toml")).unwrap();
    assert!(keys.iter().next().unwrap().adapter.is_none());
}

#[test]
fn a_bad_adapter_in_the_file_is_refused_and_a_harness_tag_binds_nothing() {
    let key = "schema = 1\n[[key]]\nid = \"ak_aaaaaaaa\"\nname = \"k\"\ndigest = \"sha256:00\"\nlast4 = \"abcd\"\ncreated = \"x\"\n";
    assert!(Keys::parse(&format!("{key}adapter = \"opencode\"\n"), Path::new("keys.toml")).is_err());
    let tagged = Keys::parse(&format!("{key}harness = \"hermes\"\n"), Path::new("keys.toml")).unwrap();
    assert!(tagged.iter().next().unwrap().adapter.is_none(), "the display tag never selects an adapter");
}

#[test]
fn reserved_and_malformed_names_are_refused() {
    for bad in ["opencode", "grok-build", "zcode", "", "a", "Hermes", "1abc", "has space", "under_score", &"a".repeat(33)] {
        assert!(name(bad).is_err(), "{bad:?} should be refused");
    }
}

#[test]
fn well_formed_names_are_accepted_even_if_unknown() {
    for ok in ["hermes", "claude-code", "ab", "some-future-harness-2", &"a".repeat(32)] {
        assert!(name(ok).is_ok(), "{ok:?} should be accepted");
    }
}
