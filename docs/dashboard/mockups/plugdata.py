import tomllib, glob, os
ROOT="/home/ali/Desktop/0router/plugins/bundled"
P={}
for f in sorted(glob.glob(ROOT+"/*.toml")):
    d=tomllib.load(open(f,"rb")); P[d["id"]]={"color":d["display"].get("color","#6b7280"),"icon":d["display"].get("text_icon",d["id"][:2].upper()),"models":d["models"]}
def color(n): return P.get(n,{}).get("color","#6b7280")
def icon(n): return P.get(n,{}).get("icon",n[:2].upper())
def models(n): return P.get(n,{}).get("models",[])
