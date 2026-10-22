//! The routing-target forms: `/do/unified/<op>` and `/do/combos/<op>` declare, replace or remove a
//! unified model or combo, and call the shared op in `nullrouter_server::ops::targets`. Members come
//! one per line; a blank line is no member.

use nullrouter_server::ops::Ctx;
use nullrouter_server::ops::targets::{self, Kind};

use super::{Form, Ran};

/// The members in a field: one per line, each trimmed, blank lines dropped (CRLF included).
pub fn members(text: &str) -> Vec<String> {
    text.lines().map(str::trim).filter(|m| !m.is_empty()).map(str::to_owned).collect()
}

/// The subject a log line names: `unified:<name>` or `combo:<name>`.
fn subject(kind: Kind, name: &str) -> String {
    match kind {
        Kind::Unified => format!("unified:{name}"),
        Kind::Combo => format!("combo:{name}"),
    }
}

fn add(ctx: &Ctx, form: &Form, kind: Kind) -> Ran {
    let name = form.get("name");
    let list = members(form.get("members"));
    let result = targets::add_with(ctx, &mut Vec::new(), kind, name, &list).map(|r| r.done);
    (subject(kind, name), result)
}

fn set(ctx: &Ctx, form: &Form, kind: Kind) -> Ran {
    let name = form.get("name");
    let list = members(form.get("members"));
    let result = targets::set_with(ctx, &mut Vec::new(), kind, name, &list).map(|r| r.done);
    (subject(kind, name), result)
}

fn remove(ctx: &Ctx, form: &Form, kind: Kind) -> Ran {
    let name = form.get("name");
    let result = targets::remove_with(ctx, &mut Vec::new(), kind, name).map(|r| r.done);
    (subject(kind, name), result)
}

pub fn unified_add(ctx: &Ctx, form: &Form) -> Ran {
    add(ctx, form, Kind::Unified)
}

pub fn unified_set(ctx: &Ctx, form: &Form) -> Ran {
    set(ctx, form, Kind::Unified)
}

pub fn unified_remove(ctx: &Ctx, form: &Form) -> Ran {
    remove(ctx, form, Kind::Unified)
}

pub fn combos_add(ctx: &Ctx, form: &Form) -> Ran {
    add(ctx, form, Kind::Combo)
}

pub fn combos_set(ctx: &Ctx, form: &Form) -> Ran {
    set(ctx, form, Kind::Combo)
}

pub fn combos_remove(ctx: &Ctx, form: &Form) -> Ran {
    remove(ctx, form, Kind::Combo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::lookup;

    #[test]
    fn members_split_on_lines_and_drop_blanks() {
        assert_eq!(members("a/b\r\n\r\n  c/d  \n\n"), vec!["a/b", "c/d"]);
        assert!(members("\r\n  \n").is_empty());
    }

    #[test]
    fn target_forms_are_registered() {
        for (area, op, id) in [
            ("unified", "add", "unified.add"),
            ("unified", "set", "unified.set"),
            ("unified", "remove", "unified.remove"),
            ("combos", "add", "combos.add"),
            ("combos", "set", "combos.set"),
            ("combos", "remove", "combos.remove"),
        ] {
            let (got, _) = lookup(area, op).expect(id);
            assert_eq!(got, id);
        }
    }

    #[test]
    fn a_subject_names_its_kind() {
        assert_eq!(subject(Kind::Unified, "fast"), "unified:fast");
        assert_eq!(subject(Kind::Combo, "fast"), "combo:fast");
    }
}
