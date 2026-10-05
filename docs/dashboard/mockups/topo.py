#!/usr/bin/env python3
"""Traffic graph mockups in 9router's ProviderTopology style (sample traffic, real plugin names and colors).
usage   : circular layout, router in the middle, providers around it, no agent nodes.
gauges  : same node/edge styles, agents on the left, providers on the right, latency dials on the hops."""
import math, sys, hashlib
from plugdata import color, icon, logo_html
PV=["anthropic","xai","grok-cli","openrouter","opencode-zen","elevenlabs"]
STATE={"anthropic":"active","openrouter":"last","elevenlabs":"error"}   # in flight / last response / failed
AG=[("claude-code","#E56A4A"),("codex","#3b82f6"),("hermes-research","#10b981"),("ci-bot","#a855f7")]
# agent, provider, router overhead ms, ttft ms, p95 ttft, requests, unified model, last ok
FL=[("claude-code","anthropic",4,412,1900,512,"sonnet",True),("claude-code","openrouter",5,1900,3100,100,"sonnet",True),
    ("codex","xai",3,690,1500,70,"grok",True),("codex","openrouter",4,820,2200,18,"sonnet",True),
    ("hermes-research","anthropic",6,380,1300,30,"sonnet",True),("hermes-research","opencode-zen",5,1250,2800,11,"glm",True),
    ("ci-bot","elevenlabs",4,2400,5200,6,"tts-flash",False)]
CY,AMB,RED="#22d3ee","#f59e0b","#ef4444"
def hue(n): return int(hashlib.md5(n.encode()).hexdigest(),16)%360
def fmt(ms): return f"{ms} ms" if ms<1000 else f"{ms/1000:.1f} s"
def bez(p0,c1,c2,p1,t):
    return tuple((1-t)**3*a+3*(1-t)**2*t*b+3*(1-t)*t*t*c+t**3*d for a,b,c,d in zip(p0,c1,c2,p1))
def pos(x,y,extra=""): return f'style="left:calc(50% + {x:.0f}px);top:calc(50% + {y:.0f}px);{extra}"'
def pnode(name,x,y,state=None,tip=""):
    c=color(name); on=state=="active"
    st=f"border-color:{c};box-shadow:0 0 16px {c}40;" if on else ""
    ping=f'<span class="ping"><i style="background:{c}"></i><b style="background:{c}"></b></span>' if on else ""
    return (f'<div class="tn" {pos(x,y,st)}>{logo_html(name,32,6,"ic")}'
            f'<span class="nm" style="{"color:"+c if on else ""}">{name}</span>{ping}{tip}</div>')
def anode(name,col,x,y):
    h=hue(name)
    av=f'background:radial-gradient(circle at 20% 25%,hsl({h} 85% 70%),transparent 60%),radial-gradient(circle at 80% 85%,hsl({(h*7)%360} 80% 62%),transparent 55%),hsl({h} 60% 90%)'
    return (f'<div class="tn" {pos(x,y)}><span class="ic" style="border-radius:50%;{av};box-shadow:0 0 0 2px {col}"></span><span class="nm">{name}</span></div>')
def router(x,y,tip="",active=0):
    on=" on" if active else ""; bd=f'<span class="bdg">{active}</span>' if active else ""
    return f'<div class="rn{on}" {pos(x,y)}><img class="rlg" src="logo-mark.png" alt=""><span class="lbl">0Router</span>{bd}{tip}</div>'
def tipbox(inner,cls="tt",extra=""): return f'<div class="{cls}" style="{extra}">{inner}</div>'
def dial(x,y,frac,label,lines):
    r=17; cx=cy=28
    def pt(a,rr=r): return (cx+rr*math.cos(math.radians(a)), cy-rr*math.sin(math.radians(a)))
    def arc(a0,a1,c):
        x0,y0=pt(a0); x1,y1=pt(a1); return f'<path d="M{x0:.1f},{y0:.1f} A{r},{r} 0 0 1 {x1:.1f},{y1:.1f}" fill="none" stroke="{c}" stroke-width="5"/>'
    z=arc(180,90,"#22c55e")+arc(90,36,"#eab308")+arc(36,0,"#ef4444")
    nx,ny=pt(180-180*min(max(frac,0),1),r*.85)
    svg=(f'<svg width="56" height="56" viewBox="0 0 56 56"><circle cx="28" cy="28" r="25" fill="#fff" fill-opacity=".95" stroke="#e5e7eb"/>{z}'
         f'<line x1="28" y1="28" x2="{nx:.1f}" y2="{ny:.1f}" stroke="#0a0a0a" stroke-width="1.6" stroke-linecap="round"/><circle cx="28" cy="28" r="2.5" fill="#0a0a0a"/>'
         f'<text x="28" y="48" text-anchor="middle" font-size="9" fill="#6B7280">{label}</text></svg>')
    tip=tipbox(f'<b>{lines[0]}</b>'+''.join(f'<div>{l}</div>' for l in lines[1:]))
    return f'<div class="gd" {pos(x,y)}>{svg}{tip}</div>'
def edge(d,stroke,w,op,dash=""):
    return f'<path d="{d}" fill="none" stroke="{stroke}" stroke-width="{w}" opacity="{op}" stroke-linecap="round"{(" stroke-dasharray=%r"%dash).replace(chr(39),chr(34)) if dash else ""}/>'
def electric(d,i):
    f=f'ef{i}'
    s=(f'<defs><filter id="{f}" filterUnits="userSpaceOnUse" x="-700" y="-450" width="1400" height="900"><feTurbulence type="fractalNoise" baseFrequency="0.9" numOctaves="2" seed="2" result="n"><animate attributeName="baseFrequency" values="0.8;1.4;0.8" dur="0.25s" repeatCount="indefinite"/></feTurbulence><feDisplacementMap in="SourceGraphic" in2="n" scale="3.5" xChannelSelector="R" yChannelSelector="G"/></filter></defs>'
       f'<path class="ek-halo" d="{d}" fill="none" stroke="#22d3ee" stroke-width="10" stroke-opacity=".35" stroke-linecap="round" filter="url(#{f})"/>'
       f'<path class="ek-plasma" d="{d}" fill="none" stroke="#4ade80" stroke-width="5" stroke-opacity=".85" stroke-linecap="round" filter="url(#{f})"/>'
       f'<path class="ek-core" d="{d}" fill="none" stroke="#f8fafc" stroke-width="2.2"/>')
    for k in range(6):
        col=["#fde047","#67e8f9","#fff"][k%3]
        s+=f'<circle r="{4 if k%2==0 else 2.5}" fill="{col}" opacity=".95" style="filter:drop-shadow(0 0 4px #22d3ee)"><animateMotion dur="{0.4+k*0.08:.2f}s" repeatCount="indefinite" path="{d}" begin="{k*0.09:.2f}s"/></circle>'
    return f'<g>{s}</g>'
CTRL='<div class="rfc"><span class="i">add</span><span class="i">remove</span><span class="i">fit_screen</span></div>'
def wrap(svg,nodes,h,z=1): return f'<div class="tf" style="height:{h}px"><div class="tfi" style="transform:scale({z})"><svg class="te" width="1" height="1" style="overflow:visible;position:absolute;left:50%;top:50%">{svg}</svg>{nodes}</div>{CTRL}</div>'
NW,NH,RW,RH=190,56,130,48
def usage():
    n=len(PV); rx,ry=320,200; svg=""; nodes=""; na=0
    for i,p in enumerate(PV):
        a=-math.pi/2+2*math.pi*i/n; cx,cy=rx*math.cos(a),ry*math.sin(a)
        if abs(a+math.pi/2)<math.pi/4: s=(0,-RH/2); t=(0,NH/2); dr=(0,-1)
        elif abs(a-math.pi/2)<math.pi/4: s=(0,RH/2); t=(0,-NH/2); dr=(0,1)
        elif cx>0: s=(RW/2,0); t=(-NW/2,0); dr=(1,0)
        else: s=(-RW/2,0); t=(NW/2,0); dr=(-1,0)
        x0,y0=s; x1,y1=cx+t[0],cy+t[1]; o=0.45*(abs(x1-x0) if dr[0] else abs(y1-y0))
        d=f"M{x0},{y0} C{x0+dr[0]*o},{y0+dr[1]*o} {x1-dr[0]*o},{y1-dr[1]*o} {x1},{y1}"
        st=STATE.get(p)
        if st=="active": na+=1; svg+=electric(d,i)
        else: svg+=(edge(d,RED,2.5,.9) if st=="error" else edge(d,AMB,2,.7) if st=="last" else edge(d,"#e5e7eb",1,.3))
        ok=st!="error"; c="#22c55e" if ok else RED
        tip=tipbox(f'<span class="dot" style="background:{c}"></span><b>{"Last response resolved" if ok else "Last response failed"}</b><div>{p} · '+("14:01:58 · served" if ok else "14:00:31 · upstream 529, fell back")+'</div>',"tt tp",f"border-color:{c}")
        nodes+=pnode(p,cx,cy,st,tip)
    boxes=''
    for a,p,ov,tt,p95,nr,um,ok in FL[:5]:
        boxes+=f'<div class="cx"><b>{p}</b><div>unified model <b>{um}</b></div><div>combo <b>none</b> <span class="m">(not built yet)</span></div></div>'
    rtip=tipbox(f'<div class="cbh">Connections routed now</div>{boxes}',"tt tr")
    return wrap(svg,nodes+router(0,0,rtip,na),480,.72)
PW=24   # pipe bore in px
def lerp(c0,c1,u): return tuple(round(a+(b-a)*u) for a,b in zip(c0,c1))
def gauge(x,y,frac,label,lines):
    """Pressure gauge screwed into a pipe: dial on a short neck with a hex nut and a threaded stub; (x,y) is the pipe's centre line.
    Scale runs white to red only (low latency white, high red)."""
    cx,cy,r=30,30,19; a0,a1=210,-30; n=26
    def pt(a,rr): return (cx+rr*math.cos(math.radians(a)), cy-rr*math.sin(math.radians(a)))
    segs=""
    for i in range(n):
        u0,u1=i/n,(i+1)/n; p0=pt(a0+(a1-a0)*u0,r); p1=pt(a0+(a1-a0)*u1,r)
        c=lerp((255,255,255),(239,68,68),min(1,(u0*1.15)**1.6))
        segs+=f'<path d="M{p0[0]:.1f},{p0[1]:.1f} A{r},{r} 0 0 1 {p1[0]:.1f},{p1[1]:.1f}" fill="none" stroke="rgb{c}" stroke-width="5"/>'
    p0=pt(a0,r); p1=pt(a1,r)
    under=f'<path d="M{p0[0]:.1f},{p0[1]:.1f} A{r},{r} 0 1 1 {p1[0]:.1f},{p1[1]:.1f}" fill="none" stroke="#cbd5e1" stroke-width="8" stroke-linecap="round"/>'
    ticks="".join(f'<line x1="{pt(a0+(a1-a0)*k/8,r-5)[0]:.1f}" y1="{pt(a0+(a1-a0)*k/8,r-5)[1]:.1f}" x2="{pt(a0+(a1-a0)*k/8,r-8)[0]:.1f}" y2="{pt(a0+(a1-a0)*k/8,r-8)[1]:.1f}" stroke="#94a3b8" stroke-width="1"/>' for k in range(9))
    nx,ny=pt(a0+(a1-a0)*min(max(frac,0),1),r-3)
    svg=(f'<svg width="54" height="77" viewBox="0 0 60 86">'
         f'<rect x="26" y="56" width="8" height="12" fill="#94a3b8"/>'                                   # neck
         f'<rect x="21" y="66" width="18" height="8" rx="1.5" fill="#cbd5e1" stroke="#64748b"/>'         # hex nut
         f'<rect x="25" y="74" width="10" height="12" fill="#94a3b8"/>'
         +"".join(f'<line x1="25" y1="{78+k*3}" x2="35" y2="{78+k*3}" stroke="#64748b" stroke-width="1"/>' for k in range(3))+   # thread
         f'<circle cx="30" cy="30" r="28" fill="#e2e8f0" stroke="#64748b" stroke-width="2"/><circle cx="30" cy="30" r="25" fill="#f8fafc" stroke="#cbd5e1"/>'
         f'{under}{segs}{ticks}<line x1="30" y1="30" x2="{nx:.1f}" y2="{ny:.1f}" stroke="#0a0a0a" stroke-width="1.8" stroke-linecap="round"/><circle cx="30" cy="30" r="3" fill="#0a0a0a"/>'
         f'<text x="30" y="46" text-anchor="middle" font-size="8" font-weight="600" fill="#475569">{label}</text></svg>')
    tip=tipbox(f'<b>{lines[0]}</b>'+''.join(f'<div>{l}</div>' for l in lines[1:]))
    return f'<div class="gp" style="left:calc(50% + {x:.0f}px);top:calc(50% + {y-PW/2+2:.0f}px)">{svg}{tip}</div>'
def collar(x,y,vertical=False):
    w,h=9,PW+10
    if vertical: w,h=h,w
    bolts="".join(f'<circle cx="{x+dx:.1f}" cy="{y+dy:.1f}" r="1.5" fill="#475569"/>' for dx,dy in ([(-(PW/2+1),0),((PW/2+1),0)] if vertical else [(0,-(PW/2+1)),(0,PW/2+1)]))
    return f'<rect x="{x-w/2:.1f}" y="{y-h/2:.1f}" width="{w}" height="{h}" rx="2" fill="#cbd5e1" fill-opacity=".92" stroke="#64748b"/>{bolts}'
def landscape():
    ax,px=-400,400; NWL=190; svg=""; nodes=""; gauges=""
    ay={a:-165+i*110 for i,(a,_) in enumerate(AG)}; col=dict(AG)
    py={p:-260+i*104 for i,p in enumerate(PV)}
    LM,RM=-110,110                     # manifold x on each side of the router
    body=[]; liquid=""; fit=""
    # agent pipes -> left manifold -> router
    for a,c in AG:
        y0=ay[a]; body.append((ax,y0,LM,y0,False))
        liquid+=f'<line class="fl" x1="{ax+NWL/2}" y1="{y0}" x2="{LM}" y2="{y0}" stroke="{c}" stroke-width="7" stroke-opacity=".85"/>'
        fit+=collar(ax+NWL/2+5,y0)+collar(LM-PW/2-10,y0)
        v=max(f[2] for f in FL if f[0]==a); gx=(ax+NWL/2+LM)/2
        gauges+=gauge(gx,y0,v/40,fmt(v),[f"{a} → router","router overhead "+fmt(v),"time before the first upstream attempt","includes any middleware","as of 14:02:11"])
        nodes+=anode(a,c,ax,y0).replace('class="tn"','class="tn" ',1).replace('style="','style="width:190px;',1)
    body.append((LM,min(ay.values()),LM,max(ay.values()),True)); body.append((LM,0,-RW/2,0,False))
    fit+=collar(LM+PW/2+10,0)+collar(-RW/2-8,0)
    # router -> right manifold -> one pipe per provider
    body.append((RW/2,0,RM,0,False)); body.append((RM,min(py.values()),RM,max(py.values()),True))
    fit+=collar(RW/2+8,0)+collar(RM-PW/2-10,0)
    for p in PV:
        y1=py[p]; body.append((RM,y1,px,y1,False))
        flows=[f for f in FL if f[1]==p]; k=len(flows)
        for i,f in enumerate(flows):
            oy=y1+(i-(k-1)/2)*6
            liquid+=f'<line class="fl" x1="{RM}" y1="{oy}" x2="{px-NWL/2}" y2="{oy}" stroke="{col[f[0]]}" stroke-width="4" stroke-opacity=".9"/>'
        fit+=collar(RM+PW/2+10,y1)+collar(px-NWL/2-5,y1)
        if flows:
            req=sum(f[5] for f in flows); tt=round(sum(f[3]*f[5] for f in flows)/req); p95=max(f[4] for f in flows); ok=all(f[7] for f in flows)
            rows=[f'<span class="dot" style="background:{col[f[0]]}"></span>{f[0]} · {f[5]} req · TTFT {fmt(f[3])}' for f in flows]
            gauges+=gauge((RM+px-NWL/2)/2,y1,tt/3000,fmt(tt),[f"router → {p}",f"TTFT p50 {fmt(tt)} · p95 {fmt(p95)} (all agents)",*rows,"last response "+("resolved" if ok else "failed")])
        st=STATE.get(p)
        nodes+=pnode(p,px,y1,"error" if st=="error" else None).replace('style="','style="width:190px;',1)
    walls=''.join(f'<line x1="{a}" y1="{b}" x2="{c}" y2="{d}" stroke="#64748b" stroke-opacity=".6" stroke-width="{PW+4}" stroke-linecap="{"round" if v else "butt"}"/>' for a,b,c,d,v in body)
    holes=''.join(f'<line x1="{a}" y1="{b}" x2="{c}" y2="{d}" stroke="#000" stroke-width="{PW}" stroke-linecap="{"round" if v else "butt"}"/>' for a,b,c,d,v in body)
    glass=''.join(f'<line x1="{a}" y1="{b}" x2="{c}" y2="{d}" stroke="#e2e8f0" stroke-opacity=".5" stroke-width="{PW}" stroke-linecap="{"round" if v else "butt"}"/>' for a,b,c,d,v in body)
    shine=''.join((f'<line x1="{a-7}" y1="{b}" x2="{c-7}" y2="{d}"' if v else f'<line x1="{a}" y1="{b-7}" x2="{c}" y2="{d-7}"')+' stroke="#fff" stroke-opacity=".75" stroke-width="2" stroke-linecap="round"/>' for a,b,c,d,v in body)
    defs='<defs><mask id="pm" maskUnits="userSpaceOnUse" x="-700" y="-420" width="1400" height="840"><rect x="-700" y="-420" width="1400" height="840" fill="#fff"/>'+holes+'</mask></defs>'
    svg=defs+f'<g mask="url(#pm)">{walls}</g>{glass}{liquid}{shine}{fit}'
    return wrap(svg,nodes+router(0,0)+gauges,700)
if __name__=="__main__": print(usage() if sys.argv[1]=="usage" else landscape())
