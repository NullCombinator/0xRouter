//! Combo (spec 014 US3): the forms that declare, change and remove unified models and combos, on the
//! ops the CLI runs (`unified add|set|remove`, `combos add|set|remove`), and the model tests: Test
//! All, and a Test form for one target. A test that bills more than one call asks first, with the
//! CLI's own question. The task panel at the top shows a running test's progress and its verdict.
//!
//! No view of this page lists the targets (`ViewName` has no unified or combo list), so a target is
//! named as the operator types it. The test plan is read with `Ctx { home, engine: None }`: the
//! page has no engine, so the plan is asked of the running server over the operator socket.

use maud::{Markup, html};
use nullrouter_registry::OperatorHome;
use nullrouter_server::ops::{self, model_tests};
use serde_json::json;

use super::{Body, Ctx, Failure, Refused, Req};
use crate::access::encode_component;
use crate::components::{Head, card, confirm_window, field, form, link_button, modal, notice, textarea};
use crate::page::{ViewName, Want};

pub const VIEWS: &[ViewName] = &[ViewName::Check];

/// Where every form here returns: this page.
const BACK: &str = "/combo";
/// The fields a declare form takes, the first of which takes a refusal that names none.
const FIELDS: &[&str] = &["name", "members"];

/// One kind of routing target and the ops and windows its forms use.
struct Area {
    /// The form route's area: `/do/<area>/<op>`.
    area: &'static str,
    icon: &'static str,
    title: &'static str,
    noun: &'static str,
    /// The op `describe_loss` names for the remove.
    loss_op: &'static str,
    /// The window a remove opens (`?window=<remove>&name=<n>`).
    remove: &'static str,
}

const UNIFIED: Area = Area {
    area: "unified",
    icon: "layers",
    title: "Unified models",
    noun: "unified model",
    loss_op: "unified.remove",
    remove: "remove-unified",
};

const COMBOS: Area = Area {
    area: "combos",
    icon: "route",
    title: "Combos",
    noun: "combo",
    loss_op: "combos.remove",
    remove: "remove-combo",
};

pub fn wants(_req: &Req) -> Vec<Want> {
    vec![Want::new(ViewName::Check, json!({}))]
}

pub fn body(ctx: &Ctx<'_>) -> Result<Body, Failure> {
    let window = match ctx.req.get("window") {
        Some("test") => test_window(ctx),
        Some(w) if w == UNIFIED.remove => remove_window(ctx, &UNIFIED),
        Some(w) if w == COMBOS.remove => remove_window(ctx, &COMBOS),
        _ => None,
    };
    let content = html! {
        (test_card())
        (declare_card(ctx.req, &UNIFIED))
        (declare_card(ctx.req, &COMBOS))
    };
    Ok(Body { content, side: None, window })
}

/// The refusal of the form `id`: a refused change's redirect names its form by `window`, so only
/// that form shows it.
fn refused_form<'a>(req: &'a Req, id: &str) -> Option<&'a Refused> {
    req.refused.as_ref().filter(|_| req.get("window") == Some(id))
}

/// The text of `r` when it belongs to the field `name`: the field it names, or the form's first
/// field when it names none of `fields`.
fn error_on<'a>(r: Option<&'a Refused>, name: &str, fields: &[&str]) -> Option<&'a str> {
    let r = r?;
    let owner = r.field.as_deref().filter(|f| fields.contains(f)).or(fields.first().copied());
    (owner == Some(name)).then_some(r.text.as_str())
}

/// The value a refused form kept for `name`.
fn kept<'a>(r: Option<&'a Refused>, name: &str) -> Option<&'a str> {
    r?.kept.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

/// The name and members form that adds (`add`) or changes (`set`) a target. A refusal shows its text
/// at the top of the page and keeps the values here, beside the field it names.
fn declare_form(req: &Req, a: &Area, op: &str, submit: &str) -> Markup {
    let window = format!("{op}-{}", a.area);
    let err = refused_form(req, &window);
    form(
        &format!("/do/{}/{op}", a.area),
        BACK,
        &req.token,
        html! {
            input type="hidden" name="_window" value=(window);
            (field("name", "Name", kept(err, "name"), error_on(err, "name", FIELDS)))
            (textarea("members", "Members, one per line", kept(err, "members"), error_on(err, "members", FIELDS)))
            button class="button button--primary" type="submit" { (submit) }
        },
    )
}

/// The remove form: a GET that names the target and opens its confirm window (it changes nothing).
fn remove_card(a: &Area) -> Markup {
    html! {
        form class="form" method="get" action=(BACK) {
            input type="hidden" name="window" value=(a.remove);
            (field("name", "Name", None, None))
            button class="button button--secondary" type="submit" { "Remove " (a.noun) }
        }
    }
}

/// One kind of target: its add and change forms, and its remove form.
fn declare_card(req: &Req, a: &Area) -> Markup {
    let head = Head::new(a.icon, a.title).subtitle("Declare, change or remove; members one per line");
    card(
        Some(head),
        html! {
            (declare_form(req, a, "add", &format!("Add {}", a.noun)))
            (declare_form(req, a, "set", &format!("Save {}", a.noun)))
            (remove_card(a))
        },
    )
}

/// The test card: Test All, and a Test form for one target (a unified model, a combo or
/// `provider/model`), which opens the test window.
fn test_card() -> Markup {
    let head = Head::new("check", "Test").subtitle("Model tests, as nullrouter test runs them");
    card(
        Some(head),
        html! {
            p class="hint" { "A target is a unified model, a combo, or provider/model. Test All tests every pair." }
            (link_button(&format!("{BACK}?window=test&target=all"), "Test All", Some("check"), true))
            form class="form" method="get" action=(BACK) {
                input type="hidden" name="window" value="test";
                (field("target", "Target", None, None))
                button class="button button--secondary" type="submit" { "Test" }
            }
        },
    )
}

/// The test window (`?window=test&target=<t>`): the plan's question when the run bills more than one
/// call, otherwise one click; a plan that fails shows its text. `all` is every pair.
fn test_window(ctx: &Ctx<'_>) -> Option<Markup> {
    let target = ctx.req.get("target")?;
    let Some(home) = ctx.json(ViewName::Check)["home"].as_str() else {
        return Some(modal("Test", BACK, notice("error", "The operator home is not known.")));
    };
    let ops_ctx = ops::Ctx { home: OperatorHome::new(home), engine: None };
    let who = (target != "all").then_some(target);
    // The run posts the target; `confirmed` is the answer to the plan's question.
    let run = |confirmed: bool| {
        form(
            "/do/test/run",
            BACK,
            &ctx.req.token,
            html! {
                input type="hidden" name="target" value=(target);
                @if confirmed { input type="hidden" name="confirmed" value="on"; }
                button class="button button--primary" type="submit" { "Run test" }
            },
        )
    };
    Some(match model_tests::plan(&ops_ctx, who, None) {
        Err(e) => modal("Test", BACK, notice("error", &e.text)),
        Ok(plan) if plan.needs_confirm() => confirm_window("Test", &plan.question(), BACK, run(true)),
        Ok(_) => modal("Test", BACK, run(false)),
    })
}

/// The remove confirm window (`?window=<remove>&name=<n>`): what removing loses, the real form beside
/// Cancel. Nothing opens without a name. A refused remove reopens it: `_back` keeps the name.
fn remove_window(ctx: &Ctx<'_>, a: &Area) -> Option<Markup> {
    let name = ctx.req.get("name")?;
    let loss = ops::describe_loss(a.loss_op, name).unwrap_or_default();
    let back = format!("{BACK}?name={}", encode_component(name));
    let action = form(
        &format!("/do/{}/remove", a.area),
        &back,
        &ctx.req.token,
        html! {
            input type="hidden" name="_window" value=(a.remove);
            input type="hidden" name="name" value=(name);
            button class="button button--primary" type="submit" { "Remove " (a.noun) }
        },
    );
    Some(confirm_window(&format!("Remove {}", a.noun), &loss, BACK, action))
}
