//! The guardrail over openai-responses bodies (T034).

suite!("openai-responses");

/// A choice no template reads (`allowed_tools`) is opaque at the choice path. It belongs to the
/// choice: removing it lets the model call any declared tool, and changing it is a change.
#[test]
fn removing_or_changing_an_allowed_tools_choice_is_a_violation() {
    let (s, mut body, _) = original(ID);
    let allowed = |name: &str| json!({"type": "allowed_tools", "mode": "required", "tools": [{"type": "function", "name": name}]});
    body["tool_choice"] = allowed("get_weather");
    let before = nullrouter_wire::codec::request::decode(&s, &body).unwrap();
    assert!(before.tool_choice.is_none() && before.opaque.iter().any(|o| o.at == "tool_choice"), "{before:?}");
    let mut removed = body.clone();
    removed.as_object_mut().unwrap().remove("tool_choice");
    violates(&guard::check_request(&s, &before, &body, &removed), R::ToolCallChanged);
    let mut changed = body.clone();
    changed["tool_choice"] = allowed("run_shell");
    assert!(matches!(guard::check_request(&s, &before, &body, &changed), Verdict::Violation { .. }));
    assert_eq!(guard::check_request(&s, &before, &body, &body), Verdict::Ok);
}
