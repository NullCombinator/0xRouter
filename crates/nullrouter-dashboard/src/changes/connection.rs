//! The connection forms: `/do/connection/<op>` sets or removes a timeout, switch or retry rule of a
//! provider, or of one of its models, and calls the shared op in `nullrouter_server::ops::connection`.

use nullrouter_server::ops::{Ctx, connection as ops};

use super::{Form, Ran};

/// The log's subject for a connection setting: the provider id.
pub fn set(ctx: &Ctx, form: &Form) -> Ran {
    let provider = form.get("provider");
    (provider.to_owned(), ops::set(ctx, provider, form.opt("model"), form.get("key"), form.get("value")))
}

pub fn unset(ctx: &Ctx, form: &Form) -> Ran {
    let provider = form.get("provider");
    (provider.to_owned(), ops::unset(ctx, provider, form.opt("model"), form.get("key")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::lookup;

    #[test]
    fn connection_forms_are_registered() {
        for (op, id) in [("set", "connection.set"), ("unset", "connection.unset")] {
            let (got, _) = lookup("connection", op).expect(id);
            assert_eq!(got, id);
        }
    }

    #[test]
    fn an_empty_model_is_absent() {
        let form = Form::from_pairs(&[("provider", "anthropic"), ("model", "")]);
        assert_eq!(form.opt("model"), None);
    }
}
