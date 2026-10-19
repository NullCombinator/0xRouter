//! The guardrail over gemini bodies (T034).

suite!("gemini");

/// A client that allows a list of functions (`allowedFunctionNames`) binds the named template
/// once per name. Adding or swapping a name must not pass as the same named choice.
#[test]
fn adding_or_swapping_an_allowed_function_name_is_a_violation() {
    let config = |names: &[&str]| json!({"mode": "ANY", "allowedFunctionNames": names});
    let check = |before: &[&str], after: &[&str]| {
        let (s, mut body, _) = original(ID);
        body["toolConfig"]["functionCallingConfig"] = config(before);
        let ir = nullrouter_wire::codec::request::decode(&s, &body).unwrap();
        let mut edited = body.clone();
        edited["toolConfig"]["functionCallingConfig"] = config(after);
        guard::check_request(&s, &ir, &body, &edited)
    };
    violates(&check(&["read"], &["read", "bash"]), R::UnplacedAdded);
    violates(&check(&["read", "write"], &["read", "exec"]), R::UnplacedAdded);
    assert_eq!(check(&["read", "write"], &["read", "write"]), Verdict::Ok);
}
