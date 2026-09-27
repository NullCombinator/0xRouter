use serde_json::Value;

use crate::{bundled, fixture};

#[test]
fn composed_transports_match_9router() {
    let reg = bundled();
    let expected = fixture("providers");
    let expected = expected.as_object().unwrap();

    assert_eq!(reg.providers().count(), 121);
    let with_transport: Vec<_> = reg.providers().filter(|p| p.transport.is_some()).map(|p| p.id.as_str()).collect();
    assert_eq!(with_transport.len(), 83);
    assert_eq!(reg.providers().filter(|p| p.transport.is_none()).count(), 38);

    let mut diffs = Vec::new();
    for id in with_transport {
        let Some(want) = expected.get(id) else {
            diffs.push(format!("{id}: not in providers.json"));
            continue;
        };
        let mut want = want.clone();
        let secret = want.as_object_mut().unwrap().remove("clientSecret");
        let composed = reg.composed_transport(id).unwrap();
        let got = serde_json::to_value(&composed).unwrap();
        if got != want {
            diffs.push(format!("{id}:\n  got  {got}\n  want {want}"));
        } else if header_order(&got) != header_order(&want) {
            diffs.push(format!("{id}: header order {:?} != {:?}", header_order(&got), header_order(&want)));
        }
        match (secret, composed.client_secret) {
            (Some(Value::String(s)), Some(held)) => assert!(held.matches(&s), "{id}: client secret differs"),
            (None, None) => {}
            (s, held) => diffs.push(format!("{id}: client secret presence {} != {}", held.is_some(), s.is_some())),
        }
    }
    assert!(diffs.is_empty(), "{} transport differences:\n{}", diffs.len(), diffs.join("\n"));
}

/// `Value` map equality ignores order; header order is observable upstream.
fn header_order(v: &Value) -> Vec<&str> {
    v["headers"].as_object().map(|h| h.keys().map(String::as_str).collect()).unwrap_or_default()
}

#[test]
fn tts_tables_match_9router() {
    let reg = bundled();
    for (table, entry) in fixture("tts-tables").as_object().unwrap() {
        let provider = entry["provider"].as_str().unwrap();
        let tts = reg.provider(provider).unwrap().capabilities.get(&zerorouter_registry::CapabilityKind::Tts);
        let tts = tts.unwrap_or_else(|| panic!("{table}: {provider} has no tts section"));
        let got: Vec<&str> = match entry["kind"].as_str().unwrap() {
            "models" => tts.models.iter().flatten().map(|m| m.id.as_str()).collect(),
            "voices" => tts.voices.iter().flatten().map(|v| v.id.as_str()).collect(),
            k => panic!("{table}: unknown kind {k}"),
        };
        let want: Vec<&str> = entry["models"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(got, want, "{table}");
    }
}
