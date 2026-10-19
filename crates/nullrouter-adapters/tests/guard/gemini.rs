//! The guardrail over gemini bodies (T034).

suite!("gemini");

use serde_json::Value;

/// The verdict on a body whose `functionCallingConfig` goes from `before` to `after`
/// (`None` drops it).
fn calling_config(before: Value, after: Option<Value>) -> Verdict {
    let (s, mut body, _) = original(ID);
    body["toolConfig"]["functionCallingConfig"] = before;
    let ir = nullrouter_wire::codec::request::decode(&s, &body).unwrap();
    let mut edited = body.clone();
    match after {
        Some(a) => edited["toolConfig"]["functionCallingConfig"] = a,
        None => {
            edited["toolConfig"].as_object_mut().unwrap().remove("functionCallingConfig");
        }
    }
    guard::check_request(&s, &ir, &body, &edited)
}

fn allowed(names: &[&str]) -> Value {
    json!({"mode": "ANY", "allowedFunctionNames": names})
}

/// A client that allows a list of functions (`allowedFunctionNames`) binds the named template
/// once per name, and a list of several reads as `required` with the list beside it. The list
/// is part of the choice: adding or swapping a name changes what a forced call may pick.
#[test]
fn changing_the_allowed_function_names_is_a_violation() {
    violates(&calling_config(allowed(&["read"]), Some(allowed(&["read", "bash"]))), R::ToolCallChanged);
    violates(&calling_config(allowed(&["read", "write"]), Some(allowed(&["read", "exec"]))), R::ToolCallChanged);
    assert_eq!(calling_config(allowed(&["read", "write"]), Some(allowed(&["read", "write"]))), Verdict::Ok);
}

/// Dropping the list but keeping `mode: ANY` still forces a call, now to any declared tool.
#[test]
fn dropping_the_function_list_while_still_forcing_a_call_is_a_violation() {
    violates(&calling_config(allowed(&["read", "write"]), Some(json!({"mode": "ANY"}))), R::ToolCallChanged);
}

/// Dropping the whole choice forces no call, so it passes, list and all.
#[test]
fn removing_a_choice_with_its_function_list_passes() {
    assert_eq!(calling_config(allowed(&["read", "write"]), None), Verdict::Ok);
}
