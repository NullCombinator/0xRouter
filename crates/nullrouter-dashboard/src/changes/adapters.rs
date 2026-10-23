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
    // A browser install takes only an existing regular `.tar.gz` file, never a link or a directory
    // (security review L5). Nothing but the path is shown back.
    let refuse = |why: &str| {
        let text = format!("{why}: give the path of a .tar.gz or .tgz package file");
        (path.to_owned(), Err(OpError { text, exit: 1, field: Some("path") }))
    };
    let archive = path.ends_with(".tar.gz") || path.ends_with(".tgz");
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_file() && archive => (path.to_owned(), ops::install(ctx, Path::new(path))),
        Ok(m) if m.file_type().is_symlink() => refuse("that path is a link"),
        Ok(m) if !m.is_file() => refuse("that path is not a regular file"),
        Ok(_) => refuse("that file is not a package archive"),
        // Nothing there: the op's own "No such file" text, the one the CLI gives.
        Err(_) => (path.to_owned(), ops::install(ctx, Path::new(path))),
    }
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

    fn form(path: &str) -> Form {
        Form { fields: [("path".to_owned(), path.to_owned())].into(), origin: None }
    }

    #[test]
    fn install_refuses_links_directories_and_non_archives_without_reading_them() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = Ctx { home: nullrouter_registry::OperatorHome::new(dir.path()), engine: None };
        let plain = dir.path().join("secret.txt");
        std::fs::write(&plain, "FILE-BYTES-MARKER").unwrap();
        let archive = dir.path().join("real.tar.gz");
        std::fs::write(&archive, "x").unwrap();
        let link = dir.path().join("link.tar.gz");
        std::os::unix::fs::symlink(&archive, &link).unwrap();
        for p in [&plain, &link, &dir.path().to_owned()] {
            let (_, r) = install(&ctx, &form(p.to_str().unwrap()));
            let e = r.unwrap_err();
            assert_eq!(e.field, Some("path"), "{p:?}");
            assert!(!e.text.contains("FILE-BYTES-MARKER"));
        }
    }

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
