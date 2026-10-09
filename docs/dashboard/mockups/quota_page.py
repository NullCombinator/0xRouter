#!/usr/bin/env python3
"""Quota Tracker in 9router's layout (ProviderLimits): filter row, two-column grid of compact account cards,
thin bars coloured by REMAINING % (>70 green, 30-70 yellow, <30 red), pagination footer. Look-only: no toggle,
edit, delete, refresh or bulk buttons. 0router adds polled/estimated, priority, and the needs-sign-in state."""
from plugdata import color, icon, logo_html
def col(r): return ("g","🟢") if r>70 else ("y","🟡") if r>=30 else ("r","🔴")
def row(name,used,total,reset,unl=False):
    if unl:
        return (f'<div class="qr"><div class="qn"><span class="e">🟢</span><span>{name}</span></div><div class="qm"><div class="qs"><span>{used:,} used · Unlimited</span><b class="g">Unlimited</b></div></div><div class="qt">{reset}</div></div>')
    rem=round(100*(total-used)/total); c,e=col(rem); z=" z" if rem==0 else ""
    return (f'<div class="qr"><div class="qn"><span class="e">{e}</span><span>{name}</span></div><div class="qm"><div class="qb {c}{z}"><i style="width:{rem}%"></i></div>'
            f'<div class="qs"><span>{used:,} / {total:,}</span><b class="{c}">{rem}%</b></div></div><div class="qt">{reset}</div></div>')
def card(prov,acct,state,badge,bk,rows,msg=None,prio=1,off=False):
    c=color(prov)
    body=(f'<div class="qx"><span class="i" style="color:#ef4444;font-size:28px">error</span><p>{msg}</p></div>' if msg
          else f'<div class="qcount">{len(rows)} quota{"s" if len(rows)>1 else ""}</div>'+"".join(rows))
    return (f'<div class="card qc{" off" if off else ""}"><div class="qh">{logo_html(prov,32,6)}'
            f'<div style="min-width:0"><h3>{prov}</h3><p>{acct}</p><p class="s">priority {prio}</p></div>'
            f'<div class="qbadge"><span class="badge {bk}" style="font-size:10px">{badge}</span><small>{state}</small></div></div><div class="qbody">{body}</div></div>')
cards=[
 card("anthropic","personal","polled 13:58","active","ok",[row("Session (5h)",620,1000,"in 3h 12m"),row("Weekly",1840,5000,"in 2d 4h")],prio=1),
 card("anthropic","work","last poll 3 days ago","needs sign-in","err",[],msg="Sign in again to read quota. In the CLI: nullrouter accounts login anthropic work",prio=2),
 card("xai","team","polled 14:01","active","ok",[row("Requests",4550,5000,"in 41m"),row("Tokens / day",1200000,10000000,"in 9h 40m")],prio=1),
 card("openrouter","main","estimated from usage","estimated","warn",[row("Daily budget",180,1000,"in 2d 0h")],prio=1),
 card("opencode-zen","free","polled 13:50","cooling","neu",[row("Free requests",200,200,"in 6h 5m")],prio=1),
 card("grok-cli","local","polled 14:00","active","ok",[row("Requests",312,0,"",unl=True)],prio=1),
]
print('<div class="qbar"><div class="qf"><span class="i">apps</span><span class="lbl">All providers</span><span class="i m">expand_more</span></div>'
      '<div class="qf sel">All accounts<span class="i m">expand_more</span></div>'
      '<div class="qf"><span class="i">hourglass_top</span><span class="lbl">Expiring first</span></div></div>')
print('<div class="qg">'+"".join(cards)+'</div>')
print('<div class="qpg"><span>Showing 1-6 of 6 accounts · polled quota is read from the provider, estimated quota is counted from your requests</span><span>Page 1 / 1</span></div>')
