#!/usr/bin/env bash
# Regenerates the mockup pages. Sample data only. Navigation follows 9router's sidebar.
a() { [ "$1" = "$2" ] && echo on; }
top() { # title subtitle icon activeIdx
cat <<EOF
<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>$1 · 0Router Proxy (mockup)</title><link rel="stylesheet" href="mockup.css"></head><body>
<input type="checkbox" id="nav"><label class="ov" for="nav"></label>
<aside><div class="logo"><div class="t"><span class="i">hub</span></div><div><b>0Router Proxy</b><small>v0.7.0</small></div></div>
<nav>
<a href="endpoint.html" class="$(a 1 $4)"><span class="i">api</span>Endpoint &amp; Key</a>
<a href="providers.html" class="$(a 2 $4)"><span class="i">dns</span>Providers</a>
<a href="combos.html" class="$(a 3 $4)"><span class="i">layers</span>Combo &amp; Vision Adapter</a>
<a href="usage.html" class="$(a 4 $4)"><span class="i">bar_chart</span>Usage</a>
<a href="quota.html" class="$(a 5 $4)"><span class="i">data_usage</span>Quota Tracker</a>
<h6>System</h6>
<a href="proxy-pools.html" class="$(a 7 $4)"><span class="i">lan</span>Proxy Pools</a>
<a href="console-log.html" class="$(a 8 $4)"><span class="i">terminal</span>Console Log</a>
<a href="settings.html" class="$(a 6 $4)"><span class="i">settings</span>Settings</a></nav>
</aside>
<main><div class="bar-top"><div class="pt"><label for="nav" class="menu"><span class="i">menu</span></label><span class="i">$3</span><div><h1>$1</h1><p>$2</p></div></div><span class="asof"><span class="i" style="font-size:16px">schedule</span>as of 14:02:11 · reload to refresh</span></div><div class="col">
EOF
}
bot() { echo '</div></main><div class="mock">MOCKUP · sample data</div></body></html>'; }
ch() { echo "<div class=\"ch\"><div class=\"tile\"><span class=\"i\">$1</span></div><div><h3>$2</h3><p>$3</p></div>$4</div>"; }
# deterministic "random" avatar from a name: same name, same picture, nothing fetched
avatar() { local h=$(( $(printf %s "$1" | cksum | cut -d' ' -f1) )); local a=$((h%360)) b=$(((h/7)%360)) r=$((h%5*20+10)); echo "<div class=\"av\" style=\"background:radial-gradient(circle at ${r}% 25%,hsl($a 85% 70%),transparent 60%),radial-gradient(circle at 80% 85%,hsl($b 80% 62%),transparent 55%),hsl($a 60% 90%)\"></div>"; }
logo() { local h=$(( $(printf %s "$1" | cksum | cut -d' ' -f1) % 360 )); echo "<div class=\"lg\" style=\"background:hsl($h 80% 94%);color:hsl($h 55% 42%)\">${1:0:1}</div>"; }

{ top "Endpoint" "API endpoint configuration" api 1
echo '<div class="card">'; ch api "API Endpoint" "Point your agents here" ""
echo '<div class="row"><span class="m mono" style="width:60px">Local</span><div class="inset mono" style="flex:1;display:flex;align-items:center">http://127.0.0.1:20129/v1<span class="i sp m" style="font-size:18px">content_copy</span></div></div></div>'
echo '<div class="secbar"><h2>Agents</h2><div class="g"><span class="m" style="font-size:13px;align-self:center">One key per agent, so each one is routed and recorded on its own</span>
<details class="add"><summary><span class="btn2 p"><span class="i">add</span>Add Agent</span></summary><div class="pop"><b>Add an agent</b><p class="m" style="margin:6px 0 10px;font-size:13px">Agents are added from the CLI. Run:</p><div class="inset mono" style="font-size:12px">nullrouter keys issue &lt;name&gt; --harness claude-code</div><p class="m" style="margin:10px 0 0;font-size:12px">harness: hermes, claude-code, codex, or other. Reload this page to see it.</p></div></details></div></div>
<div class="g3">'
for p in "claude-code|claude-code|nr_…9f2a|14:01|612|ok|enabled" "codex|codex|nr_…77c0|yesterday|88|ok|enabled" "hermes-research|hermes|nr_…c3d1|13:47|41|ok|enabled" "ci-bot|other|nr_…52aa|3 days ago|0|neu|idle" "old-laptop|claude-code|nr_…1be4|never|0|neu|revoked"; do IFS='|' read n hk k lu rq b t <<<"$p"; echo "<div class=\"pcard\" style=\"align-items:flex-start\">$(avatar "$n")<div style=\"min-width:0\"><b>$n</b><span class=\"hp\">$hk</span><small style=\"display:block;margin:4px 0 6px\" class=\"mono\">$k</small><span class=\"badge $b\" style=\"font-size:10px;padding:0 8px\">$t</span><small style=\"display:block;margin-top:6px\">last used $lu · $rq requests today</small></div></div>"; done
echo '</div><div class="card">'; ch lock "Require API key" "Requests without a valid key are rejected" '<span class="badge ok r">on</span>'; echo '</div>'
bot; } > endpoint.html

{ top "Providers" "Manage your AI provider connections" dns 2
echo '<div class="topr" style="justify-content:flex-end;margin:-16px 0 16px"><div class="search"><span class="i" style="font-size:16px">search</span>Search providers...</div><span class="btn2">All <span class="i">expand_more</span></span></div>'
echo '<div class="secbar"><h2>Custom Providers (OpenAI/Anthropic Compatible)</h2><div class="g"><span class="btn2 p"><span class="i">add</span>Add Anthropic Compatible</span><span class="btn2"><span class="i">add</span>Add OpenAI Compatible</span></div></div>
<div class="card" style="text-align:center;color:var(--muted);padding:14px;margin-bottom:28px"><span class="i" style="font-size:18px">extension</span> No custom providers — use buttons above to add compatible endpoints</div>'
echo '<div class="secbar"><h2>OAuth Providers</h2><div class="g"><span class="btn2"><span class="i">play_arrow</span>Test All</span></div></div><div class="g3">'
for p in "anthropic|2 connections" "xai|1 connection" "grok-cli|No connections"; do IFS='|' read n c <<<"$p"; echo "<div class=\"pcard\">$(logo $n)<div><b>$n</b><small>$c</small></div></div>"; done; echo '</div>'
echo '<div class="secbar"><h2>Free Tier Providers</h2><div class="g"><span class="btn2"><span class="i">play_arrow</span>Test All</span></div></div><div class="g3">'
for p in "opencode-zen|Ready" "opencode-go|No connections"; do IFS='|' read n c <<<"$p"; echo "<div class=\"pcard\">$(logo $n)<div><b>$n</b><small>$c</small></div></div>"; done; echo '</div>'
echo '<div class="secbar"><h2>API Key Providers</h2><div class="g"><span class="btn2"><span class="i">play_arrow</span>Test All</span></div></div><div class="g3">'
for p in "openrouter|1 connection · cooling" "elevenlabs|No connections" "anthropic|1 connection" "gemini|No connections" "mistral|No connections" "groq|No connections" "deepseek|No connections" "glm|1 connection"; do IFS='|' read n c <<<"$p"; echo "<div class=\"pcard\">$(logo $n)<div><b>$n</b><small>$c</small></div></div>"; done; echo '</div>'
bot; } > providers.html

{ top "Combo & Vision Adapter" "Model combos with fallback" layers 3
echo '<div class="card"><div class="empty"><div class="tile"><span class="i" style="font-size:28px">layers</span></div><h3>Combos are not built yet</h3><p>Declare unified models today with <code>[[unified_model]]</code> in config.toml and list them with <code>nullrouter unified</code>.</p></div></div>'
bot; } > combos.html

{ top "Proxy Pools" "Outbound proxy relays" lan 7
echo '<div class="card"><div class="empty"><div class="tile"><span class="i" style="font-size:28px">lan</span></div><h3>Proxy pools are not built yet</h3><p>0router has no proxy pool feature today.</p></div></div>'
bot; } > proxy-pools.html

{ top "Console Log" "Live server log" terminal 8
echo '<div class="card"><div class="empty"><div class="tile"><span class="i" style="font-size:28px">terminal</span></div><h3>The console log is not built yet</h3><p>Request records are on the Usage page. Live logs come with live refresh, which is a later slice.</p></div></div>'
bot; } > console-log.html

{ top "Usage & Analytics" "Request records, token use and latency as recorded" bar_chart 4
echo '<div class="tabs"><span class="on">Overview</span><span>Details</span></div>
<div class="tiles"><div class="tl"><small>Total requests</small><b>1,284</b></div><div class="tl"><small>Input tokens</small><b style="color:#3b82f6">4.2M</b></div><div class="tl"><small>Cached tokens</small><b style="color:var(--brand-500)">2.9M</b></div><div class="tl"><small>Output tokens</small><b style="color:#16a34a">611k</b></div></div>
<div class="card">'; ch history "Requests" "Newest first, 50 at a time · same as nullrouter records list --limit 50" '<span class="badge neu r">no prompts or secrets</span>'
echo '<table><tr><th>When</th><th>Agent</th><th>Model → placed on</th><th>Why</th><th class="n">TTFT</th><th class="n">Total</th><th>Result</th></tr>
<tr><td class="m">14:01:58</td><td>claude-code</td><td><b>sonnet</b> → anthropic / personal</td><td class="m">warm cache</td><td class="n">412 ms</td><td class="n">3.1 s</td><td><span class="badge ok">served</span></td></tr>
<tr><td class="m">14:01:40</td><td>codex</td><td><b>sonnet</b> → openrouter / main</td><td class="m">pace deficit</td><td class="n">690 ms</td><td class="n">5.4 s</td><td><span class="badge ok">served</span></td></tr>
<tr><td class="m">14:01:12</td><td>hermes-research</td><td><b>sonnet</b> → openrouter / main<br><code>anthropic refused first</code></td><td class="m">fallback</td><td class="n">1.9 s</td><td class="n">6.0 s</td><td><span class="badge warn">fallback</span></td></tr></table></div>'
bot; } > usage.html

{ top "Quota Tracker" "Track your API quota limits" data_usage 5
for q in "anthropic / personal|62|polled 13:58|resets in 3 h 12 m (17:14)|ok|active" "xai / team|91|polled 14:01|resets in 41 m (14:43)|warn|near limit" "openrouter / main|18|estimated|resets in 2 d (Oct 7)|neu|cooling"; do IFS='|' read n p s r k t <<<"$q"; echo "<div class=\"card\"><div class=\"row\"><b>$n</b><span class=\"badge $k\">$t</span><span class=\"m sp\">$p% used · $s</span></div><div class=\"bar\" style=\"margin:12px 0 8px\"><i style=\"width:$p%\"></i></div><code>$r</code></div>"; done
bot; } > quota.html

{ top "Settings" "Read-only view of your configuration" settings 6
echo '<div class="card">'; ch route "Routing" "Same as nullrouter behaviour show" ""
echo '<div class="kv"><div><b>break_behaviour</b><small>What happens when a conversation cannot stay on its account</small></div><span class="badge neu">restart (default)</span></div></div>'
echo '<div class="card">'; ch folder "Local mode" "Where your data lives" ""; echo '<div class="inset mono">~/.0router</div></div>'
echo '<div class="card">'; ch monitor "Dashboard" "Same as nullrouter dashboard status" ""
echo '<div class="kv"><div><b>Listening</b></div><span class="mono">127.0.0.1:20130</span></div><div class="kv"><div><b>Token issued</b><small>Change it with nullrouter dashboard token</small></div><span class="mono">2026-10-05 13:50</span></div></div>'
bot; } > settings.html
