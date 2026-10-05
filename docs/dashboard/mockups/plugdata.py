import tomllib, glob, os
ROOT="/home/ali/Desktop/0router/plugins/bundled"
P={}
for f in sorted(glob.glob(ROOT+"/*.toml")):
    d=tomllib.load(open(f,"rb")); P[d["id"]]={"color":d["display"].get("color","#6b7280"),"icon":d["display"].get("text_icon",d["id"][:2].upper()),"models":d["models"]}
def color(n): return P.get(n,{}).get("color","#6b7280")
def icon(n): return P.get(n,{}).get("icon",n[:2].upper())
def models(n): return P.get(n,{}).get("models",[])

ALIAS={"opencode-zen":"opencode"}
def logo_html(n,size=32,radius=6,cls="lg",extra=""):
    """9router's tile: colour at 15% behind the real provider logo (providers/<id>.png, copied from 9router, gitignored); initials if there is none."""
    c=color(n); f=ALIAS.get(n,n); ok=os.path.exists(os.path.join(os.path.dirname(os.path.abspath(__file__)),"providers",f+".png"))
    inner=(f'<img src="providers/{f}.png" alt="{n}" style="max-width:{size-2}px;max-height:{size-2}px;object-fit:contain;border-radius:{radius}px">' if ok else f'<span style="color:{c};font-weight:700">{icon(n)}</span>')
    return f'<div class="{cls}" style="width:{size}px;height:{size}px;border-radius:{radius+2}px;background:{c}26;display:grid;place-items:center;flex:none;{extra}">{inner}</div>'
