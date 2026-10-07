use nullrouter_adapter_kit::Path;
use nullrouter_adapters::selector::{covers, extract, Selector};
use serde_json::json;

fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap()
}

#[test]
fn parses_the_documented_forms() {
    for s in ["messages[*].content[*]", "tools", "$", "messages[2].images", "[\"a.b\"].c"] {
        sel(s);
    }
}

#[test]
fn refuses_bad_selectors() {
    for s in ["", "a..b", "a[", "a[x]", "a[1", ".a", "a.", "a.b.c.d.e.f.g.h.i"] {
        assert!(Selector::parse(s).is_err(), "{s:?} should be refused");
    }
    // Exactly 8 segments is fine.
    assert!(Selector::parse("a.b.c.d.e.f.g.h").is_ok());
}

#[test]
fn extracts_matches_with_concrete_paths() {
    let body = json!({"messages": [
        {"role": "user", "content": ["a", "b"]},
        {"role": "assistant", "content": ["c"]}
    ], "tools": [1]});
    let parts = extract(&body, &[sel("messages[*].content[*]")]);
    let got: Vec<(String, serde_json::Value)> = parts.iter().map(|p| (p.path.to_string(), p.value.clone())).collect();
    assert_eq!(
        got,
        vec![
            ("messages[0].content[0]".into(), json!("a")),
            ("messages[0].content[1]".into(), json!("b")),
            ("messages[1].content[0]".into(), json!("c")),
        ]
    );
    let t = extract(&body, &[sel("tools")]);
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].value, json!([1]));
}

#[test]
fn root_selector_yields_the_whole_body() {
    let body = json!({"a": 1});
    let parts = extract(&body, &[sel("$")]);
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].path.to_string(), "$");
    assert_eq!(parts[0].value, body);
}

#[test]
fn no_match_is_empty() {
    assert!(extract(&json!({"a": 1}), &[sel("messages[*]"), sel("b")]).is_empty());
    assert!(extract(&json!({"messages": "x"}), &[sel("messages[*]")]).is_empty());
}

#[test]
fn covers_paths_under_a_selector() {
    let s = [sel("messages[*].content[*]")];
    assert!(covers(&s, &Path::parse("messages[3].content[1]").unwrap()));
    assert!(covers(&s, &Path::parse("messages[3].content[1].text").unwrap()));
    assert!(!covers(&s, &Path::parse("messages[3].role").unwrap()));
    assert!(!covers(&s, &Path::parse("messages[3].content").unwrap()));
    assert!(covers(&[sel("$")], &Path::parse("anything.at[1]").unwrap()));
}
