# Look review: dashboard summaries (T051)

Compared 2026-10-21 in Chrome against `docs/dashboard/mockups/endpoint.html` and `usage.html`,
with scripts off, on branch `look/mockup-parity`.

## Cards, filter, topology graph, landscape

- Stat cards: five columns as drawn; they no longer stretch to the tallest card.
- Period filter: a segmented control (Today, 24h, 7D, 30D, 60D, All) as drawn; links, no script.
- Topology graph: framed, the router with its logo mark in the centre, provider logos, edge
  colours for failed and newest. Departures: no animated edges, no zoom buttons (no script).
- Landscape: glass pipes with walls, shine and collars, one colour per agent, gradient avatars
  ringed in the agent's colour, provider logos, the router's logo mark, legend below. User asks of
  2026-10-21, done: a moving white sheen and bubbles in the liquid, from agent to router to
  provider (SVG animation as the mockup's, no script; hidden under `prefers-reduced-motion`);
  pressure-gauge dials on a red-only scale; mockup text sizes (names 16 px, tags 12 px); node
  boxes turn white on hover; hovering a gauge or a node shows a card over the whole drawing with
  that hop's numbers. Each dial prints its median inside and marks its 95th percentile on the
  scale; the request count is on the node. Open for the user: the 95th-percentile number now
  shows only in the hover cards, where FR-027 says hover shows nothing the page doesn't print
  elsewhere.

- Usage, user asks of 2026-10-21, done: model names in Recent Requests are black; topology nodes
  at the mockup's size (logo, name, "N req · last response"); moving dashes and dots on each edge
  with requests in the period (no in-flight state is in any view, so this marks traffic in the
  period, not live requests); hovering a node or the router shows its card (requests, own TTFT
  p50/p95, last response; the router lists requests by provider).

## SC-009

Endpoint & Key alone shows which agents sent traffic (coloured pipes into the router), to which
providers (lanes out of it), and which hop is slowest (latency over each dial).

## Verdict

SC-008 / SC-009: _not yet judged by the user._
