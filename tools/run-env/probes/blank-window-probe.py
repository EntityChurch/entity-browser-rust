#!/usr/bin/env python3
"""THE BLANK WINDOW -- what does a person see before the guest says anything?

    python3 blank-window-probe.py [url] [--warm]

The kernel log scrolling by is good feedback. The window BEFORE it is the one
that gets the back button: 14.3 MiB has to arrive before the guest emits a
single byte, and the operator's concern is exactly that -- "it's a black box
until it boots, that's when people hit back and don't know it just needed a few
more seconds."

So this samples THE STATUS LINE, at 200ms, from load to prompt, and reports what
it said and for how long. COLD CACHE BY DEFAULT (a fresh profile per run), because
a warm profile skips the download that is the whole subject -- measuring the
blank window against a cache that removes it is measuring nothing.

Assertions:
  * the line is never silent for longer than SILENT_BUDGET_MS
  * it QUANTIFIES the download (a number that moves), not just "please wait"
  * it hands over to the kernel log and then to ready
Exit 0 only if all hold.
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
WARM = "--warm" in sys.argv
SILENT_BUDGET_MS = 2500


def rq(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(GRID + path, data=data, method=method,
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=300) as r:
        return json.loads(r.read() or b"{}")


# A fresh profile is the cold cache. Selenium gives each session one by default;
# what would warm it is REUSING a session, which is exactly what --warm does.
sid = rq("POST", "/session", {"capabilities": {"alwaysMatch": {
    "browserName": "firefox", "moz:firefoxOptions": {"args": ["-headless"]}}}})["value"]["sessionId"]

# ⚠ SAMPLE THE WHOLE SCREEN, NOT THE STATUS LINE. During the boot the kernel log
# IS the feedback and the status line is deliberately static -- an assertion
# against the line alone reports 5s of "nothing happening" over a terminal that
# is scrolling, which is the opposite of the claim. What the operator's concern
# is actually about is a stretch where NOTHING on screen changes.
SAMPLE = """
const s = document.getElementById('status');
const rows = document.querySelector('.xterm-rows');
const term = rows ? rows.innerText : '';
return { t: Math.round(performance.now()),
         text: s ? s.textContent : null,
         cls: s ? s.className : null,
         // a cheap change detector: length plus the tail, so a scrolling log
         // that keeps its length still counts as movement
         termLen: term.replace(/\\s+/g, '').length,
         termTail: term.replace(/\\s+/g, ' ').trim().slice(-60),
         m1: window.__m1 ? { phases: window.__m1.phases, dl: window.__m1.dl,
                             promptMs: window.__m1.promptMs } : null };
"""

samples = []
try:
    if WARM:
        rq("POST", f"/session/{sid}/url", {"url": URL})
        time.sleep(20)
    rq("POST", f"/session/{sid}/url", {"url": URL})
    t_start = time.time()
    while time.time() - t_start < 90:
        s = rq("POST", f"/session/{sid}/execute/sync", {"script": SAMPLE, "args": []})["value"]
        samples.append(s)
        if s.get("m1") and s["m1"].get("promptMs"):
            break
        time.sleep(0.2)
finally:
    try:
        rq("DELETE", f"/session/{sid}")
    except Exception:
        pass

print(f"\nBLANK WINDOW -- {URL}  ({'warm' if WARM else 'COLD cache'})\n")
if not samples:
    print("nothing sampled")
    sys.exit(1)

t0 = samples[0]["t"]


def runs_over(key):
    out, prev = [], object()
    for s in samples:
        v = key(s)
        if v != prev:
            out.append({"from": s["t"] - t0, "v": v, "s": s})
        prev = v
    return out


end = samples[-1]["t"] - t0
status_runs = runs_over(lambda s: s.get("text"))
# what a PERSON sees: the status line or the terminal, either one moving counts
screen_runs = runs_over(lambda s: (s.get("text"), s.get("termLen"), s.get("termTail")))

for i, r in enumerate(status_runs):
    to = status_runs[i + 1]["from"] if i + 1 < len(status_runs) else end
    print(f"  {r['from']:>6} - {to:>6} ms  ({to - r['from']:>5} ms)  "
          f"[{r['s'].get('cls')}]  {r['v']!r}")

m1 = samples[-1].get("m1") or {}
print(f"\n  phases: {json.dumps(m1.get('phases'))}")
print(f"  download: {json.dumps(m1.get('dl'))}\n")

# the longest stretch in which NOTHING on screen changed
longest, longest_at, longest_text = 0, 0, None
for i, r in enumerate(screen_runs):
    to = screen_runs[i + 1]["from"] if i + 1 < len(screen_runs) else end
    if to - r["from"] > longest:
        longest, longest_at, longest_text = to - r["from"], r["from"], r["s"].get("text")

texts = [r["v"] or "" for r in status_runs]
quantified = [t for t in texts if "MiB" in t and "ready" not in t]

checks = [
    ("nothing on screen is ever frozen for long",
     longest <= SILENT_BUDGET_MS,
     f"longest frozen stretch {longest}ms at t={longest_at} (budget {SILENT_BUDGET_MS}), "
     f"line was {longest_text!r}"),
    ("the download is QUANTIFIED, not just 'please wait'",
     len(quantified) >= 2,
     f"{len(quantified)} distinct quantified states, e.g. {quantified[:2]}"),
    ("it hands over to the kernel log, then to ready",
     any("booting" in t for t in texts) and any("ready in" in t for t in texts),
     f"{len(status_runs)} status states, prompt at {m1.get('promptMs')}ms"),
]
ok = True
for name, passed, detail in checks:
    ok &= bool(passed)
    print(f"  {'PASS' if passed else 'FAIL'}  {name:48} {detail}")
print()
sys.exit(0 if ok else 1)
