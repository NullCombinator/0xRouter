use serde_json::Value;

use crate::deviations::Deviations;
use crate::{bundled, fixture, not_carried};

#[test]
fn composed_transports_match_9router() {
    let reg = bundled();
    let expected = fixture("providers");
    let expected = expected.as_object().unwrap();

    assert_eq!(reg.providers().count(), 121);
    let with_transport: Vec<_> = reg.providers().filter(|p| p.has_text_transport()).map(|p| p.id.as_str()).collect();
    assert_eq!(with_transport.len(), 83);
    assert_eq!(reg.providers().filter(|p| !p.has_text_transport()).count(), 38);

    let deviations = Deviations::load();
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
        let differs = deviations.compare(id, "providers", &got, &want);
        if !differs.is_empty() {
            diffs.extend(differs);
        } else {
            let (g, w) = (header_order(&got), header_order(&want));
            let (g, w): (Vec<_>, Vec<_>) =
                (g.iter().filter(|h| w.contains(h)).collect(), w.iter().filter(|h| g.contains(h)).collect());
            if g != w {
                diffs.push(format!("{id}: header order {g:?} != {w:?}"));
            }
        }
        match (secret, composed.client_secret) {
            (Some(Value::String(s)), Some(held)) => assert!(held.matches(&s), "{id}: client secret differs"),
            (None, None) => {}
            (s, held) => diffs.push(format!("{id}: client secret presence {} != {}", held.is_some(), s.is_some())),
        }
    }
    assert!(diffs.is_empty(), "{} transport differences:\n{}", diffs.len(), diffs.join("\n"));
}

/// `Value` map equality ignores order; header order is observable upstream. Compared over
/// the headers both send (a listed deviation drops some).
fn header_order(v: &Value) -> Vec<&str> {
    v["headers"].as_object().map(|h| h.keys().map(String::as_str).collect()).unwrap_or_default()
}

#[test]
fn tts_tables_match_9router() {
    let reg = bundled();
    for (table, entry) in fixture("tts-tables").as_object().unwrap() {
        let provider = entry["provider"].as_str().unwrap();
        let entity = reg.provider(provider).unwrap();
        let tts = entity.capabilities.get(&zerorouter_registry::CapabilityKind::Tts);
        if not_carried(provider, "tts") {
            assert!(tts.is_none(), "{table}: {provider} carries tts now; drop it from NOT_CARRIED");
            continue;
        }
        let tts = tts.unwrap_or_else(|| panic!("{table}: {provider} has no tts section"));
        let got: Vec<&str> = match entry["kind"].as_str().unwrap() {
            // Schema 2 lists a type's models at the top level, by `kind`.
            "models" if !entity.endpoints.is_empty() => entity
                .models
                .iter()
                .flatten()
                .filter(|m| m.kind.is_some_and(|k| k.as_str() == "tts"))
                .map(|m| m.id.as_str())
                .collect(),
            "models" => tts.models.iter().flatten().map(|m| m.id.as_str()).collect(),
            "voices" if !entity.endpoints.is_empty() => entity.endpoints[&zerorouter_registry::schema::ModelType::Tts]
                .0
                .iter()
                .flat_map(|e| &e.voices)
                .map(String::as_str)
                .collect(),
            "voices" => tts.voices.iter().flatten().map(|v| v.id.as_str()).collect(),
            k => panic!("{table}: unknown kind {k}"),
        };
        let want: Vec<&str> = entry["models"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(got, want, "{table}");
    }
}
