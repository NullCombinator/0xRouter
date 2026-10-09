#!/usr/bin/env python3
"""Usage overview head in 9router's order: tabs + period filter, five stat cards, topology beside Recent Requests.
The period filter is radios plus :has() in mockup.css, so it needs no script (the real page would use ?period=)."""
import subprocess
PER=[("today","Today"),("24h","24h"),("7d","7D"),("30d","30D"),("60d","60D"),("all","All")]
#        requests  input   cached  output  cost
VAL={"today":("214","612k","410k","89k","$1.84"),"24h":("380","1.1M","760k","160k","$3.20"),
     "7d":("1,284","4.2M","2.9M","611k","$11.60"),"30d":("5,102","16.8M","11.4M","2.4M","$46.10"),
     "60d":("9,870","31.5M","21.9M","4.6M","$88.40"),"all":("14,300","46.0M","31.0M","6.7M","$128.90")}
REQ=[("sonnet",12400,880,"2s ago",1),("sonnet",9100,610,"31s ago",1),("grok",4300,1200,"1m ago",1),("tts-flash",120,0,"2m ago",0),
     ("sonnet",15800,940,"4m ago",1),("glm",2200,3100,"6m ago",1),("sonnet",8800,420,"9m ago",1),("grok",3900,1100,"12m ago",1),
     ("sonnet",14100,760,"15m ago",1),("sonnet",6200,510,"21m ago",1)]
def fmt(n): return f"{n:,}"
o='<div class="uctl"><div class="seg"><span class="on">Overview</span><span>Details</span></div><div class="seg sm">'
for k,l in PER: o+=f'<label><input type="radio" name="pd" id="pd-{k}"{" checked" if k=="today" else ""}><span>{l}</span></label>'
o+='</div></div>'
def tile(label,idx,color,sub=""):
    vs="".join(f'<span class="pv pv-{k}" title="{VAL[k][idx]}">{"~" if idx==4 else ""}{VAL[k][idx]}</span>' for k,_ in PER)
    return f'<div class="tl"><small>{label}</small><b style="color:{color}">{vs}</b>{sub}</div>'
o+='<div class="tiles5">'+tile("Total Requests",0,"var(--text)")+tile("Total Input Tokens",1,"var(--brand-500)")+tile("Cached Tokens",2,"#3b82f6")+tile("Output Tokens",3,"#10b981")+tile("Est. Cost",4,"#f59e0b",'<span class="sub">Estimated, not actual billing</span>')+'</div>'
rows="".join(f'<tr><td><i class="dt" style="background:{"#10b981" if ok else "#ef4444"}"></i></td><td class="mono">{m}</td><td class="n"><span style="color:var(--brand-500)">{fmt(i)}↑</span> <span style="color:#10b981">{fmt(out)}↓</span></td><td class="n m">{w}</td></tr>' for m,i,out,w,ok in REQ)
rr=f'<div class="card rr"><div class="rrh">Recent Requests</div><div class="rrb"><table><tr><th></th><th>Model</th><th class="n">In / Out</th><th class="n">When</th></tr>{rows}</table></div></div>'
graph=subprocess.check_output(["python3","topo.py","usage"],text=True)
o+=f'<div class="ug">{graph}{rr}</div>'
print(o)
