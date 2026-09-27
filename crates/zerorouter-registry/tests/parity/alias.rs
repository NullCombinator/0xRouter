use zerorouter_registry::NotFound;

use crate::{bundled, fixture};

/// Tokens 9router resolves but 0router does not (research R10): `qw`, `dv`, `devin`, and
/// `devin-cli` belong to providers outside the bundled set.
const DEVIATIONS: &[&str] = &["qw", "dv", "devin", "devin-cli"];

#[test]
fn alias_tokens_match_9router() {
    let reg = bundled();
    let data = fixture("alias");
    let mut owned = 0;
    let mut diffs = Vec::new();
    for (token, id) in data["aliasToId"].as_object().unwrap() {
        if DEVIATIONS.contains(&token.as_str()) {
            continue;
        }
        owned += 1;
        let got = reg.alias_view(token);
        if got != id.as_str() {
            diffs.push(format!("{token}: got {got:?}, want {id}"));
        }
    }
    assert!(diffs.is_empty(), "alias differences:\n{}", diffs.join("\n"));
    assert_eq!(owned, 113);
}

#[test]
fn unowned_tokens_are_not_found() {
    let reg = bundled();
    for token in DEVIATIONS {
        assert_eq!(reg.provider(token).unwrap_err(), NotFound::Provider { token: (*token).to_owned() }, "{token}");
    }
}

#[test]
fn id_to_alias_matches_9router() {
    let reg = bundled();
    let mut want: std::collections::BTreeMap<String, String> =
        serde_json::from_value(fixture("alias")["idToAlias"].clone()).unwrap();
    // Deviation: 9router gives mimo-free the alias `mmf`, which is also the id of another
    // provider (and resolves to it). The generator drops the alias; mimo-free keeps
    // `ui_alias = "mmf"` for display.
    assert_eq!(want.insert("mimo-free".into(), "mimo-free".into()).as_deref(), Some("mmf"));
    assert_eq!(bundled().provider("mimo-free").unwrap().ui_alias.as_deref(), Some("mmf"));
    assert_eq!(bundled().alias_view("mmf"), Some("mmf"));
    let got: std::collections::BTreeMap<String, String> =
        reg.id_to_alias().into_iter().map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
    assert_eq!(got.len(), 83);
    assert_eq!(got, want);
}

#[test]
fn kr_is_kiro() {
    assert_eq!(bundled().provider("kr").unwrap().id, "kiro");
}
