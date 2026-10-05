#!/usr/bin/env python3
"""Detail modals for provider and agent cards (sample data)."""
import sys, hashlib, re
from topo import FL, AG
def hue(n): return int(hashlib.md5(n.encode()).hexdigest(),16)%360
def shell(mid,title,sub,logo,body,foot=""):
    return (f'<input type="checkbox" id="{mid}" class="mt"><div class="modal"><label for="{mid}" class="mo"></label><div class="mb">'
            f'<div class="mh"><div class="row">{logo}<div><b>{title}</b><div class="m" style="font-size:12px;font-weight:400">{sub}</div></div></div><label for="{mid}" class="mx"><span class="i">close</span></label></div>'
            f'<div class="msc">{body}</div><div class="mf"><label for="{mid}" class="btn2">Close</label></div></div></div>')
def tiles(items): return '<div class="tiles" style="grid-template-columns:repeat(%d,1fr);margin-bottom:16px">%s</div>'%(len(items),''.join(f'<div class="tl"><small>{a}</small><b style="font-size:18px">{b}</b></div>' for a,b in items))
def prov(name,conns,mid):
    h=hue(name); n=int(re.match(r'(\d+)',conns).group(1)) if re.match(r'\d',conns) else 0
    logo=f'<div class="lg" style="width:32px;height:32px;border-radius:8px;display:grid;place-items:center;font-weight:600;background:hsl({h} 80% 94%);color:hsl({h} 55% 42%)">{name[0]}</div>'
    rows=''.join(f'<tr><td><b>{name}</b> / {nm}</td><td class="m">{au}</td><td>{st}</td><td class="n m">{i}</td></tr>' for i,(nm,au,st) in enumerate([("personal","OAuth",'<span class="badge ok">active</span>'),("work","key …a91f",'<span class="badge err">needs sign-in</span>')][:n],1)) or '<tr><td colspan="4" class="m" style="text-align:center">No connections. Add one with <code>nullrouter accounts add '+name+' &lt;name&gt;</code></td></tr>'
    body=tiles([("Connections",str(n)),("Models","14"),("Last response","resolved" if name!="glm" else "failed")])
    body+=f'<div class="ch"><div class="tile"><span class="i">group</span></div><div><h3>Connections</h3><p>Same as <code>nullrouter accounts list --long</code></p></div></div><table><tr><th>Account</th><th>Auth</th><th>Status</th><th class="n">Priority</th></tr>{rows}</table>'
    body+='<div class="ch" style="margin-top:20px"><div class="tile"><span class="i">sell</span></div><div><h3>Limits notes</h3><p>Same as <code>nullrouter providers</code></p></div></div><div class="inset m" style="font-size:13px">Context length differs between two models on this provider; the smaller one is used for unified model sonnet.</div>'
    return shell(mid,name,"provider plugin · schema 2",logo,body)
def agent(name,harness,mid):
    h=hue(name); fl=[f for f in FL if f[0]==name]; col=dict((a,c) for a,c,_ in AG).get(name,"#9CA3AF")
    av=f'<div class="av" style="background:radial-gradient(circle at 20% 25%,hsl({h} 85% 70%),transparent 60%),radial-gradient(circle at 80% 85%,hsl({(h*7)%360} 80% 62%),transparent 55%),hsl({h} 60% 90%)"></div>'
    body=tiles([("Requests today",str(sum(f[5] for f in fl))),("Last used","14:01"),("Harness",harness)])
    body+='<div class="ch"><div class="tile"><span class="i">alt_route</span></div><div><h3>Where its requests went</h3><p>Same as <code>nullrouter records list</code>, grouped by provider</p></div></div><table><tr><th>Provider</th><th>Unified model</th><th class="n">TTFT p50</th><th class="n">Requests</th><th>Last</th></tr>'
    body+=''.join(f'<tr><td><i style="display:inline-block;width:10px;height:10px;border-radius:50%;background:{col};margin-right:8px"></i><b>{f[1]}</b></td><td class="m">{f[6]}</td><td class="n">{f[3]} ms</td><td class="n m">{f[5]}</td><td><span class="badge {"ok" if f[7]=="ok" else "err"}">{"served" if f[7]=="ok" else "failed"}</span></td></tr>' for f in fl) or '<tr><td colspan="5" class="m" style="text-align:center">No requests yet</td></tr>'
    body+='</table><div class="ch" style="margin-top:20px"><div class="tile"><span class="i">link</span></div><div><h3>Connect this agent</h3><p>Preview of the settings to give it. The key itself is shown once, when issued.</p></div></div><div class="inset mono" style="font-size:12px;line-height:1.7">BASE_URL=http://127.0.0.1:20129<br>API_KEY=&lt;the key&gt;</div>'
    return shell(mid,name,f"agent · harness {harness}",av,body)
if __name__=="__main__":
    k=sys.argv[1]
    print(prov(*sys.argv[2:5]) if k=="prov" else agent(*sys.argv[2:5]))
