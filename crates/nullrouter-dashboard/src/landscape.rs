//! The traffic landscape on Endpoint & Key (spec 010 US3, research R9): agents on the left, the
//! router in the centre, providers on the right, one pipe per hop with a half-dial gauge on it.
//! Inline SVG drawn on the server: no script, no animation, no external reference. Every number
//! on a gauge is printed as text beside it, so neither a zone nor the needle carries a fact
//! alone (FR-027), and the `:hover`/`:focus-within` card only repeats what the page prints.
//!
//! The nodes are every unrevoked key and every provider with an account, plus any revoked key or
//! other provider with a row in the latency view (FR-020). Colours come from `agent_colour`.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use maud::{Markup, html};
use serde_json::Value;

use crate::components::agent_colour;
use crate::time::local_text;

/// Router overhead dial: full scale, in ms.
pub const OVERHEAD_FULL_MS: f64 = 40.0;
/// Provider time-to-first-token dial: full scale, in ms.
pub const TTFT_FULL_MS: f64 = 3000.0;
pub const EMPTY: &str = "No requests in the last 24 hours";
/// Longest name drawn in a node before it is shortened (the full text stays in the card).
pub const SHOWN_CHARS: usize = 22;

const WIDTH: f64 = 900.0;
const NODE_W: f64 = 210.0;
const NODE_H: f64 = 40.0;
const AGENT_X: f64 = 10.0;
const PROVIDER_X: f64 = 680.0;
const ROUTER_W: f64 = 110.0;
const ROUTER_H: f64 = 44.0;
const PITCH: f64 = 78.0;
const DIAL_R: f64 = 14.0;

/// What the landscape reads: three views and the page's clock.
pub struct Inputs<'a> {
    pub keys: &'a Value,
    pub accounts: &'a Value,
    pub latency: &'a Value,
    pub tz: &'a TimeZone,
    pub as_of: Timestamp,
}

/// "Agent traffic · last 24 h · as of HH:MM:SS".
pub fn heading(as_of: Timestamp, tz: &TimeZone) -> String {
    format!("Agent traffic · last 24 h · as of {}", as_of.to_zoned(tz.clone()).strftime("%H:%M:%S"))
}

/// Under a second in ms, otherwise seconds with one decimal (as `nullrouter latency` prints).
pub fn ms(v: f64) -> String {
    if v < 1000.0 { format!("{} ms", v.round() as u64) } else { format!("{:.1} s", v / 1000.0) }
}

/// `p50 / p95`, or `none` when no request had a value.
pub fn pair(v: &Value) -> String {
    match (v["p50"].as_f64(), v["p95"].as_f64()) {
        (Some(a), Some(b)) => format!("{} / {}", ms(a), ms(b)),
        _ => "none".to_owned(),
    }
}

fn short(text: &str) -> String {
    if text.chars().count() <= SHOWN_CHARS {
        text.to_owned()
    } else {
        let head: String = text.chars().take(SHOWN_CHARS - 1).collect();
        format!("{head}…")
    }
}

struct Agent<'a> {
    id: &'a str,
    name: String,
    tag: Option<&'a str>,
    revoked: bool,
    colour: usize,
    row: Option<&'a Value>,
}

struct Provider<'a> {
    id: &'a str,
    row: Option<&'a Value>,
}

fn rows(v: &Value) -> &[Value] {
    v.as_array().map_or(&[][..], Vec::as_slice)
}

fn agents<'a>(i: &Inputs<'a>) -> Vec<Agent<'a>> {
    let row_of = |id: &str| rows(&i.latency["agents"]).iter().find(|r| r["id"] == id);
    let mut out: Vec<Agent<'a>> = rows(i.keys)
        .iter()
        .enumerate()
        .filter_map(|(n, k)| {
            let id = k["id"].as_str()?;
            let row = row_of(id);
            let revoked = k["revoked"].is_string();
            (!revoked || row.is_some()).then(|| Agent {
                id,
                name: k["name"].as_str().unwrap_or(id).to_owned(),
                tag: k["harness"].as_str(),
                revoked,
                colour: n,
                row,
            })
        })
        .collect();
    // A latency row for a key the home no longer lists still shows, by its id.
    let known = rows(i.keys).len();
    for (n, r) in rows(&i.latency["agents"]).iter().enumerate() {
        let Some(id) = r["id"].as_str() else { continue };
        if !out.iter().any(|a| a.id == id) {
            let name = r["name"].as_str().unwrap_or(id).to_owned();
            out.push(Agent { id, name, tag: None, revoked: true, colour: known + n, row: Some(r) });
        }
    }
    out
}

fn providers<'a>(i: &Inputs<'a>) -> Vec<Provider<'a>> {
    let row_of = |id: &str| rows(&i.latency["providers"]).iter().find(|r| r["id"] == id);
    let mut ids: Vec<&str> = Vec::new();
    for a in rows(i.accounts) {
        if let Some(p) = a["provider"].as_str().filter(|p| !ids.contains(p)) {
            ids.push(p);
        }
    }
    for r in rows(&i.latency["providers"]) {
        if let Some(p) = r["id"].as_str().filter(|p| !ids.contains(p)) {
            ids.push(p);
        }
    }
    ids.into_iter().map(|id| Provider { id, row: row_of(id) }).collect()
}

/// A column of `n` nodes centred on `mid`: the y of the node centre.
fn column_y(index: usize, n: usize, mid: f64) -> f64 {
    mid + (index as f64 - (n as f64 - 1.0) / 2.0) * PITCH
}

fn point(cx: f64, cy: f64, r: f64, deg: f64) -> (f64, f64) {
    (cx + r * deg.to_radians().cos(), cy - r * deg.to_radians().sin())
}

fn arc(cx: f64, cy: f64, from: f64, to: f64, class: &str) -> Markup {
    let (x0, y0) = point(cx, cy, DIAL_R, from);
    let (x1, y1) = point(cx, cy, DIAL_R, to);
    html! { path class=(format!("gauge__zone gauge__zone--{class}")) d=(format!("M{x0:.1},{y0:.1} A{DIAL_R},{DIAL_R} 0 0 1 {x1:.1},{y1:.1}")) {} }
}

/// A half dial on a hop: green to half scale, amber to 80%, red above; the needle at the median
/// and pinned at full scale beyond it. The numbers are printed under it as text.
fn gauge(x: f64, y: f64, p50: Option<f64>, full: f64, pair_text: &str, requests: u64) -> Markup {
    let needle = p50.map(|v| point(x, y, DIAL_R - 2.0, 180.0 - 180.0 * (v / full).clamp(0.0, 1.0)));
    html! {
        g class="gauge" {
            (arc(x, y, 180.0, 90.0, "ok"))
            (arc(x, y, 90.0, 36.0, "warn"))
            (arc(x, y, 36.0, 0.0, "hot"))
            @if let Some((nx, ny)) = needle {
                line class="gauge__needle" x1=(format!("{x:.1}")) y1=(format!("{y:.1}")) x2=(format!("{nx:.1}")) y2=(format!("{ny:.1}")) {}
                circle class="gauge__hub" cx=(format!("{x:.1}")) cy=(format!("{y:.1}")) r="2" {}
            }
            text class="gauge__value" x=(format!("{x:.1}")) y=(format!("{:.1}", y + 13.0)) text-anchor="middle" { (pair_text) }
            text class="gauge__small" x=(format!("{x:.1}")) y=(format!("{:.1}", y + 24.0)) text-anchor="middle" { (requests) " req" }
        }
    }
}

fn pipe_path(x0: f64, y0: f64, x1: f64, y1: f64) -> String {
    let xm = (x0 + x1) / 2.0;
    format!("M{x0:.1},{y0:.1} C{xm:.1},{y0:.1} {xm:.1},{y1:.1} {x1:.1},{y1:.1}")
}

/// `resolved 14:01:58` or `failed 14:00:31 (503)`, with the date when it isn't the read's date;
/// `None` when the row has no last response.
fn last_text(last: &Value, i: &Inputs<'_>) -> Option<String> {
    let at = local_text(last["at"].as_str()?, i.tz)?;
    let today = i.as_of.to_zoned(i.tz.clone()).strftime("%Y-%m-%d").to_string();
    let when = match at.split_once(' ') {
        Some((date, clock)) if date == today => clock.to_owned(),
        _ => at,
    };
    let status = last["status"].as_u64().map_or_else(String::new, |s| format!(" ({s})"));
    Some(format!("{} {when}{status}", last["result"].as_str()?))
}

/// The hover card: the full name and every number the page prints for the node.
fn card(x: f64, y: f64, title: &str, lines: &[String]) -> Markup {
    let h = 18.0 + 13.0 * (lines.len() as f64 + 1.0);
    html! {
        g class="landscape__card" {
            rect x=(format!("{x:.1}")) y=(format!("{y:.1}")) width="250" height=(format!("{h:.0}")) rx="6" {}
            text class="landscape__name" x=(format!("{:.1}", x + 8.0)) y=(format!("{:.1}", y + 15.0)) { (title) }
            @for (n, l) in lines.iter().enumerate() {
                text class="landscape__text" x=(format!("{:.1}", x + 8.0)) y=(format!("{:.1}", y + 29.0 + 13.0 * n as f64)) { (l) }
            }
        }
    }
}

fn node(
    x: f64,
    y: f64,
    name: &str,
    sub: Option<&str>,
    dot: Option<usize>,
    revoked: bool,
    card_lines: &[String],
) -> Markup {
    let top = y - NODE_H / 2.0;
    let class = if revoked { "landscape__node landscape__node--revoked" } else { "landscape__node" };
    html! {
        g class=(class) tabindex="0" {
            title { (name) }
            rect x=(format!("{x:.1}")) y=(format!("{top:.1}")) width=(format!("{NODE_W:.0}")) height=(format!("{NODE_H:.0}")) rx="8" {}
            @if let Some(n) = dot {
                circle class=(format!("landscape__dot landscape__dot--{}", agent_colour(n))) cx=(format!("{:.1}", x + 14.0)) cy=(format!("{y:.1}")) r="5" {}
            }
            text class="landscape__name" x=(format!("{:.1}", x + if dot.is_some() { 28.0 } else { 12.0 })) y=(format!("{:.1}", if sub.is_some() { y - 3.0 } else { y + 4.0 })) { (short(name)) }
            @if let Some(sub) = sub {
                text class="landscape__tag" x=(format!("{:.1}", x + if dot.is_some() { 28.0 } else { 12.0 })) y=(format!("{:.1}", y + 11.0)) { (short(sub)) }
            }
            (card(x, top + NODE_H + 4.0, name, card_lines))
        }
    }
}

fn agent_lines(a: &Agent<'_>, i: &Inputs<'_>) -> Vec<String> {
    let mut lines = vec![format!("id {}", a.id)];
    if let Some(t) = a.tag {
        lines.push(format!("harness {t}"));
    }
    if let Some(r) = a.row {
        lines.push(format!("{} requests", r["requests"]));
        lines.push(format!("overhead p50/p95 {}", pair(&r["overhead"])));
        lines.push(format!("ttft p50/p95 {}", pair(&r["ttft"])));
        if let Some(l) = last_text(&r["last"], i) {
            lines.push(format!("last response {l}"));
        }
    }
    lines
}

fn provider_lines(p: &Provider<'_>, i: &Inputs<'_>) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(r) = p.row {
        lines.push(format!("{} requests", r["requests"]));
        lines.push(format!("own ttft p50/p95 {}", pair(&r["own_ttft"])));
        if let Some(l) = last_text(&r["last"], i) {
            lines.push(format!("last response {l}"));
        }
        for a in rows(&r["agents"]) {
            lines.push(format!("{} {}", a["name"].as_str().or(a["id"].as_str()).unwrap_or("?"), a["requests"]));
        }
    }
    lines
}

/// The line under an agent's name: its harness tag, and "revoked" for a revoked key.
fn sub_of(a: &Agent<'_>) -> Option<String> {
    match (a.tag, a.revoked) {
        (Some(t), true) => Some(format!("{t} · revoked")),
        (Some(t), false) => Some(t.to_owned()),
        (None, true) => Some("revoked".to_owned()),
        (None, false) => None,
    }
}

/// The pipe from an agent to the router, with its overhead gauge when it has traffic.
fn agent_pipe(a: &Agent<'_>, y: f64, rx: f64, mid: f64) -> Markup {
    let x0 = AGENT_X + NODE_W;
    let class =
        if a.row.is_some() { format!("pipe pipe--{}", agent_colour(a.colour)) } else { "pipe pipe--idle".to_owned() };
    html! {
        path class=(class) d=(pipe_path(x0, y, rx, mid)) {}
        @if let Some(r) = a.row {
            (gauge((x0 + rx) / 2.0, (y + mid) / 2.0, r["overhead"]["p50"].as_f64(), OVERHEAD_FULL_MS, &pair(&r["overhead"]), r["requests"].as_u64().unwrap_or(0)))
        }
    }
}

/// The pipe from the router to a provider: one stroke per agent that used it, in that agent's
/// colour; dim with no traffic. The own-wait gauge sits on it.
fn provider_pipe(p: &Provider<'_>, agents: &[Agent<'_>], y: f64, rx: f64, mid: f64) -> Markup {
    let x0 = rx + ROUTER_W;
    let used: Vec<&Value> = p.row.map_or_else(Vec::new, |r| rows(&r["agents"]).iter().collect());
    let strokes: Vec<(String, f64)> = used
        .iter()
        .enumerate()
        .map(|(k, u)| {
            let colour = agents.iter().find(|a| u["id"] == a.id).map_or(0, |a| a.colour);
            (format!("pipe pipe--{}", agent_colour(colour)), (k as f64 - (used.len() as f64 - 1.0) / 2.0) * 4.0)
        })
        .collect();
    html! {
        @if strokes.is_empty() {
            path class="pipe pipe--idle" d=(pipe_path(x0, mid, PROVIDER_X, y)) {}
        }
        @for (class, off) in &strokes {
            path class=(class) d=(pipe_path(x0, mid + off, PROVIDER_X, y + off)) {}
        }
        @if let Some(r) = p.row {
            (gauge((x0 + PROVIDER_X) / 2.0, (mid + y) / 2.0, r["own_ttft"]["p50"].as_f64(), TTFT_FULL_MS, &pair(&r["own_ttft"]), r["requests"].as_u64().unwrap_or(0)))
        }
    }
}

/// The landscape: nodes, pipes, gauges; below the drawing, the empty-state text when the window
/// holds no request.
pub fn landscape(i: &Inputs<'_>) -> Markup {
    let (agents, providers) = (agents(i), providers(i));
    let rows_n = agents.len().max(providers.len()).max(1);
    let height = rows_n as f64 * PITCH + 24.0;
    let mid = height / 2.0;
    let rx = (WIDTH - ROUTER_W) / 2.0;
    let none = rows(&i.latency["agents"]).is_empty() && rows(&i.latency["providers"]).is_empty();

    html! {
        div class="landscape" {
            svg class="landscape__svg" viewBox=(format!("0 0 {WIDTH:.0} {height:.0}")) role="img" aria-label="Agent traffic in the last 24 hours" {
                @for (n, a) in agents.iter().enumerate() {
                    (agent_pipe(a, column_y(n, agents.len(), mid), rx, mid))
                }
                @for (n, p) in providers.iter().enumerate() {
                    (provider_pipe(p, &agents, column_y(n, providers.len(), mid), rx, mid))
                }
                g class="landscape__node" {
                    rect class="landscape__router" x=(format!("{rx:.1}")) y=(format!("{:.1}", mid - ROUTER_H / 2.0)) width=(format!("{ROUTER_W:.0}")) height=(format!("{ROUTER_H:.0}")) rx="10" {}
                    text class="landscape__name" x=(format!("{:.1}", rx + ROUTER_W / 2.0)) y=(format!("{:.1}", mid + 4.0)) text-anchor="middle" { "0router" }
                }
                @for (n, a) in agents.iter().enumerate() {
                    (node(AGENT_X, column_y(n, agents.len(), mid), &a.name, sub_of(a).as_deref(), Some(a.colour), a.revoked, &agent_lines(a, i)))
                }
                @for (n, p) in providers.iter().enumerate() {
                    (node(PROVIDER_X, column_y(n, providers.len(), mid), p.id, None, None, false, &provider_lines(p, i)))
                }
            }
            @if none { p class="landscape__empty" { (EMPTY) } }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn tz() -> TimeZone {
        TimeZone::UTC
    }

    fn at() -> Timestamp {
        "2026-10-06T14:02:11Z".parse().unwrap()
    }

    fn keys() -> Value {
        json!([
            {"id": "ak_a", "name": "claude-code", "harness": "claude-code", "revoked": null},
            {"id": "ak_b", "name": "old", "harness": null, "revoked": "2026-10-01T00:00:00Z"},
            {"id": "ak_c", "name": "ci-bot", "harness": null, "revoked": "2026-10-01T00:00:00Z"},
        ])
    }

    fn latency() -> Value {
        json!({
            "agents": [
                {"id": "ak_a", "name": "claude-code", "requests": 612,
                 "overhead": {"p50": 5.0, "p95": 9.0, "n": 612}, "ttft": {"p50": 420.0, "p95": 1910.0, "n": 600},
                 "last": {"result": "resolved", "at": "2026-10-06T14:01:58Z", "status": null}},
                {"id": "ak_b", "name": "old", "requests": 2,
                 "overhead": {"p50": 90.0, "p95": 90.0, "n": 2}, "ttft": null, "last": null},
            ],
            "providers": [
                {"id": "anthropic", "requests": 614, "own_ttft": {"p50": 5000.0, "p95": 6000.0, "n": 600},
                 "last": {"result": "failed", "at": "2026-10-05T13:00:31Z", "status": 503},
                 "agents": [{"id": "ak_a", "name": "claude-code", "requests": 612}, {"id": "ak_b", "name": "old", "requests": 2}]},
            ],
        })
    }

    fn accounts() -> Value {
        json!([{"provider": "anthropic", "name": "main"}, {"provider": "xai", "name": "work"}, {"provider": "xai", "name": "spare"}])
    }

    fn render(keys: &Value, accounts: &Value, latency: &Value) -> String {
        let tz = tz();
        landscape(&Inputs { keys, accounts, latency, tz: &tz, as_of: at() }).into_string()
    }

    #[test]
    fn the_nodes_are_unrevoked_keys_and_account_providers_plus_revoked_ones_with_traffic() {
        let html = render(&keys(), &accounts(), &latency());
        assert!(html.contains(">claude-code<") && html.contains(">old<"), "the revoked key with traffic shows");
        assert!(!html.contains(">ci-bot<"), "a revoked key with no traffic doesn't");
        assert!(
            html.contains(">anthropic<") && html.contains(">xai<"),
            "a provider with an account and no traffic shows"
        );
        assert!(html.contains("old") && html.contains("revoked"), "the revoked key is marked");
        assert!(html.contains("landscape__node--revoked"));
    }

    #[test]
    fn a_provider_with_traffic_and_no_account_still_shows() {
        let mut l = latency();
        l["providers"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": "gone", "requests": 1, "own_ttft": null, "last": null, "agents": []}));
        let html = render(&keys(), &accounts(), &l);
        assert!(html.contains(">gone<"), "{html}");
    }

    #[test]
    fn the_numbers_are_printed_as_text_beside_every_gauge() {
        let html = render(&keys(), &accounts(), &latency());
        for text in ["5 ms / 9 ms", "612 req", "5.0 s / 6.0 s", "614 req", "90 ms / 90 ms", "2 req"] {
            assert!(html.contains(&format!(">{text}<")), "{text}\n{html}");
        }
    }

    #[test]
    fn the_needle_sits_at_the_median_and_pins_beyond_full_scale() {
        // 5 ms of 40 is 1/8: 180 - 22.5 = 157.5 degrees; 5 s of 3 s pins at 0 degrees.
        let (x, y) = point(100.0, 100.0, DIAL_R - 2.0, 157.5);
        let g = gauge(100.0, 100.0, Some(5.0), OVERHEAD_FULL_MS, "x", 1).into_string();
        assert!(g.contains(&format!("x2=\"{x:.1}\"")) && g.contains(&format!("y2=\"{y:.1}\"")), "{g}");
        let pinned = gauge(100.0, 100.0, Some(5000.0), TTFT_FULL_MS, "5.0 s", 1).into_string();
        let (px, py) = point(100.0, 100.0, DIAL_R - 2.0, 0.0);
        assert!(pinned.contains(&format!("x2=\"{px:.1}\"")) && pinned.contains(&format!("y2=\"{py:.1}\"")), "{pinned}");
        assert!(pinned.contains(">5.0 s<"), "the value beyond full scale is still printed");
        let none = gauge(100.0, 100.0, None, TTFT_FULL_MS, "none", 3).into_string();
        assert!(!none.contains("gauge__needle") && none.contains(">none<"), "no value: no needle, and `none` printed");
    }

    #[test]
    fn a_provider_pipe_has_one_stroke_per_agent_in_that_agents_colour() {
        let html = render(&keys(), &accounts(), &latency());
        assert!(html.contains("pipe pipe--agent-1") && html.contains("pipe pipe--agent-2"), "{html}");
        assert_eq!(html.matches("pipe pipe--idle").count(), 1, "only xai's pipe has no traffic: {html}");
    }

    #[test]
    fn with_no_traffic_the_nodes_show_with_the_empty_text() {
        let html = render(&keys(), &accounts(), &json!({"agents": [], "providers": []}));
        assert!(html.contains(EMPTY) && html.contains(">claude-code<") && html.contains(">anthropic<"));
        assert!(!html.contains("gauge__value"), "no numbers without traffic");
        let busy = render(&keys(), &accounts(), &latency());
        assert!(!busy.contains(EMPTY));
    }

    #[test]
    fn a_long_name_is_shortened_and_kept_whole_in_the_card() {
        let long = "a-very-long-agent-key-name-indeed";
        let keys = json!([{"id": "ak_l", "name": long, "harness": null, "revoked": null}]);
        let html = render(&keys, &json!([]), &json!({"agents": [], "providers": []}));
        assert!(html.contains(&format!("{}…", &long[..SHOWN_CHARS - 1])), "{html}");
        assert!(
            html.contains(&format!("<title>{long}</title>")) && html.contains(&format!(">{long}<")),
            "the full name is in the title and the card"
        );
    }

    #[test]
    fn the_last_response_names_the_date_only_when_it_isnt_the_read_date() {
        let html = render(&keys(), &accounts(), &latency());
        assert!(html.contains("last response resolved 14:01:58"), "{html}");
        assert!(html.contains("last response failed 2026-10-05 13:00:31 (503)"), "{html}");
    }

    #[test]
    fn the_drawing_has_no_script_animation_or_external_reference() {
        let html = render(&keys(), &accounts(), &latency());
        for banned in
            ["<script", "<animate", "<style", "href=", "xlink", "url(", "http://", "https://", "onclick", "onload"]
        {
            assert!(!html.contains(banned), "{banned}\n{html}");
        }
    }

    #[test]
    fn the_heading_names_the_window_and_the_time() {
        assert_eq!(heading(at(), &tz()), "Agent traffic · last 24 h · as of 14:02:11");
    }

    #[test]
    fn times_read_as_the_cli_prints_them() {
        assert_eq!(ms(4.6), "5 ms");
        assert_eq!(ms(999.0), "999 ms");
        assert_eq!(ms(1910.0), "1.9 s");
        assert_eq!(pair(&Value::Null), "none");
    }
}
