//! The account forms: `/do/accounts/<op>` reads its fields and calls the shared op in
//! `nullrouter_server::ops::accounts`. The secret goes straight into a `SecretString`.

use nullrouter_engine::accounts::SecretSource;
use nullrouter_registry::SecretString;
use nullrouter_server::ops::{Ctx, OpError, TaskId, accounts as ops};

use super::{Form, Ran};

/// The log's subject for an account: `<provider>/<account>`.
fn subject(form: &Form) -> String {
    format!("{}/{}", form.get("provider"), form.get("name"))
}

/// The checked fields of `accounts.add`. An empty secret is refused with the CLI's words for an
/// empty stdin secret; `order`, when present, must be a number.
fn parse_add(form: &Form) -> Result<(SecretString, Option<i64>), OpError> {
    let secret = form.get("secret");
    if secret.trim().is_empty() {
        return Err(OpError { text: "no key pasted".into(), exit: 1, field: Some("secret") });
    }
    let order = match form.opt("order") {
        Some(v) => Some(v.trim().parse::<i64>().map_err(|_| OpError {
            text: "order: not a number".into(),
            exit: 1,
            field: Some("order"),
        })?),
        None => None,
    };
    Ok((SecretString::new(secret), order))
}

/// A pasted secret (the form's `secret` field), as the CLI's stdin secret is.
pub fn add(ctx: &Ctx, form: &Form) -> Ran {
    let result = parse_add(form).and_then(|(secret, order)| {
        ops::add(ctx, form.get("provider"), form.get("name"), SecretSource::Literal, Some(secret), order)
    });
    (subject(form), result)
}

pub fn priority(ctx: &Ctx, form: &Form) -> Ran {
    (subject(form), ops::priority(ctx, form.get("provider"), form.get("name"), form.get("priority")))
}

pub fn enable(ctx: &Ctx, form: &Form) -> Ran {
    (subject(form), ops::enable(ctx, form.get("provider"), form.get("name")))
}

pub fn disable(ctx: &Ctx, form: &Form) -> Ran {
    (subject(form), ops::disable(ctx, form.get("provider"), form.get("name")))
}

pub fn exclusive(ctx: &Ctx, form: &Form) -> Ran {
    (subject(form), ops::exclusive(ctx, form.get("provider"), form.get("name"), form.flag("on")))
}

pub fn remove(ctx: &Ctx, form: &Form) -> Ran {
    (subject(form), ops::remove(ctx, form.get("provider"), form.get("name")))
}

/// Starts a sign-in as a task. `accept_terms` answers the provider's terms warning; the dashboard's
/// origin is where a loopback redirect returns.
pub fn signin(ctx: &Ctx, form: &Form) -> Ran {
    let result = ops::signin(ctx, form.get("provider"), form.get("name"), form.flag("accept_terms"), form.origin());
    (subject(form), result)
}

/// The checked fields of `accounts.signin_paste`: the task id, and the pasted code, which is held
/// as a secret from here on. An id that is not a task id is refused as no such task.
fn parse_paste(form: &Form) -> Result<(TaskId, SecretString), OpError> {
    let task = form.get("task").parse::<TaskId>().map_err(|_| OpError {
        text: "no such task".into(),
        exit: 1,
        field: Some("task"),
    })?;
    let code = form.get("code").trim();
    if code.is_empty() {
        return Err(OpError { text: "no code pasted".into(), exit: 1, field: Some("code") });
    }
    Ok((task, SecretString::new(code)))
}

/// Hands the pasted code to the sign-in task that waits for it.
pub fn signin_paste(ctx: &Ctx, form: &Form) -> Ran {
    let subject = format!("task {}", form.get("task"));
    let result = parse_paste(form).and_then(|(task, code)| ops::signin_paste(ctx, &task, code));
    (subject, result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::lookup;

    #[test]
    fn add_refuses_an_empty_secret_in_the_cli_words() {
        let form = Form::from_pairs(&[("provider", "anthropic"), ("name", "work"), ("secret", "  ")]);
        let e = parse_add(&form).expect_err("an empty secret is refused");
        assert_eq!(e.text, "no key pasted");
        assert_eq!(e.field, Some("secret"));
        assert_eq!(e.exit, 1);
    }

    #[test]
    fn add_refuses_an_order_that_is_not_a_number() {
        let form = Form::from_pairs(&[("secret", "sk-test-0001"), ("order", "first")]);
        let e = parse_add(&form).expect_err("a bad order is refused");
        assert_eq!(e.text, "order: not a number");
        assert_eq!(e.field, Some("order"));
        assert_eq!(e.exit, 1);
    }

    #[test]
    fn add_keeps_the_secret_and_reads_the_order() {
        let form = Form::from_pairs(&[("secret", "sk-test-0001"), ("order", "7")]);
        let (secret, order) = parse_add(&form).expect("valid fields parse");
        assert!(secret.with_exposed(|s| s == "sk-test-0001"));
        assert_eq!(order, Some(7));

        let form = Form::from_pairs(&[("secret", "sk-test-0001")]);
        let (_, order) = parse_add(&form).expect("an absent order parses");
        assert_eq!(order, None);
    }

    #[test]
    fn account_forms_are_registered() {
        for (op, id) in [
            ("add", "accounts.add"),
            ("priority", "accounts.priority"),
            ("enable", "accounts.enable"),
            ("disable", "accounts.disable"),
            ("exclusive", "accounts.exclusive"),
            ("remove", "accounts.remove"),
            ("signin", "accounts.signin"),
            ("signin-paste", "accounts.signin_paste"),
        ] {
            let (got, _) = lookup("accounts", op).expect(id);
            assert_eq!(got, id);
        }
    }

    #[test]
    fn paste_refuses_a_task_id_that_is_not_one_and_an_empty_code() {
        let form = Form::from_pairs(&[("task", "not-a-task"), ("code", "c")]);
        let e = parse_paste(&form).expect_err("a bad id is refused");
        assert_eq!((e.text.as_str(), e.field, e.exit), ("no such task", Some("task"), 1));

        let form = Form::from_pairs(&[("task", "tk_abcdefghijklmnop"), ("code", "  ")]);
        let e = parse_paste(&form).expect_err("an empty code is refused");
        assert_eq!((e.text.as_str(), e.field), ("no code pasted", Some("code")));
    }

    #[test]
    fn paste_keeps_the_code_as_a_secret() {
        let form = Form::from_pairs(&[("task", "tk_abcdefghijklmnop"), ("code", " sekrit-code ")]);
        let (task, code) = parse_paste(&form).expect("valid fields parse");
        assert_eq!(task.as_str(), "tk_abcdefghijklmnop");
        assert!(code.with_exposed(|c| c == "sekrit-code"));
    }
}
