#!/usr/bin/env python3
"""package-layer-probe.py -- what it costs to put installed packages back on a launch
(PLAN-2026-09-13 §11).

    python3 probes/plain-serve.py 8216 alpine-guest &
    GRID=http://127.0.0.1:4480 python3 probes/package-layer-probe.py http://127.0.0.1:8216/

Two questions, on the standalone page (resumes when the snapshot fits):
  1. REPLAY IN THE GUEST: `apk add` timings, first and again with the bytes
     already held, and tar -c / tar -x of exactly what git + python3 installed --
     the two guest-CPU ways to restore a package layer.
  2. GRAFT FROM THE PAGE: `emulator.create_file` after the resume, used from the
     guest with no cache drop, for a new name, a name the guest already looked up
     and missed, and a directory it already listed.
Needs git python3 vim tmux jq in the package set. Prints; asserts nothing.
"""
import json, os, re, secrets, sys, time, urllib.request

GRID = os.environ.get("GRID", "http://127.0.0.1:4444")
URL = (sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:8216/").rstrip("/") + "/index.html"
STRIP = "replace(/\\x1b\\[[0-9;?]*[a-zA-Z]/g, '')"


def rq(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(GRID + path, data=data, method=method, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=600) as r:
        return json.loads(r.read() or b"{}")


def js(sid, s):
    return rq("POST", f"/session/{sid}/execute/sync", {"script": s, "args": []})["value"]


def poll(sid, s, ok, budget, every=0.5):
    deadline, last = time.time() + budget, None
    while time.time() < deadline:
        last = js(sid, s)
        if ok(last):
            return last
        time.sleep(every)
    return last


def guest(sid, cmd, budget=600):
    """Type a command; return its output lines (the echo and the marker removed)."""
    tag = "T" + secrets.token_hex(4)
    js(sid, f"window.__probeFrom = (window.__m1.serial || '').length; window.emulator.serial_send_bytes(0, "
            f"new TextEncoder().encode({json.dumps(cmd + '; echo ' + tag + '-$((1+1))DONE')} + '\\n')); return 1;")
    out = poll(sid, f"const s = (window.__m1.serial || '').slice(window.__probeFrom).{STRIP};"
                    f"return s.includes({json.dumps(tag + '-2DONE')}) ? s : null;", bool, budget) or ""
    out = out.encode("latin-1", "replace").decode("utf-8", "replace")
    i = out.find(tag + "-$((1+1))DONE")
    lines = [l for l in (out[i:] if i >= 0 else out).replace("\r", "").split("\n") if l.strip()][1:]
    return [l for l in lines if tag not in l and not l.rstrip().endswith("$")]


def timed(sid, name, cmd):
    lines = guest(sid, f'S=$(date +%s%N); {cmd} >/tmp/o.log 2>&1; rc=$?; E=$(date +%s%N); '
                       f'echo "RES""ULT rc=$rc ms=$(( (E-S)/1000000 ))"')
    m = re.search(r"RESULT rc=(\d+) ms=(\d+)", " ".join(lines))
    print(f"  {name:44} " + (f"rc={m.group(1)} {int(m.group(2)) / 1000:6.1f} s" if m else f"NO RESULT {lines[-3:]}"))


def graft(sid, path, text):
    js(sid, "window.__g = null; window.emulator.create_file(%s, new TextEncoder().encode(%s))"
            ".then(() => window.__g = 'ok', e => window.__g = String(e)); return 1;" % (json.dumps(path), json.dumps(text)))
    return poll(sid, "return window.__g;", bool, 10)


caps = {"capabilities": {"alwaysMatch": {"browserName": "firefox", "moz:firefoxOptions": {"args": ["-headless"]}}}}
sid = rq("POST", "/session", caps)["value"]["sessionId"]
try:
    rq("POST", f"/session/{sid}/url", {"url": URL})
    ready = poll(sid, "const m = window.__m1 || {}; return (m.snapshot && m.snapshot.readyMs) || m.promptMs || null;", bool, 180)
    print(f"\nPACKAGE LAYER PROBE -- {URL}\n  ready {ready} ms, resumed: {js(sid, 'return !!(window.__m1.snapshot && window.__m1.snapshot.used)')}")
    time.sleep(2)
    print("\n1. replay in the guest")
    timed(sid, "apk add git", "apk add --no-cache git")
    timed(sid, "apk add python3", "apk add --no-cache python3")
    timed(sid, "tar -c what git + python3 installed",
          "apk info -L git python3 $(apk info -R git python3 | grep -v : | sort -u) | grep -v contains: | grep . | sort -u "
          "| while read f; do [ -f \"/$f\" ] && echo \"$f\"; done > /tmp/list; tar -C / -cf /tmp/w.tar -T /tmp/list")
    print("  " + " ".join(guest(sid, "echo files=$(wc -l < /tmp/list) bytes=$(wc -c < /tmp/w.tar)")))
    timed(sid, "tar -x the same archive", "mkdir -p /tmp/x && tar -xf /tmp/w.tar -C /tmp/x")
    timed(sid, "apk del git python3", "apk del git python3")
    timed(sid, "apk add git python3, bytes already held", "apk add --no-cache git python3")
    timed(sid, "apk del git python3", "apk del git python3")
    timed(sid, "apk add git python3 vim tmux jq", "apk add --no-cache git python3 vim tmux jq")
    print("\n2. graft from the page after the resume (no cache drop)")
    print("  A new name:           ", graft(sid, "usr/local/bin/ovl-a", "#!/bin/sh\necho GRAFT-A\n"),
          guest(sid, "chmod +x /usr/local/bin/ovl-a; ovl-a 2>&1"))
    print("  B looked up, missed:  ", guest(sid, "ls /usr/local/bin/ovl-b 2>&1; ovl-b 2>&1")[-1:],
          graft(sid, "usr/local/bin/ovl-b", "#!/bin/sh\necho GRAFT-B\n"),
          guest(sid, "chmod +x /usr/local/bin/ovl-b; ovl-b 2>&1"))
    print("  C listed directory:   ", guest(sid, "mkdir -p /usr/share/ovl-c; ls -A /usr/share/ovl-c | wc -l"),
          graft(sid, "usr/share/ovl-c/c.txt", "GRAFT-C\n"),
          guest(sid, "ls -A /usr/share/ovl-c; cat /usr/share/ovl-c/c.txt"))
finally:
    try:
        rq("DELETE", f"/session/{sid}")
    except Exception:
        pass
