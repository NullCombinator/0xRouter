//! The plugin forms: `/do/plugins/<op>` installs or uninstalls a community plugin and turns private
//! endpoints on or off, and `/do/providers/add-compatible` writes a custom compatible provider. They
//! call the shared ops in `nullrouter_server::ops::plugins`.

use nullrouter_server::ops::{Ctx, plugins as ops};

use super::targets;
use super::{Form, Ran};

/// The models in a field: one per line, each trimmed, blank lines dropped (CRLF included).
pub fn models(text: &str) -> Vec<String> {
    targets::members(text)
}

pub fn install(ctx: &Ctx, form: &Form) -> Ran {
    let id = form.get("id");
    (id.to_owned(), ops::install(ctx, id))
}

pub fn uninstall(ctx: &Ctx, form: &Form) -> Ran {
    let id = form.get("id");
    (id.to_owned(), ops::uninstall(ctx, id))
}

pub fn private_endpoints(ctx: &Ctx, form: &Form) -> Ran {
    ("plugins.private_endpoints".to_owned(), ops::private_endpoints(ctx, form.flag("on")))
}

pub fn add_compatible(ctx: &Ctx, form: &Form) -> Ran {
    let id = form.get("id");
    let list = models(form.get("models"));
    let result = ops::add_compatible(ctx, id, form.opt("name"), form.get("style"), form.get("base_url"), &list);
    (id.to_owned(), result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::lookup;

    #[test]
    fn models_split_on_lines_and_drop_blanks() {
        assert_eq!(models("gpt-a\r\n\r\n  gpt-b  \n\n"), vec!["gpt-a", "gpt-b"]);
        assert!(models("\r\n  \n").is_empty());
    }

    #[test]
    fn plugin_and_compatible_forms_are_registered() {
        for (area, op, id) in [
            ("plugins", "install", "plugins.install"),
            ("plugins", "uninstall", "plugins.uninstall"),
            ("plugins", "private-endpoints", "plugins.private_endpoints"),
            ("providers", "add-compatible", "providers.add_compatible"),
        ] {
            let (got, _) = lookup(area, op).expect(id);
            assert_eq!(got, id);
        }
    }
}
