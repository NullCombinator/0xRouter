//! The provider topology graph on Usage (spec 010 US4, research R10), after 9router's
//! `ProviderTopology` (`usage/components/ProviderTopology.js`): the router in the centre and the
//! providers on an ellipse around it, no agent nodes. Inline SVG drawn on the server, with no
//! script: traffic moves along an edge with requests in the period (SVG animation, as the
//! mockup's, hidden under `prefers-reduced-motion`). The edge to a provider is red when its last
//! response failed, amber on the provider with the newest last response, and the border colour
//! otherwise (`edgeStyle`, `:294`). Each node prints its request count and its last response, so a
//! colour never carries a fact alone; hovering a node or the router shows its card.

use jiff::tz::TimeZone;
use maud::{Markup, html};
use serde_json::Value;

use crate::assets;
use crate::landscape::{Row, card_shadow, hot, pair, tip};
use crate::logos::Index;
use crate::time::local_text;

pub const LABEL: &str = "last response · last 24 h";
pub const NONE: &str = "none in the last 24 h";
/// A node's second line when the provider had no response in the last 24 hours.
pub const QUIET: &str = "no response in 24 h";
pub const EMPTY: &str = "No providers in this period";

// `buildLayout` (`ProviderTopology.js:263`), at the mockup's node size: a 32 px logo tile and
// two lines of text.
const NODE_W: f64 = 210.0;
const NODE_H: f64 = 52.0;
const ROUTER_W: f64 = 150.0;
const ROUTER_H: f64 = 52.0;
const NODE_GAP: f64 = 24.0;
const MIN_RX: f64 = 290.0;
const MIN_RY: f64 = 190.0;
const RATIO: f64 = 0.55;
const MARGIN: f64 = 20.0;

/// Where the `index`th of `count` providers sits relative to the router: evenly along the ellipse
/// from the top, clockwise.
pub fn position(index: usize, count: usize) -> (f64, f64) {
    let (rx, ry) = radii(count);
    let angle = -std::f64::consts::FRAC_PI_2 + 2.0 * std::f64::consts::PI * index as f64 / count.max(1) as f64;
    (rx * angle.cos(), ry * angle.sin())
}

/// The ellipse radii for `count` nodes: wide enough that neighbours don't touch.
pub fn radii(count: usize) -> (f64, f64) {
    let min_rx = (NODE_W + NODE_GAP) * count as f64 / (2.0 * std::f64::consts::PI);
    let rx = MIN_RX.max(min_rx);
    (rx, MIN_RY.max(rx * RATIO))
}

/// One provider on the graph.
pub struct Node<'a> {
    pub id: &'a str,
    pub requests: u64,
    /// `{result, at, status}` from the latency view, or `Null`.
    pub last: &'a Value,
}

/// The nodes for a period: the usage view's providers, each with its latency row's last response.
pub fn nodes<'a>(usage: &'a Value, latency: &'a Value) -> Vec<Node<'a>> {
    static NULL: Value = Value::Null;
    usage["providers"]
        .as_array()
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(|p| {
            let id = p["id"].as_str()?;
            let last = latency["providers"]
                .as_array()
                .and_then(|rows| rows.iter().find(|r| r["id"] == id))
                .map_or(&NULL, |r| &r["last"]);
            Some(Node { id, requests: p["requests"].as_u64().unwrap_or(0), last })
        })
        .collect()
}

/// `resolved · 14:01` or `failed · 14:00 · 503`; `None` when the provider has no last response.
pub fn last_text(last: &Value, tz: &TimeZone) -> Option<String> {
    let at = local_text(last["at"].as_str()?, tz)?;
    let clock = at.get(11..16)?;
    let status = last["status"].as_u64().map_or_else(String::new, |s| format!(" · {s}"));
    Some(format!("{} · {clock}{status}", last["result"].as_str()?))
}

/// The newest last response among the nodes (RFC 3339 UTC sorts as text).
fn newest<'a>(nodes: &[Node<'a>]) -> Option<&'a str> {
    nodes.iter().filter_map(|n| n.last["at"].as_str()).max()
}

/// The edge class for a node: failed beats newest; else idle.
pub fn edge_class(node: &Node<'_>, newest: Option<&str>) -> &'static str {
    if node.last["result"] == "failed" {
        "topology__edge topology__edge--failed"
    } else if node.last["at"].as_str().is_some() && node.last["at"].as_str() == newest {
        "topology__edge topology__edge--last"
    } else {
        "topology__edge"
    }
}

fn node_class(edge: &str) -> &'static str {
    if edge.ends_with("--failed") {
        "topology__node topology__node--failed"
    } else if edge.ends_with("--last") {
        "topology__node topology__node--last"
    } else {
        "topology__node"
    }
}

/// The edge from the router's side to the node's: a curve that leaves the router level (or
/// upright, for a node above or below it) and meets the node the same way (`ProviderTopology`'s
/// bezier edges).
fn edge_path(cx: f64, cy: f64) -> String {
    if cx.abs() < (NODE_W + ROUTER_W) / 2.0 {
        let s = cy.signum();
        let (y0, y1) = (s * ROUTER_H / 2.0, cy - s * NODE_H / 2.0);
        let m = (y0 + y1) / 2.0;
        format!("M0,{y0:.1} C0,{m:.1} {cx:.1},{m:.1} {cx:.1},{y1:.1}")
    } else {
        let s = cx.signum();
        let (x0, x1) = (s * ROUTER_W / 2.0, cx - s * NODE_W / 2.0);
        let m = (x0 + x1) / 2.0;
        format!("M{x0:.1},0 C{m:.1},0 {m:.1},{cy:.1} {x1:.1},{cy:.1}")
    }
}

/// Traffic on an edge: dashes sliding out of the router and dots riding along it.
fn flow(d: &str, class: &str, seed: usize) -> Markup {
    html! {
        path class=(format!("topology__flow {class}")) d=(d) stroke-dasharray="6 10" {
            animate attributeName="stroke-dashoffset" from="16" to="0" dur="0.8s" repeatCount="indefinite" {}
        }
        @for b in 0..3usize {
            @let dur = 1.6 + (seed % 3) as f64 * 0.2;
            circle class=(format!("topology__dot {class}")) r=(if b == 1 { "2.5" } else { "3.5" }) {
                animateMotion dur=(format!("{dur:.1}s")) begin=(format!("-{:.2}s", dur * b as f64 / 3.0)) repeatCount="indefinite" path=(d) {}
            }
        }
    }
}

/// What the requests count covers, then the window the last response comes from.
fn window(usage: &Value) -> String {
    format!("Requests: {} · last response: last 24 h", usage["period"].as_str().unwrap_or("period"))
}

/// The graph; the empty text instead when no provider had a request in the period.
pub fn topology(usage: &Value, latency: &Value, tz: &TimeZone, logos: &Index) -> Markup {
    let nodes = nodes(usage, latency);
    if nodes.is_empty() {
        return html! { p class="landscape__empty" { (EMPTY) } };
    }
    let (rx, ry) = radii(nodes.len());
    let (half_w, half_h) = (rx + NODE_W / 2.0 + MARGIN, ry + NODE_H / 2.0 + MARGIN);
    let bounds = (-half_w, half_w);
    let newest = newest(&nodes);
    let window = window(usage);
    let f = |v: f64| format!("{v:.1}");
    let latency_row = |id: &str| latency["providers"].as_array().and_then(|r| r.iter().find(|r| r["id"] == id));

    let mut hots = Vec::new();
    let mut cards = Vec::new();
    for (i, n) in nodes.iter().enumerate() {
        let (cx, cy) = position(i, nodes.len());
        let class = node_class(edge_class(n, newest));
        let last = last_text(n.last, tz);
        let (x, y) = (cx - NODE_W / 2.0, cy - NODE_H / 2.0);
        let body = html! {
            title { (n.id) }
            rect class=(class) x=(f(x)) y=(f(y)) width=(f(NODE_W)) height=(f(NODE_H)) rx="8" {}
            @match logos.href(n.id) {
                Some(href) => image href=(href) x=(f(x + 12.0)) y=(f(cy - 16.0)) width="32" height="32" {},
                None => text class="topology__letters" x=(f(x + 28.0)) y=(f(cy + 5.0)) text-anchor="middle" {
                    (n.id.chars().filter(|c| c.is_alphanumeric()).take(2).collect::<String>().to_uppercase())
                },
            }
            text class="topology__name" x=(f(x + 54.0)) y=(f(cy - 3.0)) { (n.id) }
            text class="topology__last" x=(f(x + 54.0)) y=(f(cy + 14.0)) {
                (n.requests) " req · " (last.as_deref().unwrap_or(QUIET))
            }
        };
        let mut rows = vec![Row::Stat("Requests", n.requests.to_string())];
        if let Some(r) = latency_row(n.id) {
            rows.push(Row::Stat("Own TTFT p50 / p95", pair(&r["own_ttft"])));
        }
        rows.push(match &last {
            Some(t) => Row::Last(t.clone(), n.last["result"] == "resolved"),
            None => Row::Stat("Last response", NONE.to_owned()),
        });
        let card = tip(cx, (y, y + NODE_H), 0.0, bounds, n.id, &window, &rows);
        let (item, top) = hot(i + 1, "topology__hot", body, card);
        hots.push(item);
        cards.extend(top);
    }
    let mut rows = vec![Row::Stat("Requests", usage["requests"].to_string())];
    if usage["in_flight"].as_u64().is_some_and(|n| n > 0) {
        rows.push(Row::Stat("In flight", usage["in_flight"].to_string()));
    }
    rows.push(Row::Section("By provider"));
    rows.extend(nodes.iter().map(|n| Row::Item(n.id.to_owned(), n.requests.to_string())));
    let router = html! {
        rect class="topology__router" x=(f(-ROUTER_W / 2.0)) y=(f(-ROUTER_H / 2.0)) width=(f(ROUTER_W)) height=(f(ROUTER_H)) rx="12" {}
        image href=(assets::href("logo-mark.png")) x="-52" y="-13" width="26" height="26" {}
        text class="topology__name topology__name--router" x="-18" y="6" { "0Router" }
    };
    let (router, router_card) =
        hot(0, "topology__hot", router, tip(0.0, (-ROUTER_H / 2.0, ROUTER_H / 2.0), 0.0, bounds, "0Router", &window, &rows));
    cards.extend(router_card);

    html! {
        svg class="topology" viewBox=(format!("{:.0} {:.0} {:.0} {:.0}", -half_w, -half_h, half_w * 2.0, half_h * 2.0)) role="img" aria-label="Providers by requests" {
            defs { (card_shadow()) }
            @for (i, n) in nodes.iter().enumerate() {
                @let (cx, cy) = position(i, nodes.len());
                path class=(edge_class(n, newest)) d=(edge_path(cx, cy)) {}
            }
            @for (i, n) in nodes.iter().enumerate().filter(|(_, n)| n.requests > 0) {
                @let (cx, cy) = position(i, nodes.len());
                @let class = edge_class(n, newest).strip_prefix("topology__edge topology__edge").map_or(String::new(), |v| format!("topology__flow{v}"));
                (flow(&edge_path(cx, cy), &class, i))
            }
            (router)
            @for h in &hots { (h) }
            g class="landscape__cards" { @for c in &cards { (c) } }
        }
        p class="topology__label" { (LABEL) }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn usage() -> Value {
        json!({"providers": [{"id": "anthropic", "requests": 900}, {"id": "xai", "requests": 384}, {"id": "idle", "requests": 2}]})
    }

    fn latency() -> Value {
        json!({"providers": [
            {"id": "anthropic", "last": {"result": "resolved", "at": "2026-10-06T14:01:58Z", "status": null}},
            {"id": "xai", "last": {"result": "failed", "at": "2026-10-06T14:00:31Z", "status": 503}},
        ]})
    }

    #[test]
    fn providers_sit_on_an_ellipse_starting_at_the_top_clockwise() {
        let (x, y) = position(0, 4);
        assert!(x.abs() < 1e-9 && y < 0.0, "the first is straight above the router: {x} {y}");
        let (x1, y1) = position(1, 4);
        assert!(x1 > 0.0 && y1.abs() < 1e-9, "the second is to the right");
        let (rx, ry) = radii(3);
        assert_eq!((rx, ry), (MIN_RX, MIN_RY.max(MIN_RX * RATIO)), "few nodes use the minimum radii");
        let (big_rx, _) = radii(40);
        assert!(big_rx > MIN_RX, "many nodes widen the ellipse so neighbours keep their gap");
        let (ax, ay) = position(0, 40);
        let (bx, by) = position(1, 40);
        assert!(((ax - bx).powi(2) + (ay - by).powi(2)).sqrt() >= NODE_H, "neighbours don't overlap");
    }

    #[test]
    fn the_router_is_centred_and_there_are_no_agent_nodes() {
        let html = topology(&usage(), &latency(), &TimeZone::UTC, &Index::default()).into_string();
        assert!(html.contains("topology__router") && html.contains(">0Router<"));
        assert_eq!(html.matches("<title>").count(), 3, "one node per provider and nothing else: {html}");
        let layer = html.find("class=\"landscape__cards\"").unwrap();
        assert_eq!(html[layer..].matches("class=\"landscape__card landscape__card--").count(), 4, "a card per node and the router");
    }

    #[test]
    fn failed_is_red_the_newest_is_amber_and_the_rest_idle() {
        let html = topology(&usage(), &latency(), &TimeZone::UTC, &Index::default()).into_string();
        assert_eq!(html.matches("topology__edge--failed").count(), 1, "{html}");
        assert_eq!(html.matches("topology__edge--last").count(), 1, "{html}");
        let u = usage();
        let l = latency();
        let n = nodes(&u, &l);
        let newest = newest(&n);
        assert_eq!(newest, Some("2026-10-06T14:01:58Z"));
        assert_eq!(edge_class(&n[0], newest), "topology__edge topology__edge--last");
        assert_eq!(edge_class(&n[1], newest), "topology__edge topology__edge--failed");
        assert_eq!(edge_class(&n[2], newest), "topology__edge", "no last response: idle");
    }

    #[test]
    fn a_failed_newest_is_red_not_amber() {
        let l = json!({"providers": [{"id": "xai", "last": {"result": "failed", "at": "2026-10-06T15:00:00Z", "status": 503}}]});
        let u = json!({"providers": [{"id": "xai", "requests": 1}]});
        let html = topology(&u, &l, &TimeZone::UTC, &Index::default()).into_string();
        assert!(html.contains("topology__edge--failed") && !html.contains("topology__edge--last"), "{html}");
    }

    #[test]
    fn labels_print_the_counts_and_the_last_response() {
        let html = topology(&usage(), &latency(), &TimeZone::UTC, &Index::default()).into_string();
        for text in [
            ">900<",
            ">384<",
            ">2<",
            "900 req · resolved · 14:01",
            "384 req · failed · 14:00 · 503",
            "2 req · no response in 24 h",
            LABEL,
        ] {
            assert!(html.contains(text), "{text}\n{html}");
        }
    }

    #[test]
    fn no_script_or_external_reference_and_only_the_flow_moves() {
        let html = topology(&usage(), &latency(), &TimeZone::UTC, &Index::default()).into_string();
        for banned in ["<script", "<style", "xlink", "http://", "https://", "onclick", "onload"] {
            assert!(!html.contains(banned), "{banned}\n{html}");
        }
        assert!(!html.replace("url(#card-shadow)", "").contains("url("), "only the card shadow is referenced");
        assert_eq!(html.matches("<path class=\"topology__flow").count(), 3, "every edge with requests flows: {html}");
        for (at, _) in html.match_indices("<animate ") {
            assert!(html[at..].starts_with("<animate attributeName=\"stroke-dashoffset\""), "{}", &html[at..at + 60]);
        }
        for (at, _) in html.match_indices("href=\"") {
            let to = &html[at + 6..];
            assert!(to.starts_with("/assets/") || to.starts_with("/logos/"), "{}", &to[..40.min(to.len())]);
        }
    }

    #[test]
    fn no_providers_says_so() {
        let html = topology(&json!({"providers": []}), &json!({}), &TimeZone::UTC, &Index::default()).into_string();
        assert!(html.contains(EMPTY) && !html.contains("<svg"));
    }
}
