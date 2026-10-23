//! The adapter forms: `/do/adapters/<op>` installs, reviews, builds, decides, clears, rebuilds and
//! removes harness adapters and sets the review model, `/do/catalogue/install` installs one from
//! the catalogue, and `/do/alerts/ack` acknowledges adapter alerts. They call the shared ops in
//! `nullrouter_server::ops::adapters`. Review, build, rebuild and catalogue install start a task
//! (the form redirects with `?task=`); the confirm windows of clear and remove are the dashboard's
//! own, so the ops do not ask.

use std::path::Path;

use nullrouter_server::ops::adapters as ops;
use nullrouter_server::ops::{Ctx, OpError};

use super::{Form, Ran};

/// `harness` or `harness version`, the subject of a change-log line.
fn subject(form: &Form) -> String {
    match form.opt("version") {
        Some(v) => format!("{} {v}", form.get("harness")),
        None => form.get("harness").to_owned(),
    }
}

pub fn install(ctx: &Ctx, form: &Form) -> Ran {
    let path = form.get("path").trim();
    (path.to_owned(), ops::install(ctx, Path::new(path)))
}

pub fn catalogue_install(ctx: &Ctx, form: &Form) -> Ran {
    (subject(form), ops::catalogue_install(ctx, form.get("harness"), form.opt("version")))
}

pub fn review(ctx: &Ctx, form: &Form) -> Ran {
    (subject(form), ops::review(ctx, form.get("harness"), form.get("version")))
}

pub fn build(ctx: &Ctx, form: &Form) -> Ran {
    (subject(form), ops::build(ctx, form.get("harness"), form.get("version")))
}

pub fn rebuild(ctx: &Ctx, form: &Form) -> Ran {
    (subject(form), ops::rebuild(ctx, form.opt("harness")))
}

pub fn approve(ctx: &Ctx, form: &Form) -> Ran {
    (subject(form), ops::approve(ctx, form.get("harness"), form.get("version"), form.opt("note")))
}

pub fn reject(ctx: &Ctx, form: &Form) -> Ran {
    (subject(form), ops::reject(ctx, form.get("harness"), form.get("version"), form.opt("note")))
}

pub fn clear(ctx: &Ctx, form: &Form) -> Ran {
    (subject(form), ops::clear(ctx, form.get("harness"), form.get("version")))
}

pub fn remove(ctx: &Ctx, form: &Form) -> Ran {
    (subject(form), ops::remove(ctx, form.get("harness"), form.opt("version"), form.flag("force")))
}

/// A number field: absent is `None`, anything but digits names the field.
fn number<T: std::str::FromStr>(form: &Form, field: &'static str) -> Result<Option<T>, OpError> {
    match form.opt(field) {
        None => Ok(None),
        Some(t) => t.trim().parse().map(Some).map_err(|_| OpError {
            text: format!("{field} must be a whole number"),
            exit: 1,
            field: Some(field),
        }),
    }
}

pub fn review_settings(ctx: &Ctx, form: &Form) -> Ran {
    let subject = "adapters.review_settings".to_owned();
    let parsed = number::<u64>(form, "budget").and_then(|b| Ok((b, number::<u32>(form, "reserve_output")?)));
    let result = parsed.and_then(|(budget, reserve)| {
        ops::review_settings(ctx, form.opt("model"), budget, reserve, form.flag("clear"))
    });
    (subject, result)
}

pub fn ack(ctx: &Ctx, form: &Form) -> Ran {
    let all = form.get("id") == "all" || form.flag("all");
    let id = form.opt("id").filter(|_| !all);
    (if all { "all".to_owned() } else { form.get("id").to_owned() }, ops::alerts_ack(ctx, id, all))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::lookup;

    #[test]
    fn adapter_catalogue_and_alert_forms_are_registered() {
        for (area, op, id) in [
            ("adapters", "install", "adapters.install"),
            ("adapters", "review", "adapters.review"),
            ("adapters", "build", "adapters.build"),
            ("adapters", "approve", "adapters.approve"),
            ("adapters", "reject", "adapters.reject"),
            ("adapters", "clear", "adapters.clear"),
            ("adapters", "rebuild", "adapters.rebuild"),
            ("adapters", "remove", "adapters.remove"),
            ("adapters", "review-settings", "adapters.review_settings"),
            ("catalogue", "install", "catalogue.install"),
            ("alerts", "ack", "alerts.ack"),
        ] {
            let (got, _) = lookup(area, op).expect(id);
            assert_eq!(got, id);
        }
    }

    #[test]
    fn a_subject_names_the_harness_and_its_version() {
        let f = Form::from_pairs(&[("harness", "claude-code"), ("version", "v1")]);
        assert_eq!(subject(&f), "claude-code v1");
        assert_eq!(subject(&Form::from_pairs(&[("harness", "claude-code")])), "claude-code");
    }

    #[test]
    fn a_budget_that_is_not_a_number_names_its_field() {
        let f = Form::from_pairs(&[("budget", "lots")]);
        let e = number::<u64>(&f, "budget").unwrap_err();
        assert_eq!(e.field, Some("budget"));
        assert_eq!(number::<u64>(&Form::from_pairs(&[]), "budget").unwrap(), None);
        assert_eq!(number::<u64>(&Form::from_pairs(&[("budget", " 5000 ")]), "budget").unwrap(), Some(5000));
    }
}
