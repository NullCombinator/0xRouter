//! The routing forms: `/do/routing/<op>` sets an account's overrides, a provider's plugin weights
//! or the amortization window, and `/do/behaviour/set-break` sets the default break behaviour. Each
//! reads its fields and calls the shared op in `nullrouter_server::ops::routing`.

use nullrouter_server::ops::{Ctx, routing as ops};

use super::{Form, Ran};

/// The log's subject for an account: `<provider>/<account>`.
fn account(form: &Form) -> String {
    format!("{}/{}", form.get("provider"), form.get("name"))
}

/// One `key=value` setting, as the CLI's `routing set` takes it.
fn setting(form: &Form) -> String {
    format!("{}={}", form.get("key"), form.get("value"))
}

pub fn set(ctx: &Ctx, form: &Form) -> Ran {
    let result = ops::set(ctx, form.get("provider"), form.get("name"), &[setting(form)]);
    (account(form), result)
}

pub fn unset(ctx: &Ctx, form: &Form) -> Ran {
    let keys = [form.get("key").to_owned()];
    (account(form), ops::unset(ctx, form.get("provider"), form.get("name"), &keys))
}

pub fn set_plugin(ctx: &Ctx, form: &Form) -> Ran {
    let provider = form.get("provider");
    (provider.to_owned(), ops::set_plugin(ctx, provider, &[setting(form)]))
}

pub fn unset_plugin(ctx: &Ctx, form: &Form) -> Ran {
    let provider = form.get("provider");
    let keys = [form.get("key").to_owned()];
    (provider.to_owned(), ops::unset_plugin(ctx, provider, &keys))
}

/// The amortization window: the default when `target` is empty, else the window for that target.
pub fn window(ctx: &Ctx, form: &Form) -> Ran {
    let target = form.opt("target");
    let subject = match target {
        Some(t) => format!("routing.window.{t}"),
        None => "routing.window".to_owned(),
    };
    (subject, ops::window(ctx, target, form.get("duration")))
}

/// The operator's default break behaviour, which every key without its own inherits.
pub fn set_break(ctx: &Ctx, form: &Form) -> Ran {
    ("behaviour.default".to_owned(), ops::set_break_default(ctx, form.get("behaviour")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::lookup;

    #[test]
    fn a_setting_is_key_equals_value() {
        let form = Form::from_pairs(&[("key", "window.paid.capacity"), ("value", "2")]);
        assert_eq!(setting(&form), "window.paid.capacity=2");
    }

    #[test]
    fn routing_forms_are_registered() {
        for (area, op, id) in [
            ("behaviour", "set-break", "behaviour.set_break"),
            ("routing", "set", "routing.set"),
            ("routing", "unset", "routing.unset"),
            ("routing", "set-plugin", "routing.set_plugin"),
            ("routing", "unset-plugin", "routing.unset_plugin"),
            ("routing", "window", "routing.window"),
        ] {
            let (got, _) = lookup(area, op).expect(id);
            assert_eq!(got, id);
        }
    }
}
