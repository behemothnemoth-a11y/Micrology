import json, pathlib, math
root=pathlib.Path(r"C:\Users\behem\.chatgpt-worktrees\Micrology-house-joint-load\target\diagnostics\destruction_lab")
names=["autoj_after_failure-tick000000.json","autoj_after_25-tick000025.json","autoj_after_120-tick000120.json"]
docs={n:json.loads((root/n).read_text(encoding="utf-8")) for n in names}
def by_id(d):
    return {x["id"]:x for x in d["fragment_dynamics"]}
base=by_id(docs[names[0]])
for n in names:
    d=docs[n]; cur=by_id(d)
    print("\n",n)
    print("fragments",len(cur),"contacts",d["contacts"])
    rows=[]
    for fid,x in cur.items():
        b=base.get(fid,x)
        p=x["translation"]; q=b["translation"]
        disp=math.sqrt(sum((float(p[i])-float(q[i]))**2 for i in range(3)))
        vel=x["linear_velocity"]
        speed=math.sqrt(sum(float(v)**2 for v in vel))
        rows.append((int(x["cells"]),fid,disp,p,vel,speed,x["state"]))
    rows.sort(reverse=True)
    for row in rows[:10]:
        print("cells=%d id=%s disp=%.4f pos=%s vel=%s speed=%.4f state=%s"%row)
