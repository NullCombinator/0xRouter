//! US1 scenario 4, SC-002: bundled client secrets live in the core, never in plugin files,
//! and never print.

use std::path::PathBuf;

use serde_json::Value;
use zerorouter_registry::{OperatorHome, RegistryHandle, bundled_sources};

/// `(provider id, clientSecret)` for every fixture entry that has one.
fn fixture_secrets() -> Vec<(String, String)> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/9router/providers.json");
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    doc["data"]
        .as_object()
        .unwrap()
        .iter()
        .filter_map(|(id, t)| Some((id.clone(), t.get("clientSecret")?.as_str()?.to_owned())))
        .collect()
}

#[test]
fn no_plugin_file_carries_a_secret() {
    let secrets = fixture_secrets();
    assert_eq!(secrets.len(), 4);
    assert_eq!(bundled_sources().len(), 121);
    for (file, src) in bundled_sources() {
        assert!(!src.contains("client_secret"), "{file} declares client_secret");
        for (id, secret) in &secrets {
            assert!(!src.contains(secret.as_str()), "{file} contains {id}'s client secret");
        }
    }
}

#[test]
fn bundled_secrets_are_released_and_opaque() {
    let home = tempfile::tempdir().unwrap();
    let reg = RegistryHandle::open(OperatorHome::new(home.path())).unwrap().snapshot();
    for (id, secret) in fixture_secrets() {
        let held = reg.composed_transport(&id).unwrap().client_secret.unwrap_or_else(|| panic!("{id}: withheld"));
        assert!(held.matches(&secret), "{id}");
        assert!(!held.matches("not-the-secret"), "{id}");
        assert_eq!(format!("{held:?}"), "***");
        assert_eq!(format!("{held}"), "***");
        let json = serde_json::to_string(&reg.composed_transport(&id).unwrap()).unwrap();
        assert!(!json.contains(&secret) && !json.contains("clientSecret"), "{id}: secret serialised");
    }
}
