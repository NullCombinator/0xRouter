#!/usr/bin/env python3
"""Traffic graph mockups in 9router's ProviderTopology style (sample traffic, real plugin names and colors).
usage   : circular layout, router in the middle, providers around it, no agent nodes.
gauges  : same node/edge styles, agents on the left, providers on the right, latency dials on the hops."""
import math, sys, hashlib
from plugdata import color, icon
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
    return (f'<div class="tn" {pos(x,y,st)}><span class="ic" style="background:{c}26;color:{c}">{icon(name)}</span>'
            f'<span class="nm" style="{"color:"+c if on else ""}">{name}</span>{ping}{tip}</div>')
def anode(name,col,x,y):
    h=hue(name)
    av=f'background:radial-gradient(circle at 20% 25%,hsl({h} 85% 70%),transparent 60%),radial-gradient(circle at 80% 85%,hsl({(h*7)%360} 80% 62%),transparent 55%),hsl({h} 60% 90%)'
    return (f'<div class="tn" {pos(x,y)}><span class="ic" style="border-radius:50%;{av};box-shadow:0 0 0 2px {col}"></span><span class="nm">{name}</span></div>')
def router(x,y,tip="",active=0):
    on=" on" if active else ""; bd=f'<span class="bdg">{active}</span>' if active else ""
    return f'<div class="rn{on}" {pos(x,y)}><span class="i" style="font-size:22px;margin-right:8px">hub</span><span class="lbl">0Router</span>{bd}{tip}</div>'
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
    return wrap(svg,nodes+router(0,0,rtip,na),480,.8)
def landscape():
    ax,px=-400,400; n=len(AG); svg=""; nodes=""; dials=""
    ay={a:-165+i*110 for i,(a,_) in enumerate(AG)}; col=dict(AG)
    py={p:-200+i*80 for i,p in enumerate(PV)}
    for a,c in AG:
        y0=ay[a]; x0=ax+NW/2; x1=-RW/2; o=(x1-x0)*.45
        svg+=edge(f"M{x0},{y0} C{x0+o},{y0} {x1-o},0 {x1},0",c,2.5,.9,"7 6")
        nodes+=anode(a,c,ax,y0)
        v=max(f[2] for f in FL if f[0]==a); gx,gy=bez((x0,y0),(x0+o,y0),(x1-o,0),(x1,0),.38)
        dials+=dial(gx,gy,v/40,fmt(v),[f"{a} → router",f"router overhead {fmt(v)}","time before the first upstream attempt","includes any middleware","as of 14:02:11"])
    seen={}
    for a,p,ov,tt,p95,nr,um,ok in FL:
        k=seen.get(p,0); seen[p]=k+1; y1=py[p]+k*14-7; x0=RW/2; x1=px-NW/2; o=(x1-x0)*.45
        svg+=edge(f"M{x0},0 C{x0+o},0 {x1-o},{y1} {x1},{y1}",col[a],2.5 if ok else 2.5,.9)
        gx,gy=bez((x0,0),(x0+o,0),(x1-o,y1),(x1,y1),.62+.2*k)
        dials+=dial(gx,gy,tt/3000,fmt(tt),[f"router → {p} ({a})",f"TTFT p50 {fmt(tt)} · p95 {fmt(p95)}",f"{nr} requests · unified model {um}","last response "+("resolved" if ok else "failed")])
    for p in PV:
        st=STATE.get(p); nodes+=pnode(p,px,py[p],"error" if st=="error" else None)
    return wrap(svg,nodes+router(0,0)+dials,480)
if __name__=="__main__": print(usage() if sys.argv[1]=="usage" else landscape())
