//! Quota Tracker (FR-033, contracts/dashboard-http.md "Quota Tracker"): one card per account with
//! what `accounts list --long`, `quota` and `routing` show for it, in the CLI's words.
//!
//! The cards follow `quota`'s list (so `?provider=…&account=…` shows exactly what
//! `quota <provider> <name>` shows); `accounts` and `routing` are joined to each by provider and
//! name. The figure formats below are the CLI's (`quota_text`, `routing_text`), reproduced from the
//! views' fields: the page computes no fact of its own (research R1).

use jiff::Timestamp;
use maud::{Markup, html};
use serde_json::{Value, json};

use super::{Body, Ctx, Failure, Req};
use crate::components::{card, command, empty, icon, kv, logo, meter, name, section_bar, status};
use crate::page::{ViewName, Want};

pub const VIEWS: &[ViewName] = &[ViewName::Accounts, ViewName::Quota, ViewName::Routing, ViewName::Check];

/// `accounts` is read whole (the filters list every account); `quota` is narrowed the way
/// `quota [provider [name]]` is; `routing` is read whole and joined per account.
pub fn wants(req: &Req) -> Vec<Want> {
    vec![
        Want::new(ViewName::Accounts, json!({})),
        Want::new(ViewName::Quota, json!({"provider": req.get("provider"), "name": req.get("account")})),
        Want::new(ViewName::Routing, json!({"target": null})),
        Want::new(ViewName::Check, json!({})),
    ]
}

pub fn body(ctx: &Ctx<'_>) -> Result<Body, Failure> {
    let accounts = ctx.json(ViewName::Accounts);
    let quota = ctx.json(ViewName::Quota);
    let routing = ctx.json(ViewName::Routing);
    let rows = quota.as_array().map_or(&[][..], Vec::as_slice);
    let default_len = routing["amortization"]["length"].as_str().unwrap_or_default();
    let cards: Vec<Markup> = rows
        .iter()
        .map(|q| {
            let provider = q["provider"].as_str().unwrap_or_default();
            let account = q["name"].as_str().unwrap_or_default();
            let a = find_account(accounts, provider, account);
            account_card(ctx, q, a, &routes_of(routing, provider, account), default_len)
        })
        .collect();
    let bar = section_bar(
        "Accounts",
        html! {
            @if let Some(w) = amortization(ctx, routing) { (w) }
            (filters(ctx.req, accounts))
        },
    );
    let content = if cards.is_empty() {
        html! {
            (bar)
            (card(None, empty("data_usage", "No accounts.", "Run `nullrouter accounts add <provider> <name>`.")))
        }
    } else {
        html! {
            (bar)
            div class="quota-grid" { @for c in &cards { (c) } }
            p class="hint" {
                "Showing " (cards.len()) " accounts · polled quota is read from the provider, estimated quota is counted from your requests."
            }
        }
    };
    Ok(Body::new(content))
}

/// The `accounts` row of `provider/name`; `Null` when there is none.
fn find_account<'a>(accounts: &'a Value, provider: &str, account: &str) -> &'a Value {
    accounts
        .as_array()
        .into_iter()
        .flatten()
        .find(|r| r["provider"] == provider && r["name"] == account)
        .unwrap_or(&Value::Null)
}

/// Every target the account serves, as `(target, the account's row in it)`.
fn routes_of<'a>(routing: &'a Value, provider: &str, account: &str) -> Vec<(&'a Value, &'a Value)> {
    routing["targets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let row = t["accounts"].as_array()?.iter().find(|r| r["provider"] == provider && r["account"] == account)?;
            Some((t, row))
        })
        .collect()
}

/// The amortization window `routing` shows, in the header.
fn amortization(ctx: &Ctx<'_>, routing: &Value) -> Option<Markup> {
    let w = &routing["amortization"];
    let length = w["length"].as_str()?;
    Some(kv(
        "Amortization window",
        html! {
            (length)
            @if let Some(start) = w["start"].as_str() { " from " (ctx.time(start)) }
        },
    ))
}

/// Provider and account selects, "Expiring first", and the submit button: one `GET` form.
fn filters(req: &Req, accounts: &Value) -> Markup {
    let rows = accounts.as_array().map_or(&[][..], Vec::as_slice);
    let (current_provider, current_account) = (req.get("provider"), req.get("account"));
    let mut providers: Vec<&str> = rows.iter().filter_map(|r| r["provider"].as_str()).collect();
    providers.sort_unstable();
    providers.dedup();
    let mut names: Vec<&str> = rows
        .iter()
        .filter(|r| current_provider.is_none_or(|p| r["provider"] == p))
        .filter_map(|r| r["name"].as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    html! {
        form class="quota-filters" method="get" action="/quota" {
            select class="quota-filters__select" name="provider" aria-label="Provider" {
                option value="" { "All providers" }
                @for p in &providers { option value=(p) selected[current_provider == Some(*p)] { (p) } }
            }
            select class="quota-filters__select" name="account" aria-label="Account" {
                option value="" { "All accounts" }
                @for n in &names { option value=(n) selected[current_account == Some(*n)] { (n) } }
            }
            label class="quota-filters__toggle" {
                input type="checkbox" name="order" value="expiring" checked[req.get("order") == Some("expiring")];
                (icon("hourglass_top"))
                "Expiring first"
            }
            @if req.notices_open() { input type="hidden" name="notices" value=""; }
            button class="button button--secondary" type="submit" { "Show" }
        }
    }
}

/// One account: who it is and how it is signed in, its quota, and what it does in each target.
fn account_card(ctx: &Ctx<'_>, q: &Value, a: &Value, routes: &[(&Value, &Value)], default_len: &str) -> Markup {
    let provider = q["provider"].as_str().unwrap_or_default();
    let account = q["name"].as_str().unwrap_or_default();
    let state = a["state"].as_str().unwrap_or_default();
    let class = match state {
        "disabled" => "quota-card quota-card--off",
        "needs_sign_in" | "refused" => "quota-card quota-card--attention",
        _ => "quota-card",
    };
    let priority = a["priority"].as_f64().map_or_else(|| "-".to_owned(), number);
    let order = a["order"].as_i64().map_or_else(|| "-".to_owned(), |o| o.to_string());
    let kind = a["kind"].as_str().or_else(|| q["kind"].as_str()).unwrap_or("-");
    let body = html! {
        div class="quota-card__head" {
            (logo(ctx.logos.href(provider), provider))
            div class="quota-card__titles" {
                h3 class="quota-card__provider" { (name(provider)) }
                p class="quota-card__account" { (name(account)) }
                p class="quota-card__meta" { "priority " (priority) " · order " (order) " · " (kind) }
            }
            div class="quota-card__state" { (sign_in_status(ctx, a)) }
        }
        div class="quota-card__body" {
            @if state == "needs_sign_in" {
                p class="quota-card__alert" {
                    "Sign in again. In the CLI: "
                    (command(&format!("nullrouter accounts signin {provider} {account}")))
                }
            }
            (identity(ctx, a))
            (quota_section(ctx, q, routes))
            (targets_section(default_len, routes))
        }
    };
    html! { div class=(class) { (card(None, body)) } }
}

/// The sign-in status badge. A state that needs signing in again says since when, as an instant,
/// and why; the others are `accounts list`'s own text (`active`, `cooling <model> 12 s`, ...).
fn sign_in_status(ctx: &Ctx<'_>, a: &Value) -> Markup {
    let state = a["state"].as_str().unwrap_or_default();
    match state {
        "needs_sign_in" | "refused" => {
            let label = if state == "refused" { "refused by provider" } else { "needs sign-in" };
            html! {
                (status(label))
                @if let Some(since) = a["state_since"].as_str() { span class="quota-card__since" { "since " (ctx.time(since)) } }
                @if let Some(why) = a["state_reason"].as_str().filter(|w| !w.is_empty()) {
                    span class="quota-card__since" { "(" (why) ")" }
                }
            }
        }
        _ => status(a["state_text"].as_str().unwrap_or(state)),
    }
}

/// What `accounts list --long` adds for a sign-in account.
fn identity(ctx: &Ctx<'_>, a: &Value) -> Markup {
    html! {
        @if a["kind"] == "signin" {
            div class="quota-card__identity" {
                @if let Some(e) = a["email"].as_str() { (kv("Email", html! { (e) })) }
                @if let Some(t) = a["tier"].as_str() { (kv("Tier", html! { (t) })) }
                @if let Some(t) = a["expires_at"].as_str() { (kv("Token expires", ctx.time(t))) }
                @if let Some(t) = a["last_refresh_at"].as_str() { (kv("Last refreshed", ctx.time(t))) }
            }
        }
    }
}

/// One quota window as a row.
struct Row<'a> {
    name: &'a str,
    figure: String,
    /// Percent of the window left, for the bar; `None` draws no bar.
    left: Option<f64>,
    resets: Option<&'a str>,
}

/// A window of the latest poll, as `quota` words it.
fn polled_row(w: &Value) -> Row<'_> {
    Row {
        name: w["name"].as_str().unwrap_or("?"),
        figure: window_value(w),
        left: left_percent(w),
        resets: w["resets_at"].as_str(),
    }
}

/// The windows `routing` counts for an account that has no poll to show (estimated, or waiting
/// for its first one), as `routing` words them.
fn estimated_rows(route: &Value) -> Vec<Row<'_>> {
    route["windows"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|w| {
            let unit = if w["unit"] == "requests" { "req" } else { "wtok" };
            let left = w["remaining_now"].as_f64().unwrap_or(0.0);
            let capacity = w["capacity"].as_f64().unwrap_or(0.0);
            let floor = number((w["reserve"].as_f64().unwrap_or(0.0) * 1000.0).round() / 10.0);
            Row {
                name: w["name"].as_str().unwrap_or("?"),
                figure: format!("{}/{} {unit} · floor {floor}%", si(left), si(capacity)),
                left: (capacity > 0.0).then_some(left / capacity * 100.0),
                resets: w["resets_at"].as_str(),
            }
        })
        .collect()
}

/// The source, the last poll, and each window with its bar. A pay-as-you-go account has no
/// windows and no bar.
fn quota_section(ctx: &Ctx<'_>, q: &Value, routes: &[(&Value, &Value)]) -> Markup {
    let first = routes.first().map(|(_, r)| *r);
    let reported = q["reported"] == true;
    let source = match first {
        Some(r) => r["source"].as_str(),
        None if reported => Some("polled"),
        None => None,
    };
    let pending = first.map_or(reported && q["latest"].is_null(), |r| r["pending_first_poll"] == true);
    let stale = first.is_some_and(|r| r["stale"] == true);
    let payg = source == Some("pay-as-you-go");

    let mut rows: Vec<Row<'_>> = q["latest"]["windows"].as_array().into_iter().flatten().map(polled_row).collect();
    if let Some(r) = first.filter(|_| rows.is_empty() && !payg) {
        rows = estimated_rows(r);
    }
    if ctx.req.get("order") == Some("expiring") {
        // Soonest reset first; a window with no reset time last.
        rows.sort_by_key(|r| {
            let at = r.resets.and_then(|t| t.parse::<Timestamp>().ok());
            (at.is_none(), at)
        });
    }
    html! {
        div class="quota-card__section" {
            div class="quota-card__source" {
                @if let Some(s) = source { (status(s)) } @else { span class="quota-card__note" { "quota not reported" } }
                @if pending { (status("pending first poll")) }
                @if stale { (status("stale")) }
                @if reported {
                    @if let Some(at) = q["latest"]["at"].as_str() {
                        span class="quota-card__poll" {
                            "last poll " (ctx.time(at)) " · every " (every(q["interval_s"].as_u64().unwrap_or(600)))
                        }
                    }
                    @if let Some(at) = q["last_failure"]["at"].as_str() {
                        span class="quota-card__poll" {
                            "last poll failed " (ctx.time(at)) " (" (q["last_failure"]["error"]["summary"].as_str().unwrap_or("failed")) ")"
                        }
                    }
                }
            }
            @if !payg { @for r in &rows { (window_row(ctx, r)) } }
        }
    }
}

fn window_row(ctx: &Ctx<'_>, r: &Row<'_>) -> Markup {
    html! {
        div class="quota-card__window" {
            span class="quota-card__window-name" { (name(r.name)) }
            div class="quota-card__window-main" {
                @if let Some(p) = r.left { (meter(p)) }
                span class="quota-card__window-figure" { (r.figure) }
            }
            @if let Some(t) = r.resets { span class="quota-card__window-reset" { "resets " (ctx.time(t)) } }
        }
    }
}

/// Each target the account serves with its pace, share and deficit, as `routing` shows them.
fn targets_section(default_len: &str, routes: &[(&Value, &Value)]) -> Markup {
    html! {
        @if !routes.is_empty() {
            div class="quota-card__targets" {
                @for (t, r) in routes { (target_row(default_len, t, r)) }
            }
        }
    }
}

fn target_row(default_len: &str, t: &Value, r: &Value) -> Markup {
    let pace = r["pace"].as_f64().map_or_else(|| "—".to_owned(), |p| format!("{p:.2}"));
    let share = r["share"].as_f64().map_or_else(|| "—".to_owned(), |s| format!("{:.0}%", s * 100.0));
    let owed = deficit(r["deficit"].as_i64().unwrap_or(0));
    let mut notes: Vec<String> = Vec::new();
    if r["tier"] == "payg" {
        notes.push(match r["price_now"].as_f64() {
            Some(p) => format!("price now {p:.2}/Mtok in"),
            None => "price not declared".to_owned(),
        });
    }
    match r["why_not"].as_str() {
        Some("priority_zero") => notes.push("cold work off (priority 0)".to_owned()),
        Some(why) => notes.push(format!("not an option: {}", why.replace('_', " "))),
        None => {}
    }
    if let Some(len) = t["amortization_window"]["length"].as_str().filter(|l| *l != default_len) {
        notes.push(format!("amortization {len}"));
    }
    html! {
        div class="quota-card__target" {
            span class="quota-card__target-name" { (name(t["target"].as_str().unwrap_or("?"))) }
            (kv("pace", html! { (pace) }))
            (kv("share", html! { (share) }))
            (kv("deficit", html! { (owed) }))
            @for n in &notes { span class="quota-card__note" { (n) } }
        }
    }
}

// The CLI's figure formats, reproduced (the dashboard cannot depend on the CLI crate).

/// `10 min`, `1 h`, `90 s` (`quota_text::every`).
fn every(secs: u64) -> String {
    if secs != 0 && secs.is_multiple_of(3600) {
        format!("{} h", secs / 3600)
    } else if secs != 0 && secs.is_multiple_of(60) {
        format!("{} min", secs / 60)
    } else {
        format!("{secs} s")
    }
}

/// `1,240`, `87.5`, `0` (`quota_text::num`).
fn num(x: f64) -> String {
    let neg = x < 0.0;
    let x = (x.abs() * 10.0).round() / 10.0;
    let whole = x.trunc() as u64;
    let mut digits = whole.to_string();
    let mut grouped = String::new();
    while digits.len() > 3 {
        let tail = digits.split_off(digits.len() - 3);
        grouped = format!(",{tail}{grouped}");
    }
    let frac = ((x - x.trunc()) * 10.0).round() as u64;
    let sign = if neg { "-" } else { "" };
    if frac == 0 { format!("{sign}{digits}{grouped}") } else { format!("{sign}{digits}{grouped}.{frac}") }
}

/// What a polled window is worth (`quota_text::window_value`).
fn window_value(w: &Value) -> String {
    let f = |k: &str| w[k].as_f64();
    let unit = w["unit"].as_str().unwrap_or("percent");
    if unit == "percent" {
        let used = f("used");
        let left = f("remaining").or(used.map(|u| (100.0 - u).max(0.0))).unwrap_or(0.0);
        return match used {
            Some(u) if u > 100.0 => format!("{}% left ({}% used)", num(left), num(u)),
            _ => format!("{}% left", num(left)),
        };
    }
    match (f("used"), f("limit"), f("remaining")) {
        (Some(u), Some(l), _) => format!("{} / {} {unit} used", num(u), num(l)),
        (Some(u), None, _) => format!("{} {unit} used", num(u)),
        (None, _, Some(r)) => format!("{} {unit} left", num(r)),
        _ => "not reported".into(),
    }
}

/// The share of a polled window left, for its bar; `None` when the window has no limit to measure
/// against.
fn left_percent(w: &Value) -> Option<f64> {
    let f = |k: &str| w[k].as_f64();
    if w["unit"].as_str().unwrap_or("percent") == "percent" {
        return Some(f("remaining").or(f("used").map(|u| (100.0 - u).max(0.0))).unwrap_or(0.0));
    }
    let limit = f("limit").filter(|l| *l > 0.0)?;
    let left = f("remaining").or(f("used").map(|u| (limit - u).max(0.0)))?;
    Some(left / limit * 100.0)
}

/// `1234.5` → `1.2k`, `5_600_000` → `5.6M`, `250` → `250` (`routing_text::si`).
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

/// A deficit with its sign: `+91.2k`, `-250`, `0` (`routing_text::deficit`).
fn deficit(n: i64) -> String {
    match n {
        0 => "0".into(),
        n if n > 0 => format!("+{}", si(n as f64)),
        n => si(n as f64),
    }
}

/// `1` for `1.0`, `0.5` for `0.5` (`routing_text::number`).
fn number(x: f64) -> String {
    if x.fract() == 0.0 { format!("{x:.0}") } else { format!("{x}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clis_figures() {
        assert_eq!((num(1240.0), num(87.5), num(0.0)), ("1,240".into(), "87.5".into(), "0".into()));
        assert_eq!((every(600), every(3600), every(90)), ("10 min".into(), "1 h".into(), "90 s".into()));
        assert_eq!((si(5.6e6), si(1234.0), si(250.0)), ("5.6M".into(), "1.2k".into(), "250".into()));
        assert_eq!((deficit(-91_200), deficit(250), deficit(0)), ("-91.2k".into(), "+250".into(), "0".into()));
        assert_eq!((number(1.0), number(0.5)), ("1".into(), "0.5".into()));
    }

    #[test]
    fn a_polled_window_reads_as_quota_says() {
        let pct = json!({"name": "5-hour", "unit": "percent", "used": 38.0, "remaining": 62.0});
        assert_eq!(window_value(&pct), "62% left");
        assert_eq!(left_percent(&pct), Some(62.0));
        let credits =
            json!({"name": "prepaid", "unit": "credits", "used": 1240.0, "limit": 5000.0, "remaining": 3760.0});
        assert_eq!(window_value(&credits), "1,240 / 5,000 credits used");
        assert!((left_percent(&credits).unwrap() - 75.2).abs() < 1e-9);
        let open = json!({"name": "rolling", "unit": "requests", "used": 3.0});
        assert_eq!((window_value(&open), left_percent(&open)), ("3 requests used".into(), None));
    }
}
