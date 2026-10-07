//! Spec 011 T013, SC-001: `verdict::judge` over research R3's table.

use nullrouter_engine::records::ErrorClass;
use nullrouter_engine::verdict::judge::{Failed, judge};
use nullrouter_engine::verdict::{Rejection, State};
use nullrouter_registry::validate::{GateCtx, validate_with};
use nullrouter_registry::{PluginSource, ProviderEntity};

const PLAIN: &str = "schema = 2\nid = \"p\"\ncategory = \"apikey\"\n[endpoints.text]\nurl = \"https://api.example.com/v1/chat/completions\"\nwire = \"openai-chat\"\n";

fn provider(extra: &str) -> ProviderEntity {
    let ctx = GateCtx { allow_private: true, ..GateCtx::default() };
    validate_with(&format!("{PLAIN}{extra}"), PluginSource::Bundled, "p.toml", &ctx)
        .unwrap_or_else(|e| panic!("{e:#?}"))
        .entity
}

fn oauth(extra: &str) -> ProviderEntity {
    let ctx = GateCtx { allow_private: true, ..GateCtx::default() };
    let src = format!("{PLAIN}{extra}").replace("\"apikey\"", "\"oauth\"");
    validate_with(&src, PluginSource::Bundled, "p.toml", &ctx).unwrap_or_else(|e| panic!("{e:#?}")).entity
}

type Want = (State, Option<Rejection>);

fn judged(p: &ProviderEntity, status: Option<u16>, class: ErrorClass, message: &str) -> Want {
    let j = judge(&Failed { status, class, message }, p);
    (j.state, j.rejection)
}

#[test]
fn core_list() {
    let p = provider("");
    let broken = |r| (State::Broken, Some(r));
    let unknown = (State::Unknown, None);
    let cases: &[(Option<u16>, &str, Want)] = &[
        (Some(404), "not_found_error: model: claude-opus-9", broken(Rejection::ModelNotFound)),
        (Some(404), "The model 'gpt-9' does not exist", broken(Rejection::ModelNotFound)),
        (Some(400), "x-ai/grok-9 is not a valid model ID", broken(Rejection::ModelNotFound)),
        (Some(422), "Unknown model: foo", broken(Rejection::ModelNotFound)),
        (Some(403), "Your plan does not include access to model claude-opus-4-1", broken(Rejection::ModelNotAvailable)),
        (Some(403), "model claude-opus-4-1 is not available on your plan", broken(Rejection::ModelNotAvailable)),
        (Some(400), "This model does not support embeddings", broken(Rejection::TypeNotSupported)),
        (Some(405), "model is not a chat model", broken(Rejection::TypeNotSupported)),
        // A bare 404 with no model wording: a wrong path answers that too.
        (Some(404), "Not Found", unknown.clone()),
        (Some(400), "messages: field required", unknown.clone()),
        (Some(409), "model not found", unknown.clone()),
        // Never definitive, whatever the text says.
        (Some(408), "model not found", unknown.clone()),
        (Some(429), "model not found", unknown.clone()),
        (Some(500), "model does not exist", unknown.clone()),
        (Some(503), "upstream overloaded", unknown.clone()),
        (None, "timeout after 30 s", unknown.clone()),
        (None, "the stream stalled", unknown.clone()),
        // The account, not the model (FR-009).
        (Some(401), "invalid model api key", unknown.clone()),
        (Some(403), "token expired for model access", unknown.clone()),
    ];
    for (status, message, want) in cases {
        assert_eq!(&judged(&p, *status, ErrorClass::NotFound, message), want, "{status:?} {message}");
    }
    let (s, _) = judged(&p, Some(404), ErrorClass::NeedsSignIn, "model not found");
    assert_eq!(s, State::Unknown, "an out-of-service account says nothing about the model");
}

#[test]
fn reasons_carry_status_and_are_cut() {
    let p = provider("");
    let j = judge(&Failed { status: Some(404), class: ErrorClass::NotFound, message: "model x does not exist" }, &p);
    assert_eq!(j.reason, "404: model x does not exist");
    let long = "model not found ".repeat(100);
    let j = judge(&Failed { status: Some(404), class: ErrorClass::NotFound, message: &long }, &p);
    assert_eq!(j.reason.chars().count(), 500);
    let j = judge(&Failed { status: None, class: ErrorClass::Timeout, message: "timeout after 30 s" }, &p);
    assert_eq!(j.reason, "timeout after 30 s");
}

#[test]
fn a_plugin_rule_wins_over_the_core_list() {
    let p = provider(
        "[[rejections]]\nstatus = [400, 410]\nbody_contains = \"model_retired\"\nreason = \"model_not_found\"\n\n[[rejections]]\nstatus = 404\nbody_contains = \"does not exist\"\nreason = \"model_not_available\"\n",
    );
    assert_eq!(
        judged(&p, Some(410), ErrorClass::RequestError, "{\"code\":\"model_retired\"}"),
        (State::Broken, Some(Rejection::Plugin("model_not_found".into())))
    );
    // The core list would say model_not_found; the plugin's rule is tried first.
    assert_eq!(
        judged(&p, Some(404), ErrorClass::NotFound, "model x does not exist"),
        (State::Broken, Some(Rejection::Plugin("model_not_available".into())))
    );
    // A rule never overrides the not-definitive statuses (the gate refuses them anyway).
    assert_eq!(judged(&p, Some(429), ErrorClass::RateLimited, "model_retired"), (State::Unknown, None));
}

#[test]
fn a_signin_refusal_marks_the_account_not_the_model() {
    let p = oauth(
        "[signin]\nflow = \"device_code\"\nclient_id = \"c\"\ndevice_url = \"https://auth.example.com/device\"\ntoken_url = \"https://auth.example.com/token\"\nrefresh_lead = \"5m\"\n[[signin.refused]]\nstatus = 403\nbody_contains = \"account suspended\"\n",
    );
    assert_eq!(
        judged(&p, Some(403), ErrorClass::Auth, "account suspended: model access not allowed"),
        (State::Unknown, None)
    );
    assert_eq!(
        judged(&p, Some(403), ErrorClass::Auth, "model grok-4 is not available on your plan"),
        (State::Broken, Some(Rejection::ModelNotAvailable))
    );
}
