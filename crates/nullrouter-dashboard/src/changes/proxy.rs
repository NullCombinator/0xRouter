//! The proxy forms: `/do/proxy/<op>` defines, removes, assigns, clears or probes a proxy, and calls
//! the shared op in `nullrouter_server::ops::proxy`. The password is write-only: it goes straight
//! into a `SecretString` and is never logged.

use nullrouter_registry::SecretString;
use nullrouter_server::ops::{Ctx, proxy as ops};

use super::{Form, Ran};

/// A pasted password, when one was given: an empty field is no password.
fn password(form: &Form) -> Option<SecretString> {
    form.opt("password").map(SecretString::new)
}

/// The subject of an assignment: `all` for every provider, else the target as typed.
fn assigned(form: &Form) -> String {
    match form.get("scope") {
        "all" => "all".to_owned(),
        _ => form.get("target").trim().to_owned(),
    }
}

pub fn add(ctx: &Ctx, form: &Form) -> Ran {
    let name = form.get("name");
    let result = ops::add(ctx, &mut Vec::new(), name, form.get("url"), form.opt("username"), password(form));
    (name.to_owned(), result)
}

pub fn remove(ctx: &Ctx, form: &Form) -> Ran {
    let name = form.get("name");
    (name.to_owned(), ops::remove(ctx, &mut Vec::new(), name))
}

pub fn use_proxy(ctx: &Ctx, form: &Form) -> Ran {
    let result = ops::target(form.get("scope"), form.get("target"))
        .and_then(|t| ops::use_proxy(ctx, &mut Vec::new(), form.get("proxy"), &t));
    (assigned(form), result)
}

pub fn clear(ctx: &Ctx, form: &Form) -> Ran {
    let result = ops::target(form.get("scope"), form.get("target")).and_then(|t| ops::clear(ctx, &mut Vec::new(), &t));
    (assigned(form), result)
}

pub fn fixed(ctx: &Ctx, form: &Form) -> Ran {
    let name = form.get("name");
    (name.to_owned(), ops::fixed(ctx, name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::lookup;

    #[test]
    fn an_empty_password_is_none() {
        let form = Form::from_pairs(&[("name", "eu"), ("url", "http://10.0.0.2:3128"), ("password", "")]);
        assert!(password(&form).is_none());

        let form = Form::from_pairs(&[("password", "hunter2")]);
        assert!(password(&form).is_some_and(|p| p.with_exposed(|v| v == "hunter2")));
    }

    #[test]
    fn the_subject_of_an_all_scope_is_all() {
        let form = Form::from_pairs(&[("scope", "all"), ("target", "")]);
        assert_eq!(assigned(&form), "all");

        let form = Form::from_pairs(&[("scope", "provider"), ("target", " anthropic ")]);
        assert_eq!(assigned(&form), "anthropic");
    }

    #[test]
    fn proxy_forms_are_registered() {
        for (op, id) in [
            ("add", "proxy.add"),
            ("remove", "proxy.remove"),
            ("use", "proxy.use"),
            ("clear", "proxy.clear"),
            ("fixed", "proxy.fixed"),
        ] {
            let (got, _) = lookup("proxy", op).expect(id);
            assert_eq!(got, id);
        }
    }
}
