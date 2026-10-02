//! Estimator parity with 9router (T112, research R15): every `tests/fixtures/9router/count`
//! case estimated from its Messages body, and a request in another style estimated from
//! its translation into the Messages shape.

mod oracle;

use nullrouter_wire::codec::request;
use nullrouter_wire::estimate;
use oracle::{bundled, root};
use serde_json::{Value, json};

#[test]
fn estimates_match_the_9router_oracle() {
    let path = root().join("tests/fixtures/9router/count/cases.json");
    let file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let cases = file["data"]["cases"].as_array().unwrap();
    assert!(cases.len() >= 6, "the count oracle is short (run tools/gen-bundled/generate.mjs)");
    let failures: Vec<String> = cases
        .iter()
        .filter_map(|c| {
            let got = estimate::messages_body(&c["input"]);
            (json!(got) != c["input_tokens"]).then(|| format!("{}: got {got}, want {}", c["name"], c["input_tokens"]))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn another_style_is_estimated_in_the_messages_shape() {
    let messages = bundled("anthropic-messages");
    let chat = json!({"model": "m", "messages": [
        {"role": "system", "content": "You are helpful."},
        {"role": "user", "content": "What is 2 + 2?"},
        {"role": "assistant", "content": "4"},
        {"role": "user", "content": "And times 3?"},
    ]});
    let req = request::decode(&bundled("openai-chat"), &chat).unwrap();
    let encoded = request::encode(&req, &messages, "openai-chat").unwrap().body;
    let n = estimate::estimate(&req, &messages, "openai-chat").unwrap();
    assert_eq!(n, estimate::messages_body(&encoded));
    // The same conversation written as Messages (the oracle's multi-turn case) is 11.
    assert!((11..=20).contains(&n), "{n}: {encoded:#}");
}
