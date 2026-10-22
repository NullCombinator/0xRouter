//! The quota forms: `/do/quota/<op>` sets an account's polling interval, starts a poll, or
//! acknowledges usage alerts, and calls the shared op in `nullrouter_server::ops::quota`.

use nullrouter_server::ops::{Ctx, OpError, quota as ops};

use super::{Form, Ran};

/// The log's subject for an account: `<provider>/<account>`.
fn account(form: &Form) -> String {
    format!("{}/{}", form.get("provider"), form.get("name"))
}

pub fn interval(ctx: &Ctx, form: &Form) -> Ran {
    let mut log = Vec::new();
    let result =
        ops::interval_with(ctx, &mut log, form.get("provider"), form.get("name"), form.get("every")).map(|r| r.done);
    (account(form), result)
}

pub fn poll(ctx: &Ctx, form: &Form) -> Ran {
    (account(form), ops::poll(ctx, form.get("provider"), form.get("name")))
}

/// The checked fields of `quota.ack`: the alert id (`all` for every open alert) and the provider
/// and account it narrows to. An empty id is refused.
fn ack_args(form: &Form) -> Result<(&str, Option<&str>, Option<&str>), OpError> {
    let id = form.get("id").trim();
    if id.is_empty() {
        return Err(OpError { text: "no alert id given".into(), exit: 1, field: Some("id") });
    }
    Ok((id, form.opt("provider"), form.opt("account")))
}

pub fn ack(ctx: &Ctx, form: &Form) -> Ran {
    let subject = match (form.opt("provider"), form.opt("account")) {
        (Some(p), Some(a)) => format!("{p}/{a}"),
        (Some(p), None) => p.to_owned(),
        _ => form.get("id").to_owned(),
    };
    let result = ack_args(form).and_then(|(id, provider, acct)| ops::ack_with(ctx, id, provider, acct).map(|r| r.done));
    (subject, result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::lookup;

    #[test]
    fn ack_all_names_every_open_alert() {
        let form = Form::from_pairs(&[("id", "all")]);
        assert_eq!(ack_args(&form).ok(), Some(("all", None, None)));

        let form = Form::from_pairs(&[("id", "all"), ("provider", "anthropic"), ("account", "")]);
        assert_eq!(ack_args(&form).ok(), Some(("all", Some("anthropic"), None)));
    }

    #[test]
    fn ack_refuses_an_empty_id() {
        let form = Form::from_pairs(&[("id", "  ")]);
        assert!(ack_args(&form).is_err());
    }

    #[test]
    fn quota_forms_are_registered() {
        for (op, id) in [("interval", "quota.interval"), ("poll", "quota.poll"), ("ack", "quota.ack")] {
            let (got, _) = lookup("quota", op).expect(id);
            assert_eq!(got, id);
        }
    }
}
