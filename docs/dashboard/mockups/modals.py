#!/usr/bin/env python3
"""Detail modals for provider and agent cards (sample data)."""
import sys, hashlib, re
from topo import FL, AG
from plugdata import models as pmodels, color as pcolor, icon as picon, logo_html
def hue(n): return int(hashlib.md5(n.encode()).hexdigest(),16)%360
def shell(mid,title,sub,logo,body,foot=""):
    if not logo: return (f'<input type="checkbox" id="{mid}" class="mt"><div class="modal"><label for="{mid}" class="mo"></label><div class="mb wide"><div class="mh slim"><span class="m" style="font-size:12px">{sub}</span><label for="{mid}" class="mx"><span class="i">close</span></label></div><div class="msc">{body}</div></div></div>')
    return (f'<input type="checkbox" id="{mid}" class="mt"><div class="modal"><label for="{mid}" class="mo"></label><div class="mb">'
            f'<div class="mh"><div class="row">{logo}<div><b>{title}</b><div class="m" style="font-size:12px;font-weight:400">{sub}</div></div></div><label for="{mid}" class="mx"><span class="i">close</span></label></div>'
            f'<div class="msc">{body}</div><div class="mf"><label for="{mid}" class="btn2">Close</label></div></div></div>')
def tiles(items): return '<div class="tiles" style="grid-template-columns:repeat(%d,1fr);margin-bottom:16px">%s</div>'%(len(items),''.join(f'<div class="tl"><small>{a}</small><b style="font-size:18px">{b}</b></div>' for a,b in items))
KINDS=[("chat","Text","smart_toy"),("embedding","Embedding","data_array"),("image","Text to Image","brush"),("tts","Text to Speech","record_voice_over"),("stt","Speech to Text","mic"),("video","Video","movie"),("decision","Decisions","psychology")]
KI={k:(l,i) for k,l,i in KINDS}
def prov(name,conns,mid):
    """Provider detail in 9router's provider-page layout (header, Connections card, Available Models card).
    Models of every kind (text, embedding, image, speech, video, decisions) sit in one list with a kind filter."""
    n=int(re.match(r'(\d+)',conns).group(1)) if re.match(r'\d',conns) else 0
    c=pcolor(name); ms=pmodels(name)
    cr=[("personal","OAuth","lock",'<span class="badge ok dot">active</span>',"warm cache, 612 requests today"),
        ("work","API key …a91f","key",'<span class="badge err dot">needs sign-in</span>',"401 from provider at 13:42")][:n]
    if cr:
        conn=''.join(f'<div class="cr"><span class="i m">{ic}</span><div style="flex:1;min-width:0"><div class="cn">{nm}</div><div class="cb2">{st}<span class="badge neu" style="font-size:10px">{au}</span><span class="m" style="font-size:12px">#{i}</span><span class="m" style="font-size:12px;color:var(--muted)">{note}</span></div></div></div>' for i,(nm,au,ic,st,note) in enumerate(cr,1))
    else:
        conn=f'<div class="row" style="gap:12px"><div class="tile" style="border-radius:50%;background:rgba(229,106,74,.1);color:var(--brand-500)"><span class="i" style="font-size:18px">key</span></div><div><div class="m" style="font-size:14px">No connections yet</div><div class="m" style="font-size:12px">Add one in the CLI: <code>nullrouter accounts add {name} &lt;name&gt;</code></div></div></div>'
    cnt={k:sum(1 for x in ms if x.get("kind","chat")==k) for k,_,_ in KINDS}
    chips='<label class="kc"><input type="radio" name="k'+mid+'" value="all" checked><span>All <i>'+str(len(ms))+'</i></span></label>'
    for k,l,ic in KINDS:
        chips+=f'<label class="kc{" z" if not cnt[k] else ""}"><input type="radio" name="k{mid}" value="{k}"><span><span class="i">{ic}</span>{l} <i>{cnt[k]}</i></span></label>'
    def mrow(x):
        k=x.get("kind","chat"); l,ic=KI.get(k,(k,"smart_toy")); cl=x.get("context_length")
        extra=f'<span class="badge neu" style="font-size:10px">{cl:,} ctx</span>' if cl else ''
        return (f'<div class="mrow k-{k}"><span class="i m" style="font-size:16px">{ic}</span><div style="min-width:0;flex:1"><code>{x["id"]}</code>'
                f'<div class="mn"><i>{x.get("name",x["id"])}</i><span class="badge neu" style="font-size:9px">{l}</span>{extra}</div></div><span class="i cp">content_copy</span></div>')
    grid=''.join(mrow(x) for x in ms) or '<div class="m" style="font-size:13px;padding:8px 0">Install this plugin to see its models</div>'
    body=''
    body+=f'<div class="ph"><div class="phl">{logo_html(name,48,8)}</div><div><div class="phn">{name}<a class="getkey"><span class="i" style="font-size:14px">open_in_new</span>Get API Key</a></div><div class="m">{n} connection{"" if n==1 else "s"} · {len(ms)} model{"" if len(ms)==1 else "s"}</div></div></div>'
    body+=f'<div class="card pc2"><div class="pch"><h2>Connections</h2><span class="badge {"err" if name=="elevenlabs" else "ok"} dot">last response {"failed" if name=="elevenlabs" else "resolved"}</span></div>{conn}</div>'
    body+=f'<div class="card pc2"><div class="pch"><h2>Available Models</h2></div><div class="kf">{chips}</div><div class="mgrid">{grid}</div></div>'
    body+='<div class="card pc2"><div class="pch"><h2>Limits notes</h2><span class="m" style="font-size:12px">same as <code>nullrouter providers</code></span></div><div class="m" style="font-size:13px">Context length differs between two models on this provider; the smaller one is used for unified model sonnet.</div></div>'
    return shell(mid,name,"provider plugin · schema 2","",body)
def agent(name,harness,mid):
    h=hue(name); fl=[f for f in FL if f[0]==name]; col=dict((a,c) for a,c in AG).get(name,"#9CA3AF")
    av=f'<div class="av" style="background:radial-gradient(circle at 20% 25%,hsl({h} 85% 70%),transparent 60%),radial-gradient(circle at 80% 85%,hsl({(h*7)%360} 80% 62%),transparent 55%),hsl({h} 60% 90%)"></div>'
    body=tiles([("Requests today",str(sum(f[5] for f in fl))),("Last used","14:01"),("Harness",harness)])
    body+='<div class="ch"><div class="tile"><span class="i">alt_route</span></div><div><h3>Where its requests went</h3><p>Same as <code>nullrouter records list</code>, grouped by provider</p></div></div><table><tr><th>Provider</th><th>Unified model</th><th class="n">TTFT p50</th><th class="n">Requests</th><th>Last</th></tr>'
    body+=''.join(f'<tr><td><i style="display:inline-block;width:10px;height:10px;border-radius:50%;background:{col};margin-right:8px"></i><b>{f[1]}</b></td><td class="m">{f[6]}</td><td class="n">{f[3]} ms</td><td class="n m">{f[5]}</td><td><span class="badge {"ok" if f[7] else "err"}">{"served" if f[7] else "failed"}</span></td></tr>' for f in fl) or '<tr><td colspan="5" class="m" style="text-align:center">No requests yet</td></tr>'
    body+='</table><div class="ch" style="margin-top:20px"><div class="tile"><span class="i">link</span></div><div><h3>Connect this agent</h3><p>Preview of the settings to give it. The key itself is shown once, when issued.</p></div></div><div class="inset mono" style="font-size:12px;line-height:1.7">BASE_URL=http://127.0.0.1:20129<br>API_KEY=&lt;the key&gt;</div>'
    return shell(mid,name,f"agent · harness {harness}",av,body)
if __name__=="__main__":
    k=sys.argv[1]
    print(prov(*sys.argv[2:5]) if k=="prov" else agent(*sys.argv[2:5]))
