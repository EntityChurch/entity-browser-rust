#!/usr/bin/env python3
"""Print the app's fit trace from a run of host.html -- the A/B for the ResizeObserver.

    python3 fit-trace.py [url]

The end-state assertion cannot tell the two arms apart, because at the prompt the
page fits itself. This prints the MIDDLE of the boot, which is where they differ.
"""
import json
import os
import sys
import time
import urllib.request

# GRID=http://127.0.0.1:NNNN to point at a grid other than :4444 -- on a shared box
# :4444 is usually another session's, and a probe that can only reach it either
# queues behind their run or steals their slot.
GRID = os.environ.get("GRID", "http://127.0.0.1:4444")
URL = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:8201/host.html"


def rq(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(GRID + path, data=data, method=method,
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=300) as r:
        return json.loads(r.read() or b"{}")


sid = rq("POST", "/session", {"capabilities": {"alwaysMatch": {
    "browserName": "firefox", "moz:firefoxOptions": {"args": ["-headless"]}}}})["value"]["sessionId"]
res = None
try:
    rq("POST", f"/session/{sid}/url", {"url": URL})
    deadline = time.time() + 240
    while time.time() < deadline:
        res = rq("POST", f"/session/{sid}/execute/sync",
                 {"script": "return window.__host || null;", "args": []})["value"]
        if res and res.get("done"):
            break
        time.sleep(0.5)
finally:
    try:
        rq("DELETE", f"/session/{sid}")
    except Exception:
        pass

r = (res or {}).get("r") or {}
st = r.get("app_state") or {}
fit = st.get("fit") or []
print(f"\nFIT TRACE -- {URL}")
print(f"frame sized at {r.get('sized_at_ms')}ms · prompt at {st.get('promptMs')}ms · {len(fit)} samples\n")
print(f"{'t(ms)':>7} {'termH':>6} {'rows':>5} {'cols':>5} {'painted':>8} {'chars':>7}")
prev = None
for s in fit:
    row = (s["h"], s["rows"], s["cols"], s["painted"])
    # only print transitions plus a sample every second -- a flat trace is noise
    if row != prev or s["t"] % 1000 < 250:
        print(f"{s['t']:>7} {s['h']:>6} {s['rows']:>5} {s['cols']:>5} {s['painted']:>8} {s['chars']:>7}")
    prev = row
