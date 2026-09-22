import json, sys, time, urllib.request, subprocess
engine, GRID, URL, CT = sys.argv[1:5]
def rq(m, p, b=None):
    r = urllib.request.Request(GRID+p, data=json.dumps(b).encode() if b is not None else None, method=m, headers={"Content-Type":"application/json"})
    return json.loads(urllib.request.urlopen(r, timeout=120).read() or b"{}")
caps = {"browserName": engine}
sid = rq("POST","/session",{"capabilities":{"alwaysMatch":caps}})["value"]["sessionId"]
def procs():
    out = subprocess.run(["podman","exec",CT,"ps","-eo","pid,rss,args"],capture_output=True,text=True).stdout.splitlines()
    if engine=="chrome": rows=[l for l in out if "--type=renderer" in l]
    else: rows=[l for l in out if "-contentproc" in l or "firefox" in l.split()[-1:][0] if l.strip()]
    return rows
try:
    rq("POST",f"/session/{sid}/url",{"url":URL.replace("index.html","frame.html")}); time.sleep(4)
    b=procs(); print("no-iframe page (frame.html) procs:", len(b))
    rq("POST",f"/session/{sid}/url",{"url":URL}); time.sleep(6)
    a=procs(); print("probe page procs:", len(a))
    for l in a: 
        parts=l.split(); 
        flags=[x for x in parts if x.startswith(("--renderer-client-id","-isForBrowser","--site-per-process","tab","-childID")) or x in ("tab","webIsolated","web","isolated","privilegedabout")]
        print("  ", parts[0], "rss_kb="+parts[1], " ".join(parts[2:])[:60], "...", " ".join(parts[-4:])[:90])
finally:
    rq("DELETE",f"/session/{sid}")
