#!/usr/bin/env python3
"""Topology landscape mockup (sample data). mode: gauges (Endpoint & Key) or usage (hover boxes)."""
import math, sys, hashlib
AG=[("claude-code","#E56A4A",70),("codex","#3b82f6",150),("hermes-research","#10b981",230),("ci-bot","#a855f7",310)]
PV=[("anthropic",45,"ok","14:01:58"),("xai",115,"ok","14:01:40"),("openrouter",185,"ok","14:01:12"),("opencode-zen",255,"ok","13:47:03"),("glm",325,"err","14:00:31"),("mistral",395,"ok","yesterday")]
# flows: agent, provider, router overhead ms, ttft ms, p95 ttft, requests, unified model, last outcome
FL=[("claude-code","anthropic",4,412,1900,512,"sonnet","ok"),("claude-code","openrouter",5,1900,3100,100,"sonnet","ok"),
    ("codex","xai",3,690,1500,70,"gpt-5","ok"),("codex","openrouter",4,820,2200,18,"sonnet","ok"),
    ("hermes-research","anthropic",6,380,1300,30,"sonnet","ok"),("hermes-research","opencode-zen",5,1250,2800,11,"glm-4.6","ok"),
    ("ci-bot","glm",4,2400,5200,6,"glm-4.6","err")]
W,H=1000,440; RX,RY=500,205
def hue(n): return int(hashlib.md5(n.encode()).hexdigest(),16)%360
def bez(p0,p1,p2,p3,t):
    return tuple((1-t)**3*a+3*(1-t)**2*t*b+3*(1-t)*t*t*c+t**3*d for a,b,c,d in zip(p0,p1,p2,p3))
def gauge(cx,cy,frac,label,lines,color="#0a0a0a"):
    r=17
    def pt(a): return (cx+r*math.cos(math.radians(a)), cy-r*math.sin(math.radians(a)))
    def arc(a0,a1,col):
        x0,y0=pt(a0); x1,y1=pt(a1)
        return f'<path d="M{x0:.1f},{y0:.1f} A{r},{r} 0 0 1 {x1:.1f},{y1:.1f}" fill="none" stroke="{col}" stroke-width="5"/>'
    z=arc(180,90,"#22c55e")+arc(90,36,"#eab308")+arc(36,0,"#ef4444")
    a=180-180*min(max(frac,0),1); nx,ny=pt(a); nx=cx+(nx-cx)*.85; ny=cy+(ny-cy)*.85
    w=max(len(l) for l in lines)*6.1+20; h=len(lines)*16+14
    tx=cx-w/2; ty=cy-r-h-14
    tip=f'<g class="tipg"><rect x="{tx:.0f}" y="{ty:.0f}" width="{w:.0f}" height="{h}" rx="8" fill="#fff" stroke="#e5e7eb"/>'+''.join(f'<text x="{tx+10:.0f}" y="{ty+19+i*16:.0f}" font-size="11" fill="{"#0a0a0a" if i==0 else "#6B7280"}" font-weight="{600 if i==0 else 400}">{l}</text>' for i,l in enumerate(lines))+'</g>'
    return (f'<g class="gw"><circle cx="{cx:.1f}" cy="{cy:.1f}" r="24" fill="#fff" fill-opacity=".92" stroke="#e5e7eb"/>{z}'
            f'<line x1="{cx:.1f}" y1="{cy:.1f}" x2="{nx:.1f}" y2="{ny:.1f}" stroke="#0a0a0a" stroke-width="1.6" stroke-linecap="round"/><circle cx="{cx:.1f}" cy="{cy:.1f}" r="2.5" fill="#0a0a0a"/>'
            f'<text x="{cx:.1f}" y="{cy+17:.1f}" text-anchor="middle" font-size="9" fill="#6B7280">{label}</text>{tip}</g>')
def fmt(ms): return f"{ms} ms" if ms<1000 else f"{ms/1000:.1f} s"
def build(mode):
    out=[f'<svg class="topo" viewBox="0 0 {W} {H}" width="100%" xmlns="http://www.w3.org/2000/svg" font-family="Inter,system-ui,sans-serif">']
    col={a:c for a,c,_ in AG}; ay={a:y for a,_,y in AG}; py={p:y for p,y,_,_ in PV}
    used_p={}
    # links
    for a,p,ov,tt,p95,n,um,last in FL:
        c=col[a]; y0=ay[a]
        d=f'M200,{y0} C330,{y0} 370,{RY} {RX-48},{RY}'
        out.append(f'<path d="{d}" fill="none" stroke="{c}" stroke-width="2" stroke-dasharray="6 5" opacity=".85"/>')
    seen={}
    for a,p,ov,tt,p95,n,um,last in FL:
        k=seen.get(p,0); seen[p]=k+1; off=(k*7)-3
        y1=py[p]+off
        out.append(f'<path d="M{RX+48},{RY} C{RX+170},{RY} {RX+210},{y1} 800,{y1}" fill="none" stroke="{col[a]}" stroke-width="2.4" opacity=".9"/>')
    # agent nodes
    for a,c,y in AG:
        h=hue(a)
        out.append(f'<g><rect x="30" y="{y-22}" width="170" height="44" rx="10" fill="#fff" stroke="#e5e7eb"/><circle cx="54" cy="{y}" r="13" fill="hsl({h} 70% 75%)"/><circle cx="54" cy="{y}" r="13" fill="none" stroke="{c}" stroke-width="2"/><text x="76" y="{y+4}" font-size="13" font-weight="600" fill="#0a0a0a">{a}</text></g>')
    # router
    rtip=''
    if mode=='usage':
        rows=[f'{a}  →  {um} (unified)  →  {p}' for a,p,ov,tt,p95,n,um,last in FL]
        extra=[ "combo: none (combos are not built yet)"]
        w=max(len(r) for r in rows+extra)*6.2+24; h=(len(rows)+3)*17+16
        bx=RX-w/2; by=RY-48-h-10
        body=f'<rect x="{bx:.0f}" y="{by:.0f}" width="{w:.0f}" height="{h}" rx="10" fill="#fff" stroke="#e5e7eb"/><text x="{bx+12:.0f}" y="{by+22:.0f}" font-size="12" font-weight="600">Connections now routed</text>'
        for i,(a,p,ov,tt,p95,n,um,last) in enumerate(FL):
            yy=by+42+i*17
            body+=f'<circle cx="{bx+16:.0f}" cy="{yy-4}" r="4" fill="{col[a]}"/><text x="{bx+28:.0f}" y="{yy}" font-size="11" fill="#0a0a0a">{a}</text><text x="{bx+28+len(a)*6.6+8:.0f}" y="{yy}" font-size="11" fill="#6B7280">→ {um} → {p}</text>'
        body+=f'<text x="{bx+12:.0f}" y="{by+h-12:.0f}" font-size="11" fill="#a16207">{extra[0]}</text>'
        rtip=f'<g class="tipg">{body}</g>'
    out.append(f'<g class="gw"><rect x="{RX-48}" y="{RY-48}" width="96" height="96" rx="20" fill="url(#rg)" stroke="#a64027"/><text x="{RX}" y="{RY+2}" text-anchor="middle" font-size="22" font-weight="700" fill="#fff">0R</text><text x="{RX}" y="{RY+22}" text-anchor="middle" font-size="10" fill="#fff" opacity=".85">router</text>{rtip}</g>')
    # providers
    for p,y,st,lastt in PV:
        h=hue(p); dot="#22c55e" if st=="ok" else "#ef4444"
        tip=''
        if mode=='usage':
            msg="Last response resolved" if st=="ok" else "Last response failed"
            sub=f"{lastt} · "+("served" if st=="ok" else "upstream error 529, fell back")
            w=max(len(msg),len(sub))*6.3+24
            tip=f'<g class="tipg"><rect x="{800+80-w/2:.0f}" y="{y-22-56}" width="{w:.0f}" height="46" rx="10" fill="#fff" stroke="{dot}"/><circle cx="{800+80-w/2+14:.0f}" cy="{y-22-56+18}" r="5" fill="{dot}"/><text x="{800+80-w/2+26:.0f}" y="{y-22-56+22}" font-size="12" font-weight="600">{msg}</text><text x="{800+80-w/2+12:.0f}" y="{y-22-56+38}" font-size="11" fill="#6B7280">{sub}</text></g>'
        out.append(f'<g class="gw"><rect x="800" y="{y-22}" width="160" height="44" rx="10" fill="#fff" stroke="#e5e7eb"/><rect x="810" y="{y-12}" width="24" height="24" rx="6" fill="hsl({h} 80% 94%)"/><text x="822" y="{y+5}" text-anchor="middle" font-size="12" font-weight="600" fill="hsl({h} 55% 42%)">{p[0]}</text><text x="842" y="{y+4}" font-size="13" font-weight="600" fill="#0a0a0a">{p}</text><circle cx="946" cy="{y}" r="4.5" fill="{dot}"/>{tip}</g>')
    # gauges
    if mode=='gauges':
        for a,p,ov,tt,p95,n,um,last in FL:
            # left hop gauge, once per agent
            pass
        done=set()
        for a,p,ov,tt,p95,n,um,last in FL:
            if a not in done:
                done.add(a); y0=ay[a]
                gx,gy=bez((200,y0),(330,y0),(370,RY),(RX-48,RY),.42)
                ovs=[f[2] for f in FL if f[0]==a]; v=max(ovs)
                out.append(gauge(gx,gy-30,v/40,fmt(v),[f"{a} → router","router overhead p50 "+fmt(v),"time before the first upstream attempt","includes any middleware","last 50 requests · as of 14:02:11"]))
        seen={}
        for a,p,ov,tt,p95,n,um,last in FL:
            k=seen.get(p,0); seen[p]=k+1; y1=py[p]+(k*7)-3
            gx,gy=bez((RX+48,RY),(RX+170,RY),(RX+210,y1),(800,y1),.58+.17*k)
            out.append(gauge(gx,gy-30,tt/3000,fmt(tt),[f"router → {p}  ({a})",f"TTFT p50 {fmt(tt)} · p95 {fmt(p95)}",f"{n} requests · unified model {um}","last response "+("resolved" if last=="ok" else "failed")]))
    out.insert(1,'<defs><linearGradient id="rg" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#E56A4A"/><stop offset="1" stop-color="#a64027"/></linearGradient></defs>')
    out.append('</svg>')
    return ''.join(out)
if __name__=='__main__': print(build(sys.argv[1]))
