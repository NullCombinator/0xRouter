#!/usr/bin/env bash
# Regenerates the mockup pages. Sample data only. Navigation follows 9router's sidebar.
a() { [ "$1" = "$2" ] && echo on; }
top() { # title subtitle icon activeIdx
cat <<EOF
<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>$1 · 0Router Proxy (mockup)</title><link rel="icon" type="image/png" href="logo-mark.png"><link rel="stylesheet" href="mockup.css?v=$(date +%s)"></head><body>
<input type="checkbox" id="nav"><label class="ov" for="nav"></label>
<aside><div class="logo"><div class="t"><img src="logo-mark.png" alt="0Router"></div><div><b>0Router Proxy</b><small>v0.7.0</small></div></div>
<nav>
<a href="endpoint.html" class="$(a 1 $4)"><span class="i">api</span>Endpoint &amp; Key</a>
<a href="providers.html" class="$(a 2 $4)"><span class="i">dns</span>Providers</a>
<a href="combos.html" class="$(a 3 $4)"><span class="i">layers</span>Combo</a>
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
bot() { local P=""; [ -n "$RS" ] && P='<input type="checkbox" id="rsb"><label for="rsb" class="rsov"></label><label for="rsb" class="rstab" title="Customize this page"><span class="i">chevron_left</span></label>'; echo "</div></main>${P}${RS}${MODALS}${HK}<div class=\"mock\">MOCKUP · sample data</div></body></html>"; RS=""; MODALS=""; }

prow() { echo "<div class=\"prow\">$(logo "$1")<div style=\"min-width:0\"><b>$1</b><small>$2</small></div><span class=\"badge $4 sp\" style=\"font-size:10px\">$3</span></div>"; }
tg() { echo "<span class=\"tg $1\"><i></i></span>"; }
mock_note() { echo "<div class=\"mnote\">MOCKUP NOTE: these controls need a write path. 007 is look-only (brief, FR-010), so they stay a design until you decide.</div>"; }
panel_head() { echo "<div class=\"rh\"><span class=\"i\">$1</span><b>$2</b><label for=\"rsb\" class=\"x\"><span class=\"i\">chevron_right</span></label></div><div class=\"srow\"><div class=\"search\" style=\"flex:1;width:auto\"><span class=\"i\" style=\"font-size:16px\">search</span>$3</div><label for=\"$4\" class=\"gear\" title=\"Settings\"><span class=\"i\">settings</span></label></div>"; }
MODALS=""; PM=0
HK='<input type="checkbox" id="hkc"><div class="hkp"><div class="hkh"><span class="i">smart_toy</span><div><b>Housekeeping agent</b><small>native agent · runs inside the router</small></div><label for="hkc" class="mx"><span class="i">close</span></label></div><div class="hkb"><div class="hkm">Pruned 214 request records older than 30 days.<small>14:00 · sample</small></div><div class="hkm">anthropic / work needs sign-in again.<small>13:42 · sample</small></div><div class="hkm">opencode-zen quota is estimated, not polled.<small>13:10 · sample</small></div></div><div class="hkf"><div class="search"><span class="i" style="font-size:16px">chat</span>Ask the housekeeping agent...</div></div></div><label for="hkc" class="hk" title="Router housekeeping agent"><span class="i">smart_toy</span><i class="hkd"></i></label>'
cardprov() { PM=$((PM+1)); MODALS+="$(python3 modals.py prov "$1" "$2" pm$PM)"; echo "<label for=pm$PM class=\"pcard$3\">$(logo $1)<div><b>$1</b><small>$2</small></div></label>"; }
cardagent() { PM=$((PM+1)); MODALS+="$(python3 modals.py agent "$1" "$2" pm$PM)"; echo "<label for=pm$PM class=\"pcard$5\" style=\"align-items:flex-start\">$(avatar "$1")<div style=\"min-width:0\"><b>$1</b><span class=\"hp\">$2</span><small style=\"display:block;margin:4px 0 6px\" class=\"mono\">$3</small><span class=\"badge $4\" style=\"font-size:10px;padding:0 8px\">$6</span><small style=\"display:block;margin-top:6px\">$7</small></div></label>"; }
ps() { echo "<div class=\"ps\"><h5>$1</h5>$2</div>"; }
act() { echo "<div class=\"act\"><div><b>$1</b><small>$2</small></div><span class=\"badge $3\" style=\"font-size:10px\">$4</span></div>"; }
chk() { echo "<label class=\"chk\"><span class=\"cb $2\">$([ "$2" = on ] && echo '<span class="i">check</span>')</span>$1<span class=\"m sp\" style=\"font-size:12px\">$3</span></label>"; }
ch() { echo "<div class=\"ch\"><div class=\"tile\"><span class=\"i\">$1</span></div><div><h3>$2</h3><p>$3</p></div>$4</div>"; }
# deterministic "random" avatar from a name: same name, same picture, nothing fetched
avatar() { local h=$(( $(printf %s "$1" | cksum | cut -d' ' -f1) )); local a=$((h%360)) b=$(((h/7)%360)) r=$((h%5*20+10)); echo "<div class=\"av\" style=\"background:radial-gradient(circle at ${r}% 25%,hsl($a 85% 70%),transparent 60%),radial-gradient(circle at 80% 85%,hsl($b 80% 62%),transparent 55%),hsl($a 60% 90%)\"></div>"; }
logo() { python3 -c "import sys;from plugdata import color,icon;n=sys.argv[1];c=color(n);print(f'<div class=\"lg\" style=\"border-radius:6px;background:{c}26;color:{c}\">{icon(n)}</div>')" "$1"; }

{ top "Endpoint" "API endpoint configuration" api 1
echo '<div class="plain">'; ch hub "Agent traffic" "Each color is one agent. Dashed: agent to router. Solid: router to provider. Dial: latency of that hop; hover for the numbers." '<span class="badge neu r">as of 14:02:11</span>'; python3 topo.py gauges; echo '<div class="legend"><span><i style="background:#E56A4A"></i>claude-code</span><span><i style="background:#3b82f6"></i>codex</span><span><i style="background:#10b981"></i>hermes-research</span><span><i style="background:#a855f7"></i>ci-bot</span></div></div>
<div class="card">'; ch api "API Endpoint" "Point your agents here" ""
echo '<div class="row"><span class="m mono" style="width:60px">Local</span><div class="inset mono" style="flex:1;display:flex;align-items:center">http://127.0.0.1:20129/v1<span class="i sp m" style="font-size:18px">content_copy</span></div></div></div>'
echo '<div class="secbar"><h2>Agents</h2><div class="g"><span class="m" style="font-size:13px;align-self:center">One key per agent, so each one is routed and recorded on its own</span>
<details class="add"><summary><span class="btn2 p"><span class="i">add</span>Add Agent</span></summary><div class="pop"><b>Add an agent</b><p class="m" style="margin:6px 0 10px;font-size:13px">Agents are added from the CLI. Run:</p><div class="inset mono" style="font-size:12px">nullrouter keys issue &lt;name&gt; --harness claude-code</div><p class="m" style="margin:10px 0 0;font-size:12px">harness: hermes, claude-code, codex, or other. Reload this page to see it.</p></div></details></div></div>
<div class="g3">'
for p in "claude-code|claude-code|nr_…9f2a|14:01|612|ok|enabled" "codex|codex|nr_…77c0|yesterday|88|ok|enabled" "hermes-research|hermes|nr_…c3d1|13:47|41|ok|enabled" "ci-bot|other|nr_…52aa|3 days ago|0|neu|idle" "old-laptop|claude-code|nr_…1be4|never|0|neu|revoked"; do IFS='|' read n hk k lu rq b t <<<"$p"; sel=""; [ "$n" = claude-code ] && [ "$k" = "nr_…9f2a" ] && sel=" sel"; cardagent "$n" "$hk" "$k" "$b" "$sel" "$t" "last used $lu · $rq requests today"; done
RS="<aside class=\"rs\">$(panel_head terminal 'Client adapters' 'Search adapters...' m2)$(ps 'Built in' "$(prow hermes 'core code · always on' live ok)")$(ps 'Third party · sandboxed' "$(prow example-claude 'sample · reviewed v1.2' live ok)$(prow example-codex 'sample · v2.0 awaiting review' 'v1.9 serving' warn)")$(ps 'How adapters work' '<div class=\"m\" style=\"font-size:12px;line-height:1.5\">An adapter handles one client harness quirks. A new version goes live only after you review it, and the previous version keeps serving meanwhile.</div>')</aside><input type=\"checkbox\" id=\"m2\" class=\"mt\"><div class=\"modal\"><label for=\"m2\" class=\"mo\"></label><div class=\"mb\"><div class=\"mh\"><b>Client adapter settings</b><label for=\"m2\" class=\"mx\"><span class=\"i\">close</span></label></div><div class=\"msc\"><table><tr><th>Adapter</th><th>Source</th><th>State</th><th>Enabled</th><th></th></tr>
<tr><td><b>hermes</b></td><td class=\"m\">built in</td><td><span class=\"badge ok\">live</span></td><td>$(tg on)</td><td></td></tr>
<tr><td><b>example-claude</b></td><td class=\"m\">third party</td><td><span class=\"badge ok\">live v1.2</span></td><td>$(tg on)</td><td><span class=\"btn2\">Remove</span></td></tr>
<tr><td><b>example-codex</b></td><td class=\"m\">third party</td><td><span class=\"badge warn\">v2.0 awaiting review</span></td><td>$(tg on)</td><td><span class=\"btn2 p\">Review</span></td></tr></table>
<div class=\"secbar\" style=\"margin-top:20px\"><h2 style=\"font-size:15px\">Add an adapter</h2></div><div class=\"inset m\" style=\"font-size:13px\">Adapters are Rust source built by the separate builder service. Paste a source address to submit it for review.</div>$(mock_note)</div><div class=\"mf\"><span class=\"btn2\">Cancel</span><span class=\"btn2 p\">Save</span></div></div></div>"
echo '</div><div class="card">'; ch lock "Require API key" "Requests without a valid key are rejected" '<span class="badge ok r">on</span>'; echo '</div>'
bot; } > endpoint.html

{ top "Providers" "Manage your AI provider connections" dns 2
echo '<div class="topr" style="justify-content:flex-end;margin:-16px 0 16px"><div class="search"><span class="i" style="font-size:16px">search</span>Search providers...</div><span class="btn2">All <span class="i">expand_more</span></span></div>'
echo '<div class="secbar"><h2>Custom Providers (OpenAI/Anthropic Compatible)</h2><div class="g"><span class="btn2 p"><span class="i">add</span>Add Anthropic Compatible</span><span class="btn2"><span class="i">add</span>Add OpenAI Compatible</span></div></div>
<div class="card" style="text-align:center;color:var(--muted);padding:14px;margin-bottom:28px"><span class="i" style="font-size:18px">extension</span> No custom providers — use buttons above to add compatible endpoints</div>'
echo '<div class="secbar"><h2>OAuth Providers</h2><div class="g"><span class="btn2"><span class="i">play_arrow</span>Test All</span></div></div><div class="g3">'
for p in "anthropic|2 connections" "xai|1 connection" "grok-cli|No connections"; do IFS='|' read n c <<<"$p"; sel=""; [ "$n" = anthropic ] && sel=" sel"; cardprov "$n" "$c" "$sel"; done; echo '</div>'
echo '<div class="secbar"><h2>Free Tier Providers</h2><div class="g"><span class="btn2"><span class="i">play_arrow</span>Test All</span></div></div><div class="g3">'
for p in "opencode-zen|Ready" "opencode-go|No connections"; do IFS='|' read n c <<<"$p"; cardprov "$n" "$c" ""; done; echo '</div>'
echo '<div class="secbar"><h2>API Key Providers</h2><div class="g"><span class="btn2"><span class="i">play_arrow</span>Test All</span></div></div><div class="g3">'
RS="<aside class=\"rs\">$(panel_head extension 'Provider plugins' 'Search plugins...' m1)$(ps 'Bundled · 7' "$(prow anthropic 'schema 2 · 3 models' loaded ok)$(prow xai 'schema 2 · 8 models' loaded ok)$(prow grok-cli 'schema 2 · 5 models' loaded ok)$(prow openrouter 'schema 2 · 17 models' loaded ok)$(prow opencode-go 'schema 2 · 28 models' loaded ok)$(prow opencode-zen 'schema 2 · 72 models' loaded ok)$(prow elevenlabs 'schema 2 · 5 models' loaded ok)")$(ps 'Installed from community · 2' "$(prow mistral 'community · 11 models' loaded ok)$(prow groq 'community · 8 models' hidden neu)")$(ps 'Community · not installed' '<div class=\"m\" style=\"font-size:12px\">114 available. Open settings to browse and install.</div>')</aside><input type=\"checkbox\" id=\"m1\" class=\"mt\"><div class=\"modal\"><label for=\"m1\" class=\"mo\"></label><div class=\"mb\"><div class=\"mh\"><b>Provider plugin settings</b><label for=\"m1\" class=\"mx\"><span class=\"i\">close</span></label></div><div class=\"msc\"><table><tr><th>Plugin</th><th>Source</th><th>Show on page</th><th>Enabled</th><th></th></tr>
<tr><td><b>anthropic</b></td><td class=\"m\">bundled</td><td>$(tg on)</td><td>$(tg on)</td><td><span class=\"btn2\">Update</span></td></tr>
<tr><td><b>xai</b></td><td class=\"m\">bundled</td><td>$(tg on)</td><td>$(tg on)</td><td><span class=\"btn2\">Update</span></td></tr>
<tr><td><b>openrouter</b></td><td class=\"m\">bundled</td><td>$(tg on)</td><td>$(tg on)</td><td><span class=\"btn2\">Update</span></td></tr>
<tr><td><b>mistral</b></td><td class=\"m\">community</td><td>$(tg on)</td><td>$(tg on)</td><td><span class=\"btn2\">Uninstall</span></td></tr>
<tr><td><b>groq</b></td><td class=\"m\">community</td><td>$(tg off)</td><td>$(tg on)</td><td><span class=\"btn2\">Uninstall</span></td></tr></table>
<div class=\"secbar\" style=\"margin-top:20px\"><h2 style=\"font-size:15px\">Install a community plugin</h2></div><div class=\"row\"><div class=\"search\" style=\"flex:1;width:auto\"><span class=\"i\" style=\"font-size:16px\">search</span>Search 114 community plugins...</div><span class=\"btn2 p\">Install</span></div>$(mock_note)</div><div class=\"mf\"><span class=\"btn2\">Cancel</span><span class=\"btn2 p\">Save</span></div></div></div>"
for p in "openrouter|1 connection · cooling" "elevenlabs|No connections" "anthropic|1 connection" "gemini|No connections" "mistral|No connections" "groq|No connections" "deepseek|No connections" "glm|1 connection"; do IFS='|' read n c <<<"$p"; cardprov "$n" "$c" ""; done; echo '</div>'
bot; } > providers.html

{ top "Combo" "Model combos with fallback" layers 3
echo '<div class="card"><div class="empty"><div class="tile"><span class="i" style="font-size:28px">layers</span></div><h3>Combos are not built yet</h3><p>Declare unified models today with <code>[[unified_model]]</code> in config.toml and list them with <code>nullrouter unified</code>.</p></div></div>'
bot; } > combos.html

{ top "Proxy Pools" "Outbound proxy relays" lan 7
echo '<div class="card"><div class="empty"><div class="tile"><span class="i" style="font-size:28px">lan</span></div><h3>Proxy pools are not built yet</h3><p>0router has no proxy pool feature today.</p></div></div>'
bot; } > proxy-pools.html

{ top "Console Log" "Live server log" terminal 8
echo '<div class="card"><div class="empty"><div class="tile"><span class="i" style="font-size:28px">terminal</span></div><h3>The console log is not built yet</h3><p>Request records are on the Usage page. Live logs come with live refresh, which is a later slice.</p></div></div>'
bot; } > console-log.html

{ top "Usage & Analytics" "Request records, token use and latency as recorded" bar_chart 4
python3 usage_head.py
echo '<div class="card">'; ch history "Requests" "Newest first, 50 at a time · same as nullrouter records list --limit 50" '<span class="badge neu r">no prompts or secrets</span>'
echo '<table><tr><th>When</th><th>Agent</th><th>Model → placed on</th><th>Why</th><th class="n">TTFT</th><th class="n">Total</th><th>Result</th></tr>
<tr><td class="m">14:01:58</td><td>claude-code</td><td><b>sonnet</b> → anthropic / personal</td><td class="m">warm cache</td><td class="n">412 ms</td><td class="n">3.1 s</td><td><span class="badge ok">served</span></td></tr>
<tr><td class="m">14:01:40</td><td>codex</td><td><b>sonnet</b> → openrouter / main</td><td class="m">pace deficit</td><td class="n">690 ms</td><td class="n">5.4 s</td><td><span class="badge ok">served</span></td></tr>
<tr><td class="m">14:01:12</td><td>hermes-research</td><td><b>sonnet</b> → openrouter / main<br><code>anthropic refused first</code></td><td class="m">fallback</td><td class="n">1.9 s</td><td class="n">6.0 s</td><td><span class="badge warn">fallback</span></td></tr></table></div>'
bot; } > usage.html

{ top "Quota Tracker" "Track your API quota limits" data_usage 5
python3 quota_page.py
bot; } > quota.html

{ top "Settings" "Read-only view of your configuration" settings 6
echo '<div class="card">'; ch route "Routing" "Same as nullrouter behaviour show" ""
echo '<div class="kv"><div><b>break_behaviour</b><small>What happens when a conversation cannot stay on its account</small></div><span class="badge neu">restart (default)</span></div></div>'
echo '<div class="card">'; ch folder "Local mode" "Where your data lives" ""; echo '<div class="inset mono">~/.0router</div></div>'
echo '<div class="card">'; ch monitor "Dashboard" "Same as nullrouter dashboard status" ""
echo '<div class="kv"><div><b>Listening</b></div><span class="mono">127.0.0.1:20130</span></div><div class="kv"><div><b>Token issued</b><small>Change it with nullrouter dashboard token</small></div><span class="mono">2026-10-05 13:50</span></div></div>'
bot; } > settings.html

sed -i 's/\\"/"/g' *.html
