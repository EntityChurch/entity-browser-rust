#!/usr/bin/env python3
"""THE 44x REFETCH -- measured, and now diagnosed.

    python3 refetch-probe.py [url] [--no-store]

`…-l` measured that v86 re-requests the same blob up to 44 times and that whether
that is free or catastrophic is decided entirely by HTTP caching: heuristic
caching 21.4 MiB, `Cache-Control: no-store` 56.1 MiB. It did not diagnose WHY.

The why is in the engine: `N.prototype.CloseInode` calls
`this.storage.uncache(sha256sum)`, so a file is dropped from memory when the
guest CLOSES it -- and every busybox applet invocation re-opens one.

--no-store points this at a server that forbids caching, which is the
configuration that turns the refetch into transfer. That is the arm that matters:
with the browser cache absorbing it, a fix and no fix look nearly identical.
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
args = [a for a in sys.argv[1:] if not a.startswith("--")]
URL = args[0] if args else "http://127.0.0.1:8212/"


def rq(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(GRID + path, data=data, method=method,
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=300) as r:
        return json.loads(r.read() or b"{}")


READ = """
const m = window.__m1 || {};
return { promptMs: m.promptMs || null, net: m.net || null, retain: m.retain || null,
         probeOk: !!m.probeOk };
"""

sid = rq("POST", "/session", {"capabilities": {"alwaysMatch": {
    "browserName": "firefox", "moz:firefoxOptions": {"args": ["-headless"]}}}})["value"]["sessionId"]
res = None
try:
    rq("POST", f"/session/{sid}/url", {"url": URL})
    deadline = time.time() + 240
    while time.time() < deadline:
        res = rq("POST", f"/session/{sid}/execute/sync", {"script": READ, "args": []})["value"]
        if res and res.get("promptMs"):
            break
        time.sleep(0.5)
finally:
    try:
        rq("DELETE", f"/session/{sid}")
    except Exception:
        pass

if not res or not res.get("promptMs"):
    print("no prompt within the budget")
    print(json.dumps(res, indent=1)[:1200] if res else "")
    sys.exit(1)

net = res.get("net") or {}
b = net.get("blobs") or {}
r = res.get("retain") or {}
total = sum((net.get(k) or {}).get("bytes", 0) for k in ("blobs", "kernel", "engine", "index"))

print(f"\nREFETCH -- {URL}")
print(f"  prompt                  {res['promptMs']/1000:.1f}s")
print(f"  blob requests           {b.get('n')}")
print(f"  distinct blobs          {b.get('distinct')}")
print(f"  REPEATS                 {b.get('repeats')}")
print(f"  blob bytes transferred  {(b.get('bytes') or 0)/1048576:.1f} MiB")
print(f"  cache hits (0-byte)     {b.get('cacheHits')}")
print(f"  TOTAL transferred       {total/1048576:.1f} MiB")
if r.get("installed"):
    # installing AFTER the first read would retain nothing and still report
    # "installed" -- so the landing time is part of the evidence, not trivia
    print(f"\n  retaining storage       installed at {r.get('installed_at_ms')}ms")
    print(f"                          hits {r.get('hits')} · misses {r.get('misses')} · "
          f"deduped {r.get('deduped')}")
    print(f"  evictions suppressed    {r.get('evictions_suppressed')}")
    print(f"  bytes held              {(r.get('bytes_held') or 0)/1048576:.1f} MiB")
else:
    print(f"\n  retaining storage       NOT INSTALLED ({r.get('why')})")
print()
