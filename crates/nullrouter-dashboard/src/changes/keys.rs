//! The client-key forms: `/do/keys/<op>` reads its fields and calls the shared op in
//! `nullrouter_server::ops::keys`. An empty optional field clears where the op can clear.

use nullrouter_server::ops::{Ctx, keys as ops};

use super::{Form, Ran};

/// An optional field's value and whether it asks to clear: empty (or absent) clears.
fn optional<'a>(form: &'a Form, name: &str) -> (Option<&'a str>, bool) {
    let value = form.opt(name);
    (value, value.is_none())
}

pub fn issue(ctx: &Ctx, form: &Form) -> Ran {
    let name = form.get("name");
    (name.to_owned(), ops::issue(ctx, name, form.opt("break"), form.opt("harness"), form.opt("adapter")))
}

pub fn revoke(ctx: &Ctx, form: &Form) -> Ran {
    let key = form.get("key");
    (key.to_owned(), ops::revoke(ctx, key))
}

pub fn tag(ctx: &Ctx, form: &Form) -> Ran {
    let key = form.get("key");
    let (text, clear) = optional(form, "text");
    (key.to_owned(), ops::tag(ctx, key, text, clear))
}

pub fn set_break(ctx: &Ctx, form: &Form) -> Ran {
    let key = form.get("key");
    (key.to_owned(), ops::set_break(ctx, key, form.get("behaviour")))
}

pub fn set_adapter(ctx: &Ctx, form: &Form) -> Ran {
    let key = form.get("key");
    let (harness, clear) = optional(form, "harness");
    (key.to_owned(), ops::set_adapter(ctx, key, harness, clear))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::lookup;

    #[test]
    fn an_empty_optional_field_clears() {
        let form = Form::from_pairs(&[("text", "")]);
        assert_eq!(optional(&form, "text"), (None, true));
        assert_eq!(optional(&form, "harness"), (None, true), "an absent field clears too");

        let form = Form::from_pairs(&[("harness", "claude-code")]);
        assert_eq!(optional(&form, "harness"), (Some("claude-code"), false));
    }

    #[test]
    fn key_forms_are_registered() {
        for (op, id) in [
            ("issue", "keys.issue"),
            ("revoke", "keys.revoke"),
            ("tag", "keys.tag"),
            ("set-break", "keys.set_break"),
            ("set-adapter", "keys.set_adapter"),
        ] {
            let (got, _) = lookup("keys", op).expect(id);
            assert_eq!(got, id);
        }
    }
}
