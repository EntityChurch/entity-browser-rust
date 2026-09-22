#!/usr/bin/env python3
"""drive.py ENGINE GRID URL OUT  -- opens URL in a fresh WebDriver session, waits for window.__RESULT__, saves JSON."""
import json, sys, time, urllib.request
engine, GRID, URL, OUT = sys.argv[1:5]
def rq(method, path, body=None, timeout=300):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(GRID + path, data=data, method=method, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read() or b"{}")
if engine == "firefox":
    caps = {"browserName": "firefox"}
else:
    caps = {"browserName": "chrome", "goog:chromeOptions": {"args": ["--no-sandbox"]}}
sid = rq("POST", "/session", {"capabilities": {"alwaysMatch": caps}})["value"]
bv = sid["capabilities"].get("browserVersion"); sid = sid["sessionId"]
try:
    js = lambda s: rq("POST", f"/session/{sid}/execute/sync", {"script": s, "args": []})["value"]
    rq("POST", f"/session/{sid}/window/rect", {"width": 1280, "height": 900})
    rq("POST", f"/session/{sid}/url", {"url": URL})
    t0 = time.time(); last = None
    while time.time() - t0 < 400:
        v = js("return window.__RESULT__ ? JSON.stringify(window.__RESULT__) : null")
        if v:
            res = json.loads(v); res["browserVersion"] = bv; res["engine"] = engine; res["url"] = URL
            open(OUT, "w").write(json.dumps(res, indent=1))
            print(f"{engine} {URL} done in {time.time()-t0:.0f}s -> {OUT}", flush=True); break
        p = js("return window.__PROGRESS__ || null")
        if p != last: print(f"  [{time.time()-t0:5.1f}s] {p}", flush=True); last = p
        time.sleep(1)
    else:
        print("TIMEOUT waiting for __RESULT__", flush=True)
finally:
    rq("DELETE", f"/session/{sid}")
