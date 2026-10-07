//! Usage (contracts/dashboard-http.md "Usage"): the places kept for the period filter, the stat
//! cards and the topology graph, "Recent Requests", and the Requests table with the window one
//! row opens. Everything shown is a record `records list` or `records show` holds, in the words
//! `records` uses: a number the record doesn't hold is "not reported" or "-", never zero, and
//! nothing is computed from what the views say.

use maud::{Markup, html};
use serde_json::{Value, json};

use super::{Body, Ctx, Failure, Req};
use crate::access::encode_component;
use crate::components::{Head, Tone, badge, card, empty, kv, modal, name, slot};
use crate::page::{ViewName, Want};

pub const VIEWS: &[ViewName] = &[ViewName::Records, ViewName::Record, ViewName::Check];

/// How many records a page of the table holds (`records list --limit 50`).
pub const PAGE: usize = 50;
/// How many of them "Recent Requests" lists.
pub const RECENT: usize = 10;

pub const SUBTITLE: &str = "Newest first, 50 at a time · same as nullrouter records list --limit 50";
pub const EMPTY: &str = "No request records yet.";
pub const NO_OLDER: &str = "No older request records.";

pub fn wants(req: &Req) -> Vec<Want> {
    let mut wants = vec![Want::new(ViewName::Records, json!({"limit": PAGE, "before": req.get("before")}))];
    if let Some(id) = &req.window {
        wants.push(Want::new(ViewName::Record, json!({"id": id})));
    }
    wants.push(Want::new(ViewName::Check, json!({})));
    wants
}

pub fn body(ctx: &Ctx<'_>) -> Result<Body, Failure> {
    let records = arr(ctx.json(ViewName::Records));
    let content = html! {
        (overview(ctx, records))
        (requests(ctx, records))
    };
    // Only when the record was read: a window whose id doesn't exist is drawn by the frame.
    let window = ctx.page.view(ViewName::Record).map(|v| window(ctx, &v.json));
    Ok(Body { content, side: None, window })
}

// ---------------------------------------------------------------------------------------------
// The words `records` uses

fn arr(v: &Value) -> &[Value] {
    v.as_array().map_or(&[][..], |a| a.as_slice())
}

/// A string a client or a provider chose, without control characters (as `records` scrubs it).
fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

/// A string field, or `-`.
fn text(v: &Value) -> String {
    v.as_str().map_or_else(|| "-".to_owned(), clean)
}

/// `1234` → `1 234`, as `records` groups.
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

fn grouped_comma(n: u64) -> String {
    grouped(n).replace(' ', ",")
}

/// `412.3 ms`, or `-` when the record holds no time.
fn ms(v: &Value) -> String {
    let tenths = |f: f64| (f * 10.0).round() as u64;
    v.as_f64().map_or_else(|| "-".into(), |f| format!("{}.{} ms", grouped(tenths(f) / 10), tenths(f) % 10))
}

/// A token count, or `not reported`.
fn count(v: &Value) -> String {
    v.as_u64().map_or_else(|| "not reported".into(), grouped)
}

/// `1234.5` → `1.2k`, `5_600_000` → `5.6M`.
fn si(x: f64) -> String {
    let a = x.abs();
    if a >= 1e6 {
        format!("{:.1}M", x / 1e6)
    } else if a >= 1e3 {
        format!("{:.1}k", x / 1e3)
    } else {
        format!("{x:.0}")
    }
}

/// A deficit with its sign: `+91.2k`, `-250`, `0`.
fn deficit(n: i64) -> String {
    match n {
        0 => "0".into(),
        n if n > 0 => format!("+{}", si(n as f64)),
        n => si(n as f64),
    }
}

/// `provider/account` or just `provider`.
fn who(v: &Value) -> String {
    match v["account"].as_str() {
        Some(a) => format!("{}/{}", text(&v["provider"]), clean(a)),
        None => text(&v["provider"]),
    }
}

/// A record's outcome as `records list` words it: `succeeded`, `in progress`, `interrupted`.
fn result_words(r: &Value) -> String {
    text(&r["outcome"]).replace('_', " ")
}

fn result_tone(r: &Value) -> Tone {
    match r["outcome"].as_str() {
        Some("succeeded") => Tone::Success,
        Some("failed" | "refused") => Tone::Error,
        Some("in_progress") => Tone::Info,
        Some("cancelled" | "interrupted") => Tone::Warning,
        _ => Tone::Default,
    }
}

/// An attempt's outcome as `records show` words it.
fn attempt_outcome(a: &Value) -> String {
    let o = &a["outcome"];
    match o["state"].as_str() {
        Some("ok") => "ok".into(),
        Some("failed") => {
            let status = o["status"].as_u64().map_or_else(String::new, |s| format!("{s} "));
            format!("{status}{}: {}", text(&o["class"]).replace('_', " "), text(&o["reason"]))
        }
        Some("skipped") => format!("skipped: {}", text(&o["reason"])),
        Some("cancelled") => "cancelled".into(),
        _ => "in progress".into(),
    }
}

/// `cold_by_deficit (rank 0)` for an attempt a placement chose.
fn placed(a: &Value) -> Option<String> {
    let p = &a["placement"];
    Some(format!("{} (rank {})", p["reason"].as_str().map(clean)?, p["rank"].as_u64()?))
}

/// `in 18,210 · out 512 · cache w 18,100`, for the counts the provider reported.
fn attempt_usage(u: &Value) -> String {
    [("in", "input"), ("out", "output"), ("cache r", "cache_read"), ("cache w", "cache_write")]
        .iter()
        .filter_map(|(label, key)| u[*key].as_u64().map(|n| format!("{label} {}", grouped_comma(n))))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The attempt that served the record, else the last one a placement chose: the table's "Why".
fn why(r: &Value) -> String {
    let attempts = arr(&r["attempts"]);
    let by = attempts
        .iter()
        .rev()
        .find(|a| a["outcome"]["state"] == "ok")
        .or_else(|| attempts.iter().rev().find(|a| a["placement"]["reason"].is_string()));
    by.and_then(|a| a["placement"]["reason"].as_str()).map_or_else(|| "-".to_owned(), clean)
}

/// `in 1 204 out 388`, or `not reported`.
fn tokens(r: &Value) -> String {
    let u = &r["usage"];
    if u.is_null() { "not reported".into() } else { format!("in {} out {}", count(&u["input"]), count(&u["output"])) }
}

fn record_href(req: &Req, id: &str) -> String {
    req.href(&format!("/usage/records/{}", encode_component(id)), &[], &[])
}

// ---------------------------------------------------------------------------------------------
// The overview: three slots and Recent Requests

fn overview(ctx: &Ctx<'_>, records: &[Value]) -> Markup {
    html! {
        div class="usage-overview" {
            div class="usage-overview__period" { (slot("Period filter")) }
            div class="usage-overview__stats" { (slot("Requests, input, cached, output, Est. Cost")) }
            div class="usage-overview__pair" {
                div class="usage-overview__graph" { (slot("Topology graph")) }
                (recent(ctx, records))
            }
        }
    }
}

/// "Recent Requests": the newest [`RECENT`] of the table's own read.
fn recent(ctx: &Ctx<'_>, records: &[Value]) -> Markup {
    let rows = records.iter().take(RECENT);
    let body = html! {
        @if records.is_empty() {
            p class="usage-recent__none" { (EMPTY) }
        } @else {
            table class="usage-recent__table" {
                thead { tr { th {} th { "Model" } th { "Tokens" } th { "When" } } }
                tbody {
                    @for r in rows {
                        @let id = r["id"].as_str().unwrap_or_default();
                        tr class="usage-recent__row" {
                            td { span class=(dot(r)) title=(result_words(r)) {} }
                            td { a href=(record_href(ctx.req, id)) { (text(&r["target"])) } }
                            td { (tokens(r)) }
                            td { (ctx.time(r["arrived"].as_str().unwrap_or_default())) }
                        }
                    }
                }
            }
        }
    };
    html! { div class="usage-recent" { (card(Some(Head::new("schedule", "Recent Requests")), body)) } }
}

fn dot(r: &Value) -> &'static str {
    match r["outcome"].as_str() {
        Some("succeeded") => "usage-recent__dot usage-recent__dot--ok",
        Some("failed" | "refused") => "usage-recent__dot usage-recent__dot--bad",
        _ => "usage-recent__dot usage-recent__dot--open",
    }
}

// ---------------------------------------------------------------------------------------------
// The Requests table

fn requests(ctx: &Ctx<'_>, records: &[Value]) -> Markup {
    let req = ctx.req;
    let head = Head::new("history", "Requests").subtitle(SUBTITLE);
    // A full page may have more behind it; `records list --before <last id>` is the next one.
    let older = (records.len() >= PAGE)
        .then(|| records.last().and_then(|r| r["id"].as_str()))
        .flatten()
        .map(|id| req.href("/usage", &["before"], &[("before", id)]));
    let body = html! {
        @if records.is_empty() {
            (empty("history", if req.get("before").is_some() { NO_OLDER } else { EMPTY }, ""))
        } @else {
            table class="usage-table" {
                thead {
                    tr {
                        th { "When" } th { "Agent" } th { "Model → placed on" } th { "Why" }
                        th class="usage-table__num" { "TTFT" } th class="usage-table__num" { "Total" } th { "Result" }
                    }
                }
                tbody { @for r in records { (row(ctx, r)) } }
            }
        }
        @if let Some(href) = older {
            p class="usage-table__older" { a href=(href) { "Older" } }
        }
    };
    card(Some(head), body)
}

fn row(ctx: &Ctx<'_>, r: &Value) -> Markup {
    let id = r["id"].as_str().unwrap_or_default();
    let placed_on = if r["served_by"].is_null() { "-".to_owned() } else { who(&r["served_by"]) };
    html! {
        tr class="usage-row" {
            td { a class="usage-row__link" href=(record_href(ctx.req, id)) { (ctx.time(r["arrived"].as_str().unwrap_or_default())) } }
            td class="usage-row__name" { (name(&text(&r["agent"]["key"]))) }
            td class="usage-row__name" { b { (name(&text(&r["target"]))) } " → " (name(&placed_on)) }
            td { (why(r)) }
            td class="usage-table__num" { (ms(&r["ttft_ms"])) }
            td class="usage-table__num" { (ms(&r["total_ms"])) }
            td { (badge(result_tone(r), &result_words(r))) }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The window: what `records show` shows

fn window(ctx: &Ctx<'_>, r: &Value) -> Markup {
    let key = r["agent"]["key"].as_str().unwrap_or_default();
    let agent = ctx.extra(ViewName::Record)["key_names"][key].as_str().unwrap_or(key);
    let body = html! {
        (kv("When", ctx.time(r["arrived"].as_str().unwrap_or_default())))
        (kv("Result", badge(result_tone(r), &result_words(r))))
        @if !r["agent"].is_null() {
            @match r["agent"]["session"].as_str() {
                Some(session) => { (kv("Agent", html! { (clean(agent)) " / session " (clean(session)) })) }
                None => { (kv("Agent", html! { (clean(agent)) })) }
            }
        }
        (kv("Door", html! { (text(&r["style"])) "  " (text(&r["op"])) "  " (text(&r["model_type"])) }))
        @if r["target"].is_string() {
            (kv("Target", html! { (text(&r["target"])) @if !r["unified_model"].is_null() { " (unified)" } }))
        }
        @if !r["served_by"].is_null() {
            (kv("Served by", html! { (who(&r["served_by"])) "  " (text(&r["served_by"]["model"])) }))
        }
        (kv("TTFT", html! { (ms(&r["ttft_ms"])) }))
        (kv("Total", html! { (ms(&r["total_ms"])) }))
        (kv("Usage", usage(&r["usage"])))
        (kv("Break", html! { (break_words(&r["break_handling"])) }))
        @if let Some(job) = r["job"].as_object() {
            (kv("Job", html! {
                (text(job.get("nullrouter_job_id").unwrap_or(&Value::Null))) "  upstream "
                (text(job.get("upstream_id").unwrap_or(&Value::Null)))
            }))
        }
        (decision(ctx, &r["decision"]))
        h3 class="attempt__title" { "Attempts" }
        @for a in arr(&r["attempts"]) { (attempt(a)) }
    };
    modal(&text(&r["id"]), &ctx.req.close_href(), body)
}

fn usage(u: &Value) -> Markup {
    if u.is_null() {
        return html! { "not reported" };
    }
    html! {
        "input " (count(&u["input"])) "  output " (count(&u["output"])) "  cache-read " (count(&u["cache_read"]))
        "  cache-write " (count(&u["cache_write"])) @if u["estimated"] == true { "  (estimated)" }
    }
}

fn break_words(b: &Value) -> String {
    match b["kind"].as_str() {
        Some("error_event") => format!("error event: {}", text(&b["reason"])),
        Some(k) => k.replace('_', " "),
        None => "none".into(),
    }
}

/// The stay-warm decision and its candidates, as `records show` prints them.
fn decision(ctx: &Ctx<'_>, d: &Value) -> Markup {
    if d.is_null() {
        return html! {};
    }
    let warm = &d["warm"];
    let named = |w: &Value| format!("{}/{}", text(&w["provider"]), text(&w["account"]));
    let prefix = |w: &Value| {
        format!(
            "prefix {} · idle {:.0} s",
            si(w["prefix_tokens"].as_f64().unwrap_or(0.0)),
            w["idle_s"].as_f64().unwrap_or(0.0)
        )
    };
    let rows = arr(&d["candidates"]);
    let order: Vec<usize> = arr(&d["order"]).iter().filter_map(|i| i.as_u64().map(|i| i as usize)).collect();
    // The attempt order first, then the candidates it left out.
    let mut seq: Vec<(Option<usize>, &Value)> =
        order.iter().enumerate().filter_map(|(rank, i)| rows.get(*i).map(|r| (Some(rank), r))).collect();
    seq.extend(rows.iter().enumerate().filter(|(i, _)| !order.contains(i)).map(|(_, r)| (None, r)));
    html! {
        @if d["kind"] == "warm" && !warm.is_null() {
            (kv("Decision", html! { "warm on " (named(warm)) " · " (prefix(warm)) " · stayed" }))
        } @else {
            (kv("Decision", html! {
                (text(&d["kind"])) " · size " (si(d["size_tokens"].as_f64().unwrap_or(0.0)))
                " · amortization window from " (ctx.time(d["amortization_window"]["start"].as_str().unwrap_or_default()))
                ", " (text(&d["amortization_window"]["length"])) " long"
            }))
            @if !warm.is_null() {
                (kv("Warm", html! {
                    (named(warm)) " · " (prefix(warm)) " · moved: "
                    (warm["moved_because"].as_str().map_or_else(|| "not usable".to_owned(), clean)) " on " (named(warm))
                }))
            }
        }
        @if !seq.is_empty() {
            table class="usage-table usage-table--decision" {
                thead {
                    tr {
                        th { "#" } th { "account" } th { "tier" } th { "eligible" } th { "pace" } th { "share" }
                        th { "deficit" } th { "price" }
                    }
                }
                tbody {
                    @for (rank, c) in &seq {
                        tr {
                            td { (rank.map_or_else(|| "-".to_owned(), |n| n.to_string())) }
                            td { (who(c)) }
                            td { (text(&c["tier"])) }
                            td {
                                @match c["why_not"].as_str() {
                                    Some(why) => { "no: " (clean(why).replace('_', " ")) }
                                    None => { "yes" }
                                }
                            }
                            td { (c["pace"].as_f64().map_or_else(String::new, |p| format!("{p:.2}"))) }
                            td { (c["share"].as_f64().map_or_else(String::new, |x| format!("{:.0}%", x * 100.0))) }
                            td { (c["deficit_before"].as_i64().map_or_else(String::new, deficit)) }
                            td { (c["price_now"].as_f64().map_or_else(String::new, |p| format!("{p:.2}"))) }
                        }
                    }
                }
            }
        }
    }
}

/// One attempt: who it went to, why, how it ended and how long it took, and what it changed.
fn attempt(a: &Value) -> Markup {
    let (variant, tone) = match a["outcome"]["state"].as_str() {
        Some("ok") => ("ok", Tone::Success),
        Some("failed") => ("failed", Tone::Error),
        Some("skipped") => ("skipped", Tone::Warning),
        Some("cancelled") => ("cancelled", Tone::Default),
        _ => ("pending", Tone::Info),
    };
    let class = format!("attempt attempt--{variant}");
    let latency = match (a["started"].as_f64(), a["ended"].as_f64()) {
        (Some(from), Some(to)) => {
            let used = attempt_usage(&a["usage"]);
            let secs = format!("{:.2} s", (to - from) / 1000.0);
            Some(if used.is_empty() { secs } else { format!("{secs}  {used}") })
        }
        _ => None,
    };
    html! {
        div class=(class) {
            div class="attempt__head" {
                span class="attempt__n" { (a["n"].as_u64().map_or_else(|| "-".to_owned(), |n| n.to_string())) }
                span class="attempt__who" { (who(a)) }
                span class="attempt__model" { (text(&a["model"])) }
                @if let Some(p) = placed(a) { span class="attempt__placed" { (p) } }
                span class="attempt__outcome" { (badge(tone, &attempt_outcome(a))) }
            }
            @if let Some(l) = latency { div class="attempt__latency" { (l) } }
            @for d in arr(&a["dropped"]) {
                div class="attempt__change" { "dropped " (text(&d["path"])) ": " (text(&d["reason"])) }
            }
            @for f in arr(&a["forced"]) {
                div class="attempt__change" { "forced " (text(&f[0])) ": " (forced_value(&f[1])) }
            }
        }
    }
}

/// A forced parameter's value as written: a string bare, anything else as JSON.
fn forced_value(v: &Value) -> String {
    v.as_str().map_or_else(|| clean(&v.to_string()), clean)
}
