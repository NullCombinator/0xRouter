//! The traffic landscape on Endpoint & Key (spec 010 US3, research R9): agents on the left, the
//! router in the centre, providers on the right, one pipe per hop with a pressure gauge on it.
//! Pipes are glass tubes that run straight into a manifold on each side of the router; the liquid
//! in each is its agent's colour, one lane per agent on a provider's pipe, and a sheen and bubbles
//! move through it in the direction of the traffic (SVG animation as the mockup draws it, no
//! script; hidden under `prefers-reduced-motion`).
//! Inline SVG drawn on the server: no script, no external reference. A dial prints its median
//! inside and marks its 95th percentile on the scale; its request count is on the node. Hovering
//! a node or a gauge shows a card, drawn over the rest of the drawing, with that hop's numbers.
//!
//! The nodes are every unrevoked key and every provider with an account, plus any revoked key or
//! other provider with a row in the latency view (FR-020). Colours come from `agent_colour`.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use maud::{Markup, html};
use serde_json::Value;

use crate::assets;
use crate::components::agent_colour;
use crate::logos::Index;
use crate::time::local_text;

/// Router overhead dial: full scale, in ms.
pub const OVERHEAD_FULL_MS: f64 = 40.0;
/// Provider time-to-first-token dial: full scale, in ms.
pub const TTFT_FULL_MS: f64 = 3000.0;
pub const EMPTY: &str = "No requests in the last 24 hours";
/// Longest name drawn in a node before it is shortened (the full text stays in the card).
pub const SHOWN_CHARS: usize = 16;

const WIDTH: f64 = 1200.0;
const NODE_W: f64 = 190.0;
const NODE_H: f64 = 52.0;
const AGENT_X: f64 = 105.0;
const PROVIDER_X: f64 = 905.0;
const ROUTER_W: f64 = 130.0;
const ROUTER_H: f64 = 52.0;
const PITCH: f64 = 110.0;
/// The dial: the scale's radius, the bezel's, and the stem that screws it into its pipe. The
/// scale sweeps `SWEEP` degrees clockwise from `START`, as a pressure gauge's does.
const DIAL_R: f64 = 18.0;
const FACE_R: f64 = 25.0;
const NECK: f64 = 10.0;
const START: f64 = 210.0;
const SWEEP: f64 = 240.0;
/// The scale's bands, pale to deep: 9router's red spectrum.
const BANDS: [&str; 7] = ["red-50", "red-100", "red-200", "red-300", "red-400", "red-500", "red-600"];
/// The manifolds: where agent pipes turn towards the router, and provider pipes leave it.
const MANIFOLD_L: f64 = 490.0;
const MANIFOLD_R: f64 = 710.0;
/// The pipe's bore, and the liquid inside it.
const BORE: f64 = 20.0;
const LIQUID: f64 = 14.0;
/// The sheen's wavelength and period, as the mockup's `flowR`.
const SHEEN: f64 = 80.0;
const SHEEN_DUR: &str = "1.3s";

/// What the landscape reads: three views and the page's clock.
pub struct Inputs<'a> {
    pub keys: &'a Value,
    pub accounts: &'a Value,
    pub latency: &'a Value,
    pub tz: &'a TimeZone,
    pub as_of: Timestamp,
    pub logos: &'a Index,
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

/// The angle of `frac` (0 to 1) of full scale.
fn scale_deg(frac: f64) -> f64 {
    START - SWEEP * frac.clamp(0.0, 1.0)
}

fn arc(cx: f64, cy: f64, r: f64, from: f64, to: f64) -> String {
    let (x0, y0) = point(cx, cy, r, from);
    let (x1, y1) = point(cx, cy, r, to);
    let large = u8::from(from - to > 180.0);
    format!("M{x0:.1},{y0:.1} A{r},{r} 0 {large} 1 {x1:.1},{y1:.1}")
}

fn gauge(x: f64, y: f64, p50: Option<f64>, p95: Option<f64>, full: f64) -> Markup {
    let needle = p50.map(|v| point(x, y, DIAL_R - 2.0, scale_deg(v / full)));
    let peak = p95.map(|v| (point(x, y, DIAL_R + 3.5, scale_deg(v / full)), point(x, y, DIAL_R - 2.5, scale_deg(v / full))));
    let band = SWEEP / BANDS.len() as f64;
    let f = |v: f64| format!("{v:.1}");
    html! {
        // The stem, its hex nut and the thread below it.
        rect class="gauge__stem" x=(f(x - 3.5)) y=(f(y + FACE_R - 1.0)) width="7" height=(f(NECK + 1.0)) {}
        rect class="gauge__nut" x=(f(x - 8.0)) y=(f(y + FACE_R + 1.0)) width="16" height="5" rx="1.5" {}
        @for k in 0..2 {
            @let ty = y + FACE_R + 7.5 + 2.0 * f64::from(k);
            line class="gauge__thread" x1=(f(x - 3.5)) y1=(f(ty)) x2=(f(x + 3.5)) y2=(f(ty)) {}
        }
        circle class="gauge__bezel" cx=(f(x)) cy=(f(y)) r=(f(FACE_R)) stroke-width="2" {}
        circle class="gauge__face" cx=(f(x)) cy=(f(y)) r=(f(FACE_R - 3.0)) {}
        path class="gauge__track" d=(arc(x, y, DIAL_R, START, START - SWEEP)) stroke-width="7" {}
        @for (n, shade) in BANDS.iter().enumerate() {
            @let from = START - band * n as f64;
            path class=(format!("gauge__band gauge__band--{shade}")) d=(arc(x, y, DIAL_R, from, from - band)) stroke-width="4.5" {}
        }
        @for k in 0..=9 {
            @let deg = START - SWEEP * f64::from(k) / 9.0;
            @let (x0, y0) = point(x, y, DIAL_R - 4.5, deg);
            @let (x1, y1) = point(x, y, DIAL_R - (if k % 3 == 0 { 8.5 } else { 7.0 }), deg);
            line class="gauge__tick" x1=(f(x0)) y1=(f(y0)) x2=(f(x1)) y2=(f(y1)) {}
        }
        @if let Some(((ox, oy), (ix, iy))) = peak {
            line class="gauge__peak" x1=(f(ox)) y1=(f(oy)) x2=(f(ix)) y2=(f(iy)) stroke-width="2" {}
        }
        @if let Some((nx, ny)) = needle {
            line class="gauge__needle" x1=(f(x)) y1=(f(y)) x2=(f(nx)) y2=(f(ny)) stroke-width="1.8" {}
            circle class="gauge__hub" cx=(f(x)) cy=(f(y)) r="3" {}
        }
        path class="gauge__glare" d=(arc(x, y, FACE_R - 5.0, 160.0, 100.0)) stroke-width="2" {}
        text class="gauge__value" x=(f(x)) y=(f(y + 16.0)) text-anchor="middle" { (p50.map_or_else(|| "none".to_owned(), ms)) }
    }
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

/// One line of a hover card.
enum Row {
    /// A label on the left and its value on the right.
    Stat(&'static str, String),
    /// The last response, coloured by its result.
    Last(String, bool),
    /// A small heading over the rows after it.
    Section(&'static str),
    /// One agent's share: its colour, its name and its count.
    Share(usize, String, String),
}

fn last_row(last: &Value, i: &Inputs<'_>) -> Option<Row> {
    Some(Row::Last(last_text(last, i)?, last["result"] == "resolved"))
}

/// A hover card: title, the window it reads, a rule, then the rows. Cards are drawn in their
/// own layer after everything else (see `landscape`), and placed towards the drawing's middle so
/// they stay inside it.
fn tip(cx: f64, (top, bottom): (f64, f64), mid: f64, title: &str, window: &str, rows: &[Row]) -> Markup {
    // Text widths, near enough for the sans at 12 px.
    let chars = |t: &str| t.chars().count() as f64;
    let (mut label_w, mut value_w): (f64, f64) = (0.0, 0.0);
    for r in rows {
        let (l, v) = match r {
            Row::Stat(l, v) => (chars(l), chars(v)),
            Row::Last(v, _) => (chars("Last response"), chars(v)),
            Row::Section(l) => (chars(l), 0.0),
            Row::Share(_, l, v) => (chars(l) + 2.0, chars(v)),
        };
        label_w = label_w.max(l);
        value_w = value_w.max(v);
    }
    let w = (32.0 + (label_w + value_w) * 6.4 + 24.0).max(chars(title) * 7.2 + 32.0).max(chars(window) * 6.0 + 32.0).min(WIDTH - 8.0);
    let h = 62.0 + 19.0 * rows.len() as f64;
    let x = (cx - w / 2.0).clamp(4.0, WIDTH - 4.0 - w);
    let y = if (top + bottom) / 2.0 < mid { bottom + 8.0 } else { top - 8.0 - h };
    let f = |v: f64| format!("{v:.1}");
    let (left, right) = (x + 16.0, x + w - 16.0);
    html! {
        rect class="landscape__tip" x=(f(x)) y=(f(y)) width=(f(w)) height=(f(h)) rx="10" filter="url(#card-shadow)" {}
        text class="landscape__tip-title" x=(f(left)) y=(f(y + 22.0)) { (title) }
        text class="landscape__tip-window" x=(f(left)) y=(f(y + 38.0)) { (window) }
        line class="landscape__tip-rule" x1=(f(left)) y1=(f(y + 48.0)) x2=(f(right)) y2=(f(y + 48.0)) {}
        @for (n, r) in rows.iter().enumerate() {
            @let ry = y + 66.0 + 19.0 * n as f64;
            @match r {
                Row::Stat(l, v) => {
                    text class="landscape__tip-label" x=(f(left)) y=(f(ry)) { (l) }
                    text class="landscape__tip-value" x=(f(right)) y=(f(ry)) text-anchor="end" { (v) }
                }
                Row::Last(v, ok) => {
                    text class="landscape__tip-label" x=(f(left)) y=(f(ry)) { "Last response" }
                    text class=(if *ok { "landscape__tip-value landscape__tip-value--ok" } else { "landscape__tip-value landscape__tip-value--failed" }) x=(f(right)) y=(f(ry)) text-anchor="end" { (v) }
                }
                Row::Section(l) => {
                    text class="landscape__tip-section" x=(f(left)) y=(f(ry)) { (l) }
                }
                Row::Share(colour, l, v) => {
                    circle class=(format!("landscape__swatch-dot landscape__ring--{}", agent_colour(*colour))) cx=(f(left + 3.5)) cy=(f(ry - 4.0)) r="3.5" {}
                    text class="landscape__tip-label" x=(f(left + 12.0)) y=(f(ry)) { (l) }
                    text class="landscape__tip-value" x=(f(right)) y=(f(ry)) text-anchor="end" { (v) }
                }
            }
        }
    }
}

/// What a node shows before its name: an agent's avatar or a provider's logo.
enum Mark<'a> {
    Avatar(usize),
    Logo(&'a str),
    Letters(String),
}

/// An agent's avatar, as the mockup draws it: two soft blobs in its colour and the next one on a
/// pale disc, ringed in its colour.
fn avatar(cx: f64, cy: f64, n: usize) -> Markup {
    let (cx, cy) = (format!("{cx:.1}"), format!("{cy:.1}"));
    html! {
        circle class="landscape__avatar" cx=(cx) cy=(cy) r="15" {}
        circle cx=(cx) cy=(cy) r="15" fill=(format!("url(#avatar-{n}-a)")) {}
        circle cx=(cx) cy=(cy) r="15" fill=(format!("url(#avatar-{n}-b)")) {}
        circle class=(format!("landscape__ring landscape__ring--{}", agent_colour(n))) cx=(cx) cy=(cy) r="15" {}
    }
}

/// The two gradients behind agent colour `n`'s avatar.
fn avatar_defs(n: usize) -> Markup {
    let next = (n + 1) % 7;
    html! {
        radialGradient id=(format!("avatar-{n}-a")) cx="20%" cy="25%" r="60%" {
            stop class=(format!("landscape__stop--{}", agent_colour(n))) offset="0" {}
            stop class=(format!("landscape__stop--{} landscape__stop--clear", agent_colour(n))) offset="1" {}
        }
        radialGradient id=(format!("avatar-{n}-b")) cx="80%" cy="85%" r="55%" {
            stop class=(format!("landscape__stop--{}", agent_colour(next))) offset="0" {}
            stop class=(format!("landscape__stop--{} landscape__stop--clear", agent_colour(next))) offset="1" {}
        }
    }
}

fn mark(x: f64, y: f64, m: &Mark<'_>) -> Markup {
    let f = |v: f64| format!("{v:.1}");
    match m {
        Mark::Avatar(n) => avatar(x + 30.0, y, *n),
        Mark::Logo(href) => html! {
            rect class="landscape__logo-tile" x=(f(x + 14.0)) y=(f(y - 16.0)) width="32" height="32" rx="8" {}
            image href=(href) x=(f(x + 16.0)) y=(f(y - 14.0)) width="28" height="28" {}
        },
        Mark::Letters(t) => html! {
            rect class="landscape__letters-tile" x=(f(x + 14.0)) y=(f(y - 16.0)) width="32" height="32" rx="8" {}
            text class="landscape__letters" x=(f(x + 30.0)) y=(f(y + 4.5)) text-anchor="middle" { (t) }
        },
    }
}

fn provider_mark<'a>(id: &str, logos: &'a Index) -> Mark<'a> {
    logos.href(id).map_or_else(
        || Mark::Letters(id.chars().filter(|c| c.is_alphanumeric()).take(2).collect::<String>().to_uppercase()),
        Mark::Logo,
    )
}

/// A node's box, mark and name; its card is drawn with it by `hot`.
fn node(x: f64, y: f64, name: &str, sub: Option<&str>, m: &Mark<'_>) -> Markup {
    let top = y - NODE_H / 2.0;
    let f = |v: f64| format!("{v:.1}");
    html! {
        title { (name) }
        rect class="landscape__box" x=(f(x)) y=(f(top)) width=(format!("{NODE_W:.0}")) height=(format!("{NODE_H:.0}")) rx="8" {}
        (mark(x, y, m))
        text class="landscape__name" x=(f(x + 56.0)) y=(f(if sub.is_some() { y - 2.0 } else { y + 5.5 })) { (short(name)) }
        @if let Some(sub) = sub {
            text class="landscape__tag" x=(f(x + 56.0)) y=(f(y + 14.0)) { (sub) }
        }
    }
}

/// How many hover cards the stylesheet can raise to the top layer (`landscape__hot--N`); a
/// drawing with more keeps the rest of its cards inside their items.
const RAISED: usize = 64;

/// Something that shows a card while hovered or focused, the `n`th in the drawing. Returns the
/// item, and its card for the top layer (or `None` when the card stays in the item).
fn hot(n: usize, class: &str, body: Markup, card: Markup) -> (Markup, Option<Markup>) {
    if n < RAISED {
        (
            html! { g class=(format!("landscape__hot landscape__hot--{n} {class}")) tabindex="0" { (body) } },
            Some(html! { g class=(format!("landscape__card landscape__card--{n}")) { (card) } }),
        )
    } else {
        (html! { g class=(format!("landscape__hot {class}")) tabindex="0" { (body) g class="landscape__card" { (card) } } }, None)
    }
}

fn agent_rows(a: &Agent<'_>, i: &Inputs<'_>) -> Vec<Row> {
    let mut out = vec![Row::Stat("Key", a.id.to_owned())];
    if let Some(t) = a.tag {
        out.push(Row::Stat("Harness", t.to_owned()));
    }
    if let Some(r) = a.row {
        out.push(Row::Stat("Requests", r["requests"].to_string()));
        out.push(Row::Stat("Overhead p50 / p95", pair(&r["overhead"])));
        out.push(Row::Stat("TTFT p50 / p95", pair(&r["ttft"])));
        out.extend(last_row(&r["last"], i));
    }
    out
}

fn provider_rows(p: &Provider<'_>, agents: &[Agent<'_>], i: &Inputs<'_>) -> Vec<Row> {
    let mut out = Vec::new();
    if let Some(r) = p.row {
        out.push(Row::Stat("Requests", r["requests"].to_string()));
        out.push(Row::Stat("Own TTFT p50 / p95", pair(&r["own_ttft"])));
        out.extend(last_row(&r["last"], i));
        let shares = rows(&r["agents"]);
        if !shares.is_empty() {
            out.push(Row::Section("By agent"));
        }
        for u in shares {
            let colour = agents.iter().find(|a| u["id"] == a.id).map_or(0, |a| a.colour);
            let name = u["name"].as_str().or(u["id"].as_str()).unwrap_or("?").to_owned();
            out.push(Row::Share(colour, name, u["requests"].to_string()));
        }
    }
    out
}

/// The line under an agent's name: its harness tag, "revoked" for a revoked key, and its
/// request count when it has traffic.
fn sub_of(a: &Agent<'_>) -> Option<String> {
    let requests = a.row.map(|r| format!("{} req", r["requests"]));
    let parts: Vec<String> =
        [a.tag.map(str::to_owned), a.revoked.then(|| "revoked".to_owned()), requests].into_iter().flatten().collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// A straight run of tube, centre line from one point to the other.
#[derive(Clone, Copy)]
struct Run(f64, f64, f64, f64);

impl Run {
    fn line(self, class: &str, width: f64) -> Markup {
        let Run(x0, y0, x1, y1) = self;
        html! { line class=(class) x1=(format!("{x0:.1}")) y1=(format!("{y0:.1}")) x2=(format!("{x1:.1}")) y2=(format!("{y1:.1}")) stroke-width=(format!("{width:.1}")) {} }
    }

    /// The sheen gradient that moves the way this run's traffic goes.
    fn sheen(self) -> &'static str {
        let Run(x0, y0, x1, y1) = self;
        if y0 == y1 {
            if x1 >= x0 { "url(#flow-right)" } else { "url(#flow-left)" }
        } else if y1 > y0 {
            "url(#flow-down)"
        } else {
            "url(#flow-up)"
        }
    }

    /// Bubbles riding the liquid from the start of the run to its end, staggered so they never
    /// move in step: about one per 60 units, each with its own size, speed and lane.
    fn bubbles(self, width: f64, seed: usize) -> Markup {
        let Run(x0, y0, x1, y1) = self;
        let len = (x1 - x0).abs() + (y1 - y0).abs();
        let count = ((len / 60.0).ceil() as usize).clamp(1, 6);
        let across = y0 == y1;
        html! {
            @for b in 0..count {
                @let k = seed * 7 + b * 13;
                @let r = 1.3 + (k % 5) as f64 * 0.25;
                @let dur = len / (55.0 + (k % 4) as f64 * 9.0);
                @let off = ((k % 3) as f64 - 1.0) * width * 0.22;
                @let (dx, dy) = if across { (0.0, off) } else { (off, 0.0) };
                circle class="pipe__bubble" r=(format!("{r:.2}")) {
                    animateMotion dur=(format!("{dur:.2}s")) begin=(format!("-{:.2}s", dur * b as f64 / count as f64 + (k % 7) as f64 * 0.1)) repeatCount="indefinite"
                        path=(format!("M{:.1},{:.1} L{:.1},{:.1}", x0 + dx, y0 + dy, x1 + dx, y1 + dy)) {}
                }
            }
        }
    }
}

/// A sheen gradient: a soft white band every `SHEEN` units, sliding along `(dx, dy)`.
fn sheen_def(id: &str, (dx, dy): (f64, f64)) -> Markup {
    html! {
        linearGradient id=(id) gradientUnits="userSpaceOnUse" x1="0" y1="0" x2=(format!("{:.0}", dx.abs())) y2=(format!("{:.0}", dy.abs())) spreadMethod="repeat" {
            stop class="pipe__sheen-stop" offset="0" {}
            stop class="pipe__sheen-stop pipe__sheen-stop--peak" offset="0.5" {}
            stop class="pipe__sheen-stop" offset="1" {}
            animateTransform attributeName="gradientTransform" type="translate" from="0 0" to=(format!("{dx:.0} {dy:.0}")) dur=(SHEEN_DUR) repeatCount="indefinite" {}
        }
    }
}

/// The liquid in a run: one class per stroke (`pipe pipe--agent-N`, or `pipe pipe--idle`).
struct Fill {
    class: String,
    run: Run,
    width: f64,
}

/// A flange where a tube meets a node or the router: a short band across a horizontal run.
fn collar(x: f64, y: f64) -> Markup {
    html! { rect class="pipe__collar" x=(format!("{:.1}", x - 3.5)) y=(format!("{:.1}", y - BORE / 2.0 - 4.0)) width="7" height=(format!("{:.0}", BORE + 8.0)) rx="2" {} }
}

fn agent_class(a: &Agent<'_>) -> String {
    if a.row.is_some() { format!("pipe pipe--{}", agent_colour(a.colour)) } else { "pipe pipe--idle".to_owned() }
}

/// The gauge on a pipe's straight run, its face above the bore on a neck: the dial's centre.
fn gauge_centre(x: f64, y: f64) -> (f64, f64) {
    (x, y - BORE / 2.0 - NECK - FACE_R)
}

/// The vertical extent of a gauge whose dial is centred at `cy`, stem included.
fn gauge_span(cy: f64) -> (f64, f64) {
    (cy - FACE_R, cy + FACE_R + NECK)
}

/// The landscape: nodes, pipes, gauges, the legend; below the drawing, the empty-state text when
/// the window holds no request.
pub fn landscape(i: &Inputs<'_>) -> Markup {
    let (agents, providers) = (agents(i), providers(i));
    let rows_n = agents.len().max(providers.len()).max(1);
    let height = rows_n as f64 * PITCH + 60.0;
    let mid = height / 2.0 + 30.0;
    let rx = (WIDTH - ROUTER_W) / 2.0;
    let none = rows(&i.latency["agents"]).is_empty() && rows(&i.latency["providers"]).is_empty();
    let ay: Vec<f64> = (0..agents.len()).map(|n| column_y(n, agents.len(), mid)).collect();
    let py: Vec<f64> = (0..providers.len()).map(|n| column_y(n, providers.len(), mid)).collect();
    let span = |ys: &[f64]| {
        ys.iter().fold((mid, mid), |(lo, hi), y| (lo.min(*y), hi.max(*y)))
    };
    let window = format!("Last 24 h · as of {}", i.as_of.to_zoned(i.tz.clone()).strftime("%H:%M:%S"));

    // The tubes, then the liquid in them; then everything that shows a card on hover, and the
    // cards themselves, drawn last so each opens over the whole drawing.
    let mut tubes = Vec::new();
    let mut fills = Vec::new();
    let mut collars = Vec::new();
    let mut hots: Vec<Markup> = Vec::new();
    let mut cards: Vec<Markup> = Vec::new();
    let mut add = |class: &str, body: Markup, card: Markup| {
        let (item, raised) = hot(hots.len(), class, body, card);
        hots.push(item);
        cards.extend(raised);
    };
    let node_span = |y: f64| (y - NODE_H / 2.0, y + NODE_H / 2.0);
    let x0 = AGENT_X + NODE_W;
    for (a, &y) in agents.iter().zip(&ay) {
        let run = Run(x0, y, MANIFOLD_L, y);
        tubes.push(run);
        fills.push(Fill { class: agent_class(a), run, width: LIQUID });
        if a.row.is_some() {
            fills.push(Fill { class: agent_class(a), run: Run(MANIFOLD_L, y, MANIFOLD_L, mid), width: LIQUID });
        }
        collars.push(collar(x0 + 5.0, y));
        collars.push(collar(MANIFOLD_L - BORE / 2.0 - 6.0, y));
        if let Some(r) = a.row {
            let (gx, gy) = gauge_centre((x0 + MANIFOLD_L) / 2.0 - 6.0, y);
            let stats = [Row::Stat("Router overhead p50 / p95", pair(&r["overhead"])), Row::Stat("Requests", r["requests"].to_string())];
            let body = gauge(gx, gy, r["overhead"]["p50"].as_f64(), r["overhead"]["p95"].as_f64(), OVERHEAD_FULL_MS);
            add("gauge", body, tip(gx, gauge_span(gy), mid, &format!("{} → router", a.name), &window, &stats));
        }
        let class = if a.revoked { "landscape__node landscape__node--revoked" } else { "landscape__node" };
        let body = node(AGENT_X, y, &a.name, sub_of(a).as_deref(), &Mark::Avatar(a.colour));
        add(class, body, tip(AGENT_X + NODE_W / 2.0, node_span(y), mid, &a.name, &window, &agent_rows(a, i)));
    }
    let (lo, hi) = span(&ay);
    tubes.push(Run(MANIFOLD_L, lo, MANIFOLD_L, hi));
    let feeds = [Run(MANIFOLD_L, mid, rx, mid), Run(rx + ROUTER_W, mid, MANIFOLD_R, mid)];
    for run in feeds {
        tubes.push(run);
        fills.push(Fill { class: "pipe pipe--router".to_owned(), run, width: LIQUID });
    }
    collars.push(collar(rx - 6.0, mid));
    collars.push(collar(rx + ROUTER_W + 6.0, mid));
    let (lo, hi) = span(&py);
    tubes.push(Run(MANIFOLD_R, lo, MANIFOLD_R, hi));
    for (p, &y) in providers.iter().zip(&py) {
        let run = Run(MANIFOLD_R, y, PROVIDER_X, y);
        tubes.push(run);
        let used: Vec<&Value> = p.row.map_or_else(Vec::new, |r| rows(&r["agents"]).iter().collect());
        let lane = LIQUID / used.len().max(1) as f64;
        for (j, u) in used.iter().enumerate() {
            let colour = agents.iter().find(|a| u["id"] == a.id).map_or(0, |a| a.colour);
            let off = (j as f64 - (used.len() as f64 - 1.0) / 2.0) * lane;
            let class = format!("pipe pipe--{}", agent_colour(colour));
            if j == 0 {
                fills.push(Fill { class: class.clone(), run: Run(MANIFOLD_R, mid, MANIFOLD_R, y), width: LIQUID });
            }
            fills.push(Fill { class, run: Run(MANIFOLD_R, y + off, PROVIDER_X, y + off), width: lane });
        }
        if used.is_empty() {
            fills.push(Fill { class: "pipe pipe--idle".to_owned(), run, width: LIQUID });
        }
        collars.push(collar(MANIFOLD_R + BORE / 2.0 + 6.0, y));
        collars.push(collar(PROVIDER_X - 5.0, y));
        if let Some(r) = p.row {
            let (gx, gy) = gauge_centre((MANIFOLD_R + PROVIDER_X) / 2.0 + 6.0, y);
            let mut stats = vec![Row::Stat("Own TTFT p50 / p95", pair(&r["own_ttft"])), Row::Stat("Requests", r["requests"].to_string())];
            stats.extend(last_row(&r["last"], i));
            let body = gauge(gx, gy, r["own_ttft"]["p50"].as_f64(), r["own_ttft"]["p95"].as_f64(), TTFT_FULL_MS);
            add("gauge", body, tip(gx, gauge_span(gy), mid, &format!("router → {}", p.id), &window, &stats));
        }
        let sub = p.row.map(|r| format!("{} req", r["requests"]));
        let body = node(PROVIDER_X, y, p.id, sub.as_deref(), &provider_mark(p.id, i.logos));
        add("landscape__node", body, tip(PROVIDER_X + NODE_W / 2.0, node_span(y), mid, p.id, &window, &provider_rows(p, &agents, i)));
    }
    let flowing: Vec<&Fill> = fills.iter().filter(|f| !f.class.ends_with("--idle")).collect();

    html! {
        div class="landscape" {
            svg class="landscape__svg" viewBox=(format!("0 0 {WIDTH:.0} {height:.0}")) role="img" aria-label="Agent traffic in the last 24 hours" {
                defs {
                    @for n in 0..7 { (avatar_defs(n)) }
                    (sheen_def("flow-right", (SHEEN, 0.0)))
                    (sheen_def("flow-left", (-SHEEN, 0.0)))
                    (sheen_def("flow-down", (0.0, SHEEN)))
                    (sheen_def("flow-up", (0.0, -SHEEN)))
                    filter id="card-shadow" x="-20%" y="-20%" width="140%" height="160%" {
                        feDropShadow class="landscape__shadow" dx="0" dy="8" stdDeviation="9" flood-opacity="0.16" {}
                        feDropShadow class="landscape__shadow" dx="0" dy="1" stdDeviation="1" flood-opacity="0.06" {}
                    }
                }
                // Wall, glass, liquid, shine: each layer over the whole network, so joints stay clean.
                @for t in &tubes { (t.line("pipe__wall", BORE + 4.0)) }
                @for t in &tubes { (t.line("pipe__glass", BORE)) }
                @for f in &fills { (f.run.line(&f.class, f.width)) }
                // Traffic moving through the liquid: a sheen over it and bubbles in it.
                @for f in &flowing {
                    @let Run(x0, y0, x1, y1) = f.run;
                    line class="pipe__sheen" x1=(format!("{x0:.1}")) y1=(format!("{y0:.1}")) x2=(format!("{x1:.1}")) y2=(format!("{y1:.1}")) stroke=(f.run.sheen()) stroke-width=(format!("{:.1}", f.width)) {}
                }
                @for (n, f) in flowing.iter().enumerate() { (f.run.bubbles(f.width, n)) }
                @for t in tubes.iter().filter(|t| t.1 == t.3) {
                    (Run(t.0, t.1 - BORE / 2.0 + 3.5, t.2, t.3 - BORE / 2.0 + 3.5).line("pipe__shine", 2.0))
                }
                @for c in &collars { (c) }
                g class="landscape__node" {
                    rect class="landscape__router" x=(format!("{rx:.1}")) y=(format!("{:.1}", mid - ROUTER_H / 2.0)) width=(format!("{ROUTER_W:.0}")) height=(format!("{ROUTER_H:.0}")) rx="10" {}
                    image href=(assets::href("logo-mark.png")) x=(format!("{:.1}", rx + 16.0)) y=(format!("{:.1}", mid - 14.0)) width="28" height="28" {}
                    text class="landscape__name landscape__name--router" x=(format!("{:.1}", rx + 52.0)) y=(format!("{:.1}", mid + 5.5)) { "0Router" }
                }
                @for h in &hots { (h) }
                g class="landscape__cards" { @for c in &cards { (c) } }
            }
            @if !agents.is_empty() {
                ul class="landscape__legend" {
                    @for a in &agents {
                        li class="landscape__key" { span class=(format!("landscape__swatch landscape__swatch--{}", agent_colour(a.colour))) {} (a.name) }
                    }
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
        landscape(&Inputs { keys, accounts, latency, tz: &tz, as_of: at(), logos: &Index::default() }).into_string()
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
    fn each_dial_prints_its_median_and_each_node_its_request_count() {
        let html = render(&keys(), &accounts(), &latency());
        for median in ["5 ms", "5.0 s", "90 ms"] {
            assert!(html.contains(&format!(">{median}</text>")), "{median}\n{html}");
        }
        for sub in ["claude-code · 612 req", "revoked · 2 req", ">614 req<"] {
            assert!(html.contains(sub), "{sub}\n{html}");
        }
        // The gauge's card names the hop and both percentiles.
        for text in [">claude-code → router<", ">Router overhead p50 / p95<", ">5 ms / 9 ms<", ">router → anthropic<", ">Own TTFT p50 / p95<", ">5.0 s / 6.0 s<"] {
            assert!(html.contains(text), "{text}\n{html}");
        }
        assert_eq!(html.matches("class=\"gauge__peak\"").count(), 3, "every dial marks its 95th percentile");
    }

    #[test]
    fn every_card_is_drawn_after_everything_and_shows_with_its_item() {
        let html = render(&keys(), &accounts(), &latency());
        let layer = html.find("class=\"landscape__cards\"").unwrap();
        assert!(html.rfind("landscape__hot ").unwrap() < layer, "the cards come after every item");
        let items = html.matches("class=\"landscape__hot landscape__hot--").count();
        assert_eq!(html[layer..].matches("class=\"landscape__card landscape__card--").count(), items);
        for n in 0..items {
            assert!(html.contains(&format!("landscape__hot--{n} ")) && html.contains(&format!("landscape__card--{n}\"")), "{n}");
        }
        assert!(items < RAISED, "the stylesheet raises every card of this drawing");
    }

    #[test]
    fn the_needle_sits_at_the_median_and_pins_beyond_full_scale() {
        // 5 ms of 40 is 1/8 of the sweep: 210 - 30 = 180 degrees; 5 s of 3 s pins at the end.
        let (x, y) = point(100.0, 100.0, DIAL_R - 2.0, 180.0);
        let g = gauge(100.0, 100.0, Some(5.0), Some(9.0), OVERHEAD_FULL_MS).into_string();
        assert!(g.contains(&format!("x2=\"{x:.1}\"")) && g.contains(&format!("y2=\"{y:.1}\"")), "{g}");
        let pinned = gauge(100.0, 100.0, Some(5000.0), Some(6000.0), TTFT_FULL_MS).into_string();
        let (px, py) = point(100.0, 100.0, DIAL_R - 2.0, START - SWEEP);
        assert!(pinned.contains(&format!("x2=\"{px:.1}\"")) && pinned.contains(&format!("y2=\"{py:.1}\"")), "{pinned}");
        assert!(pinned.contains(">5.0 s<"), "the value beyond full scale is still printed");
        let none = gauge(100.0, 100.0, None, None, TTFT_FULL_MS).into_string();
        assert!(!none.contains("gauge__needle") && !none.contains("gauge__peak") && none.contains(">none<"), "no value: no needle, and `none` printed");
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
        assert!(html.contains("landscape__tip-value--ok\" x=") && html.contains(">resolved 14:01:58<"), "{html}");
        assert!(html.contains("landscape__tip-value--failed\" x=") && html.contains(">failed 2026-10-05 13:00:31 (503)<"), "{html}");
    }

    #[test]
    fn the_drawing_has_no_script_or_external_reference() {
        let html = render(&keys(), &accounts(), &latency());
        for banned in ["<script", "<style", "xlink", "http://", "https://", "onclick", "onload", "onbegin", "onend", "<set"] {
            assert!(!html.contains(banned), "{banned}\n{html}");
        }
        // The only animation moves the sheens and the bubbles.
        for (at, _) in html.match_indices("<animate") {
            let to = &html[at..];
            assert!(to.starts_with("<animateMotion ") || to.starts_with("<animateTransform attributeName=\"gradientTransform\""), "{}", &to[..60]);
        }
        // Images only from this server's assets and logos; paint only from the drawing's own gradients.
        for (at, _) in html.match_indices("href=\"") {
            let to = &html[at + 6..];
            assert!(to.starts_with("/assets/") || to.starts_with("/logos/"), "{}", &to[..40.min(to.len())]);
        }
        for (at, _) in html.match_indices("url(#") {
            let id = &html[at + 5..at + 5 + html[at + 5..].find(')').unwrap()];
            assert!(
                (id.starts_with("avatar-") || id.starts_with("flow-") || id == "card-shadow") && html.contains(&format!("id=\"{id}\"")),
                "url(#{id}) names no gradient here"
            );
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
