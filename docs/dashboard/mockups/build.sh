#!/usr/bin/env bash
# Regenerates the mockup pages. Sample data only.
top() { # file title subtitle icon activeIdx
cat <<EOF
<!doctype html><html lang="en"><head><meta charset="utf-8"><title>$2 · 0router (mockup)</title><link rel="stylesheet" href="mockup.css"></head><body>
<aside><div class="logo"><div class="t"><span class="i">hub</span></div><div><b>0router</b><small>v0.7.0 · read only</small></div></div>
<nav>
<a href="endpoint.html" class="$(a 1 $5)"><span class="i">api</span>Endpoint &amp; Key</a>
<a href="providers.html" class="$(a 2 $5)"><span class="i">dns</span>Providers</a>
<a href="combos.html" class="$(a 3 $5)"><span class="i">layers</span>Combo &amp; Vision Adapter<span class="soon">soon</span></a>
<a href="usage.html" class="$(a 4 $5)"><span class="i">bar_chart</span>Usage</a>
<a href="quota.html" class="$(a 5 $5)"><span class="i">data_usage</span>Quota Tracker</a>
<h6>System</h6>
<a href="settings.html" class="$(a 6 $5)"><span class="i">settings</span>Settings</a></nav>
<div class="foot">Local mode · all data stays on this machine</div></aside>
<main><div class="bar-top"><div class="pt"><span class="i">$4</span><div><h1>$2</h1><p>$3</p></div></div><span class="asof"><span class="i" style="font-size:16px">schedule</span>as of 14:02:11 · reload to refresh</span></div><div class="col">
EOF
}
a() { [ "$1" = "$2" ] && echo on; }
bot() { echo '</div></main><div class="mock">MOCKUP · sample data · navigation follows 9router</div></body></html>'; }
ch() { echo "<div class=\"ch\"><div class=\"tile\"><span class=\"i\">$1</span></div><div><h3>$2</h3><p>$3</p></div>$4</div>"; }

{ top x "Endpoint" "API endpoint configuration" api 1
echo '<div class="card">'; ch api "API Endpoint" "Point your clients here" ""
echo '<div class="row"><span class="m mono" style="width:60px">Local</span><div class="inset mono" style="flex:1;display:flex;align-items:center">http://127.0.0.1:20129/v1<span class="i sp m" style="font-size:18px">content_copy</span></div></div></div>'
echo '<div class="card">'; ch key "API Keys" "Same as nullrouter keys list" '<span class="badge neu r">issued from the CLI</span>'
echo '<div class="kv"><div><b>Require API key</b><small>Requests without a valid key are rejected</small></div><span class="badge ok">on</span></div>'
echo '<div class="kv"><div><b>claude-code</b><small class="mono">nr_…9f2a · created Oct 1 · last used 14:01</small></div><span class="badge ok">enabled</span></div>'
echo '<div class="kv"><div><b>codex</b><small class="mono">nr_…77c0 · created Oct 3 · last used yesterday</small></div><span class="badge ok">enabled</span></div>'
echo '<div class="kv"><div><b>old-laptop</b><small class="mono">nr_…1be4 · created Sep 20 · never used</small></div><span class="badge neu">revoked</span></div></div>'
bot; } > endpoint.html

{ top x "Providers" "Manage your AI provider connections" dns 2
echo '<div class="sec"><span class="i" style="color:var(--brand-500)">verified_user</span>Connected</div><div class="grid4" style="margin-bottom:20px">'
for p in "kiro|K|2 accounts|1 needs sign-in|err" "openrouter|O|1 account|cooling|warn" "openai-codex|C|1 account|active|ok" "glm|G|1 account|pay-as-you-go|neu"; do IFS='|' read n l c s k <<<"$p"; echo "<div class=\"pc\"><div class=\"d\" style=\"background:var(--brand-50);color:var(--brand-500)\">$l</div><div><b>$n</b><small>$c</small><br><span class=\"badge $k\" style=\"font-size:10px;padding:0 8px\">$s</span></div></div>"; done
echo '</div><div class="card">'; ch group "Accounts" "Same as nullrouter accounts list --long" ""
echo '<table><tr><th>Provider / name</th><th>Auth</th><th>Status</th><th class="n">Priority</th></tr>
<tr><td><b>kiro</b> / personal</td><td class="m">OAuth</td><td><span class="badge ok">active</span></td><td class="n m">1</td></tr>
<tr><td><b>kiro</b> / work-kiro</td><td class="m">OAuth</td><td><span class="badge err">needs sign-in</span><br><code>nullrouter accounts login kiro work-kiro</code></td><td class="n m">2</td></tr>
<tr><td><b>openrouter</b> / main</td><td class="m">key <code>…a91f</code></td><td><span class="badge warn">cooling</span> <code>until 14:09</code></td><td class="n m">1</td></tr>
<tr><td><b>openai-codex</b> / team</td><td class="m">OAuth</td><td><span class="badge ok">active</span></td><td class="n m">1</td></tr>
<tr><td><b>glm</b> / env</td><td class="m"><code>env:GLM_KEY</code></td><td><span class="badge neu">pay-as-you-go</span></td><td class="n m">3</td></tr></table></div>'
echo '<div class="sec"><span class="i" style="color:var(--brand-500)">extension</span>Available plugins</div><div class="grid4">'
for n in anthropic gemini-cli github-copilot cursor mistral groq; do echo "<div class=\"pc\"><div class=\"d\" style=\"background:var(--surface-2);color:var(--muted)\">${n:0:1}</div><div><b>$n</b><small>No connections</small></div></div>"; done; echo '</div>'
bot; } > providers.html

{ top x "Combo & Vision Adapter" "Model combos with fallback" layers 3
echo '<div class="card"><div class="empty"><div class="tile"><span class="i" style="font-size:28px">layers</span></div><h3>Combos are not built yet</h3><p>Declare unified models today with <code>[[unified_model]]</code> in config.toml and list them with <code>nullrouter unified</code>.</p></div></div>'
bot; } > combos.html

{ top x "Usage & Analytics" "Request records, token use and latency as recorded" bar_chart 4
echo '<div class="tabs"><span class="on">Overview</span><span>Details</span></div>
<div class="tiles"><div class="tl"><small>Total requests</small><b>1,284</b></div><div class="tl"><small>Input tokens</small><b style="color:#3b82f6">4.2M</b></div><div class="tl"><small>Cached tokens</small><b style="color:var(--brand-500)">2.9M</b></div><div class="tl"><small>Output tokens</small><b style="color:#16a34a">611k</b></div></div>
<div class="card">'; ch history "Requests" "Newest first, 50 at a time · same as nullrouter records list --limit 50" '<span class="badge neu r">no prompts or secrets</span>'
echo '<table><tr><th>When</th><th>Model → placed on</th><th>Why</th><th class="n">TTFT</th><th class="n">Total</th><th class="n">In / out</th><th>Result</th></tr>
<tr><td class="m">14:01:58</td><td><b>sonnet</b> → kiro / personal</td><td class="m">warm cache</td><td class="n">412 ms</td><td class="n">3.1 s</td><td class="n m">1,204 / 388</td><td><span class="badge ok">served</span></td></tr>
<tr><td class="m">14:01:40</td><td><b>sonnet</b> → openai-codex / team</td><td class="m">pace deficit</td><td class="n">690 ms</td><td class="n">5.4 s</td><td class="n m">8,330 / 902</td><td><span class="badge ok">served</span></td></tr>
<tr><td class="m">14:01:12</td><td><b>sonnet</b> → openrouter / main<br><code>kiro refused first</code></td><td class="m">fallback</td><td class="n">1.9 s</td><td class="n">6.0 s</td><td class="n m">930 / 210</td><td><span class="badge warn">fallback</span></td></tr>
<tr><td class="m">14:00:31</td><td><b>opus</b> → kiro / work-kiro</td><td class="m">—</td><td class="n m">—</td><td class="n">0.2 s</td><td class="n m">—</td><td><span class="badge err">refused</span></td></tr></table></div>
<div class="card">'; ch alt_route "Routing view" "Same as nullrouter routing" ""
echo '<table><tr><th>Account</th><th class="n">Pace</th><th class="n">Share</th><th class="n">Deficit</th></tr><tr><td><b>kiro / personal</b></td><td class="n">0.58</td><td class="n">0.62</td><td class="n m">−0.04</td></tr><tr><td><b>openai-codex / team</b></td><td class="n">0.40</td><td class="n">0.91</td><td class="n m">−0.51</td></tr></table></div>'
bot; } > usage.html

{ top x "Quota Tracker" "Track your API quota limits" data_usage 5
for q in "kiro / personal|62|polled 13:58|resets in 3 h 12 m (17:14)|ok|active" "openai-codex / team|91|polled 14:01|resets in 41 m (14:43)|warn|near limit" "openrouter / main|18|estimated|resets in 2 d (Oct 7)|neu|cooling"; do IFS='|' read n p s r k t <<<"$q"; echo "<div class=\"card\"><div class=\"row\"><b>$n</b><span class=\"badge $k\">$t</span><span class=\"m sp\">$p% used · $s</span></div><div class=\"bar\" style=\"margin:12px 0 8px\"><i style=\"width:$p%\"></i></div><code>$r</code></div>"; done
echo '<div class="card"><div class="row"><b>kiro / work-kiro</b><span class="badge err">needs sign-in</span><span class="m sp">quota unknown</span></div></div>'
bot; } > quota.html

{ top x "Settings" "Read-only view of your configuration" settings 6
echo '<div class="card">'; ch route "Routing" "Same as nullrouter behaviour show" ""
echo '<div class="kv"><div><b>break_behaviour</b><small>What happens when a conversation cannot stay on its account</small></div><span class="badge neu">restart (default)</span></div></div>'
echo '<div class="card">'; ch folder "Local mode" "Where your data lives" ""
echo '<div class="inset mono">~/.0router</div></div>'
echo '<div class="card">'; ch monitor "Dashboard" "Same as nullrouter dashboard status" ""
echo '<div class="kv"><div><b>Listening</b></div><span class="mono">127.0.0.1:20130</span></div><div class="kv"><div><b>Token issued</b><small>Change it with nullrouter dashboard token</small></div><span class="mono">2026-10-05 13:50</span></div></div>'
bot; } > settings.html
