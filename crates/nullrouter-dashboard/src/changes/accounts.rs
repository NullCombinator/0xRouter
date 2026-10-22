//! The account forms: `/do/accounts/<op>` reads its fields and calls the shared op in
//! `nullrouter_server::ops::accounts`. The secret goes straight into a `SecretString`.

use nullrouter_engine::accounts::SecretSource;
use nullrouter_registry::SecretString;
use nullrouter_server::ops::{Ctx, OpError, accounts as ops};

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

// T014: accounts.signin, accounts.signin_paste

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
        ] {
            let (got, _) = lookup("accounts", op).expect(id);
            assert_eq!(got, id);
        }
    }
}
