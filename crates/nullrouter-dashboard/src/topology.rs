//! The provider topology graph on Usage (spec 010 US4, research R10), after 9router's
//! `ProviderTopology` (`usage/components/ProviderTopology.js`): the router in the centre and the
//! providers on an ellipse around it, no agent nodes. Inline SVG drawn on the server, with no
//! script and no animation. The edge to a provider is red when its last response failed, amber on
//! the provider with the newest last response, and the border colour otherwise (`edgeStyle`,
//! `:294`). Each node prints its request count and its last response, so a colour never carries a
//! fact alone.

use jiff::tz::TimeZone;
use maud::{Markup, html};
use serde_json::Value;

use crate::time::local_text;

pub const LABEL: &str = "last response · last 24 h";
pub const NONE: &str = "none in the last 24 h";
pub const EMPTY: &str = "No providers in this period";

// `buildLayout` (`ProviderTopology.js:263`): node 180x30, router 120x44, gap 24.
const NODE_W: f64 = 180.0;
const NODE_H: f64 = 30.0;
const ROUTER_W: f64 = 120.0;
const ROUTER_H: f64 = 44.0;
const NODE_GAP: f64 = 24.0;
const MIN_RX: f64 = 320.0;
const MIN_RY: f64 = 200.0;
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

/// The graph; the empty text instead when no provider had a request in the period.
pub fn topology(usage: &Value, latency: &Value, tz: &TimeZone) -> Markup {
    let nodes = nodes(usage, latency);
    if nodes.is_empty() {
        return html! { p class="landscape__empty" { (EMPTY) } };
    }
    let (rx, ry) = radii(nodes.len());
    let (half_w, half_h) = (rx + NODE_W / 2.0 + MARGIN, ry + NODE_H / 2.0 + MARGIN);
    let newest = newest(&nodes);
    html! {
        svg class="topology" viewBox=(format!("{:.0} {:.0} {:.0} {:.0}", -half_w, -half_h, half_w * 2.0, half_h * 2.0)) role="img" aria-label="Providers by requests" {
            @for (i, n) in nodes.iter().enumerate() {
                @let (cx, cy) = position(i, nodes.len());
                @let class = edge_class(n, newest);
                path class=(class) d=(format!("M0,0 L{cx:.1},{cy:.1}")) {}
            }
            rect class="topology__router" x=(format!("{:.0}", -ROUTER_W / 2.0)) y=(format!("{:.0}", -ROUTER_H / 2.0)) width=(format!("{ROUTER_W:.0}")) height=(format!("{ROUTER_H:.0}")) rx="10" {}
            text class="topology__name" x="0" y="5" text-anchor="middle" { "0router" }
            @for (i, n) in nodes.iter().enumerate() {
                @let (cx, cy) = position(i, nodes.len());
                @let class = node_class(edge_class(n, newest));
                @let last = last_text(n.last, tz);
                g {
                    title { (n.id) }
                    rect class=(class) x=(format!("{:.1}", cx - NODE_W / 2.0)) y=(format!("{:.1}", cy - NODE_H / 2.0)) width=(format!("{NODE_W:.0}")) height=(format!("{NODE_H:.0}")) rx="8" {}
                    text class="topology__name" x=(format!("{:.1}", cx - NODE_W / 2.0 + 10.0)) y=(format!("{:.1}", cy + 5.0)) { (n.id) }
                    text class="topology__count" x=(format!("{:.1}", cx + NODE_W / 2.0 - 10.0)) y=(format!("{:.1}", cy + 5.0)) text-anchor="end" { (n.requests) }
                    text class="topology__last" x=(format!("{cx:.1}")) y=(format!("{:.1}", cy + NODE_H / 2.0 + 12.0)) text-anchor="middle" {
                        "last response " (last.as_deref().unwrap_or(NONE))
                    }
                }
            }
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
        let html = topology(&usage(), &latency(), &TimeZone::UTC).into_string();
        assert!(html.contains("topology__router") && html.contains(">0router<"));
        assert_eq!(html.matches("<title>").count(), 3, "one node per provider and nothing else: {html}");
    }

    #[test]
    fn failed_is_red_the_newest_is_amber_and_the_rest_idle() {
        let html = topology(&usage(), &latency(), &TimeZone::UTC).into_string();
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
        let html = topology(&u, &l, &TimeZone::UTC).into_string();
        assert!(html.contains("topology__edge--failed") && !html.contains("topology__edge--last"), "{html}");
    }

    #[test]
    fn labels_print_the_counts_and_the_last_response() {
        let html = topology(&usage(), &latency(), &TimeZone::UTC).into_string();
        for text in [
            ">900<",
            ">384<",
            ">2<",
            "last response resolved · 14:01",
            "last response failed · 14:00 · 503",
            "last response none in the last 24 h",
            LABEL,
        ] {
            assert!(html.contains(text), "{text}\n{html}");
        }
    }

    #[test]
    fn no_animation_script_or_external_reference() {
        let html = topology(&usage(), &latency(), &TimeZone::UTC).into_string();
        for banned in ["<script", "<animate", "<style", "href=", "xlink", "http://", "https://", "onclick", "onload"] {
            assert!(!html.contains(banned), "{banned}\n{html}");
        }
    }

    #[test]
    fn no_providers_says_so() {
        let html = topology(&json!({"providers": []}), &json!({}), &TimeZone::UTC).into_string();
        assert!(html.contains(EMPTY) && !html.contains("<svg"));
    }
}
