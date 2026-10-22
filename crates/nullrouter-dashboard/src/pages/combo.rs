//! Combo (spec 014 US3): the forms that declare, change and remove unified models and combos, on the
//! ops the CLI runs (`unified add|set|remove`, `combos add|set|remove`), and the model tests: Test
//! All, and a Test form for one target. A test that bills more than one call asks first, with the
//! CLI's own question. The task panel at the top shows a running test's progress and its verdict.
//!
//! The page lists every declared target from the `unified` and `combos` views, each with its members,
//! an Edit form, a Remove link and a Test link. The test plan is read in process
//! (`model_tests::plan` on the engine's snapshot: no network call, no socket).

use maud::{Markup, html};
use nullrouter_server::ops::{self, model_tests};
use serde_json::Value;
use serde_json::json;

use super::{Body, Ctx, Failure, Refused, Req};
use crate::access::encode_component;
use crate::components::{Head, Tone, badge, card, confirm_window, field, form, link_button, modal, notice, textarea};
use crate::page::{ViewName, Want};

pub const VIEWS: &[ViewName] = &[ViewName::Check, ViewName::Unified, ViewName::Combos];

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
    vec![
        Want::new(ViewName::Check, json!({})),
        Want::new(ViewName::Unified, json!({})),
        Want::new(ViewName::Combos, json!({})),
    ]
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
        (target_card(ctx, &UNIFIED))
        (target_card(ctx, &COMBOS))
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
/// at the top of the page and keeps the values here, beside the field it names. `fixed` is the name
/// of a target being edited (a hidden field, its members prefilled).
fn declare_form(req: &Req, a: &Area, op: &str, submit: &str, fixed: Option<(&str, &str)>) -> Markup {
    let window = match fixed {
        Some((name, _)) => format!("{op}-{}-{name}", a.area),
        None => format!("{op}-{}", a.area),
    };
    let err = refused_form(req, &window);
    let members = kept(err, "members").or(fixed.map(|(_, m)| m));
    form(
        &format!("/do/{}/{op}", a.area),
        BACK,
        &req.token,
        html! {
            input type="hidden" name="_window" value=(window);
            @if let Some((name, _)) = fixed {
                input type="hidden" name="name" value=(name);
            } @else {
                (field("name", "Name", kept(err, "name"), error_on(err, "name", FIELDS)))
            }
            (textarea("members", "Members, one per line", members, error_on(err, "members", FIELDS)))
            button class="button button--primary" type="submit" { (submit) }
        },
    )
}

/// The members of a listed entry, one per line, as the Edit form takes them.
fn member_lines(a: &Area, entry: &Value) -> String {
    let lines = entry["members"].as_array().into_iter().flatten().filter_map(|m| {
        if a.area == UNIFIED.area {
            Some(format!("{}/{}", m["provider"].as_str()?, m["requested"].as_str()?))
        } else {
            m["name"].as_str().map(str::to_owned)
        }
    });
    lines.collect::<Vec<_>>().join("\n")
}

/// One listed target: its members, Test and Remove links, and its Edit form.
fn target_row(req: &Req, a: &Area, entry: &Value) -> Markup {
    let name = entry["name"].as_str().unwrap_or_default();
    let lines = member_lines(a, entry);
    let enc = encode_component(name);
    html! {
        div class="model-row" {
            div class="model-row__head" {
                code class="code model-row__id" title=(name) { (name) }
                @if let Some(k) = entry["model_kind"].as_str() { (badge(Tone::Primary, k)) }
            }
            ul class="model-row__facts" {
                @for l in lines.lines() { li { code class="code" { (l) } } }
            }
            div class="model-row__actions" {
                (link_button(&format!("{BACK}?window=test&target={enc}"), "Test", Some("check"), false))
                (link_button(&format!("{BACK}?window={}&name={enc}", a.remove), &format!("Remove {}", a.noun), None, false))
                (declare_form(req, a, "set", &format!("Save {}", a.noun), Some((name, &lines))))
            }
        }
    }
}

/// One kind of target: what is declared (with its Edit forms) and the form that declares another.
fn target_card(ctx: &Ctx<'_>, a: &Area) -> Markup {
    let (view, key) =
        if a.area == UNIFIED.area { (ViewName::Unified, "unified") } else { (ViewName::Combos, "combos") };
    let listed = ctx.json(view);
    let entries = listed[key].as_array().map(Vec::as_slice).unwrap_or_default();
    let dropped = listed["dropped"].as_array().map(Vec::as_slice).unwrap_or_default();
    let head = Head::new(a.icon, a.title).subtitle("Declare, change or remove; members one per line");
    card(
        Some(head),
        html! {
            @if entries.is_empty() { p class="hint" { "None declared." } }
            @for e in entries { (target_row(ctx.req, a, e)) }
            @for d in dropped {
                p class="hint" { "Dropped: " code class="code" { (d["name"].as_str().unwrap_or_default()) } " " (d["reason"].as_str().unwrap_or_default()) }
            }
            (declare_form(ctx.req, a, "add", &format!("Add {}", a.noun), None))
        },
    )
}

/// The test card: Test All; each listed target has its own Test link.
fn test_card() -> Markup {
    let head = Head::new("check", "Test").subtitle("Model tests, as nullrouter test runs them");
    card(
        Some(head),
        html! {
            p class="hint" { "Test All tests every pair; each target below has its own Test link." }
            (link_button(&format!("{BACK}?window=test&target=all"), "Test All", Some("check"), true))
        },
    )
}

/// The test window (`?window=test&target=<t>`): the plan's question when the run bills more than one
/// call, otherwise one click; a plan that fails shows its text. `all` is every pair.
fn test_window(ctx: &Ctx<'_>) -> Option<Markup> {
    let target = ctx.req.get("target")?;
    let ops_ctx = ctx.ops();
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
