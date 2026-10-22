//! The model-test and verdict forms: `/do/test/run` runs a test, `/do/verdicts/<op>` marks or clears
//! a model's verdict or tunes the test settings, and calls the shared ops in
//! `nullrouter_server::ops::model_tests`. A test that bills more than one call asks first: the
//! `confirmed` flag is the answer.

use nullrouter_server::ops::{Ctx, model_tests as ops};

use super::{Form, Ran};

/// The target a test runs on: `all` is every pair (`None`), else the combo or model named.
fn target_of(text: &str) -> Option<&str> {
    match text.trim() {
        "all" => None,
        t => Some(t),
    }
}

/// The subject of a verdict: `<provider>/<account>/<model>`.
fn verdict(form: &Form) -> String {
    format!("{}/{}/{}", form.get("provider"), form.get("account"), form.get("model"))
}

pub fn run(ctx: &Ctx, form: &Form) -> Ran {
    let text = form.get("target").trim();
    let result = ops::run(ctx, target_of(text), form.opt("account"), form.flag("confirmed"));
    (text.to_owned(), result)
}

pub fn mark(ctx: &Ctx, form: &Form) -> Ran {
    let result = ops::mark(ctx, form.get("provider"), form.get("account"), form.get("model"), form.opt("note"));
    (verdict(form), result)
}

pub fn clear(ctx: &Ctx, form: &Form) -> Ran {
    let result = ops::clear(ctx, form.get("provider"), form.get("account"), form.get("model"));
    (verdict(form), result)
}

pub fn settings(ctx: &Ctx, form: &Form) -> Ran {
    let setting = form.get("setting");
    let result = ops::settings(ctx, setting, form.opt("type"), form.get("value"));
    (format!("verdicts.{setting}"), result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::lookup;

    #[test]
    fn target_all_is_none() {
        assert_eq!(target_of("all"), None);
        assert_eq!(target_of("  all "), None);
        assert_eq!(target_of("fast"), Some("fast"));
    }

    #[test]
    fn a_verdict_names_provider_account_and_model() {
        let form = Form::from_pairs(&[("provider", "openai"), ("account", "main"), ("model", "gpt-x")]);
        assert_eq!(verdict(&form), "openai/main/gpt-x");
    }

    #[test]
    fn test_and_verdict_forms_are_registered() {
        for (area, op, id) in [
            ("test", "run", "test.run"),
            ("verdicts", "mark", "verdicts.mark"),
            ("verdicts", "clear", "verdicts.clear"),
            ("verdicts", "settings", "verdicts.settings"),
        ] {
            let (got, _) = lookup(area, op).expect(id);
            assert_eq!(got, id);
        }
    }
}
