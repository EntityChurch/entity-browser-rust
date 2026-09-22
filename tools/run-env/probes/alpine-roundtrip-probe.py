#!/usr/bin/env python3
"""THE ROUND TRIP GATE -- page -> guest -> page, on the ALPINE guest, in a real iframe.

    python3 alpine-roundtrip-probe.py [url-to-host.html]

`x-put-files` / `x-get-file` were measured on the DEMO guest, whose root was a
9p tree with nothing in it. This guest is a real Alpine root with a real /mnt,
and a verb that worked against an empty tree is not evidence about this one.

WHAT MAKES IT A ROUND TRIP AND NOT A PAIR OF PUTS: the middle step. The GUEST'S
OWN SHELL reads what the page wrote and writes what the page reads back. A put
followed by a get proves v86's fs9p remembered a blob -- it does not prove the
bytes ever entered the guest's namespace, and that is the claim hop 3-4 makes.

Two payloads, and the second is the one that matters for a VM: 33 bytes of text
(the length the trailing-NUL transfer bug was measured on) and 256 bytes holding
EVERY byte value including NUL and 0xFF. A path that is clean for ASCII can
still be lossy for a tarball.

It also runs inside a real `sandbox="allow-scripts"` iframe -- an opaque origin,
our third-party app tier -- so it carries the first half of step 2 for free.

Exit 0 only if every assertion holds.
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
# ?boot=cold: the paint-during-boot check below needs a BOOT, and with a snapshot
# published beside the page a launch resumes in ~2 s with no boot to sample.
URL = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:8201/host.html?app=index.html%3Fboot%3Dcold"
BUDGET_S = 240


def rq(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(GRID + path, data=data, method=method,
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=300) as r:
        return json.loads(r.read() or b"{}")


caps = {"capabilities": {"alwaysMatch": {
    "browserName": "firefox",
    "moz:firefoxOptions": {"args": ["-headless"]},
}}}

print(f"\nROUND TRIP GATE -- {URL}\n")
sid = rq("POST", "/session", caps)["value"]["sessionId"]
res = None
try:
    rq("POST", f"/session/{sid}/url", {"url": URL})
    deadline = time.time() + BUDGET_S
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

if not res or not res.get("done"):
    print("NO RESULT -- the harness never finished within the budget.")
    print(json.dumps(res, indent=1)[:1500] if res else "(nothing reported)")
    sys.exit(1)

r = res.get("r") or {}
ls = r.get("listing_after_put") or []
guest = r.get("guest_out") or []
text = r.get("text") or {}
binr = r.get("bin") or {}
st = r.get("app_state") or {}

# The fit trace, read in the middle rather than at the end. "Painting" is rows
# beyond the 0x0 fallback AND characters actually on screen, comfortably before
# the prompt -- 2 s of margin so a fit that lands in the last moments of the boot
# cannot be scored as "painted throughout".
fit = st.get("fit") or []
prompt_ms = st.get("promptMs") or 0
cutoff = prompt_ms - 2000
early = [s for s in fit if s["t"] < cutoff and s["rows"] > 1 and s["chars"] > 0]
first_fit = next((s["t"] for s in fit if s["rows"] > 1), None)
fit_ok = bool(fit) and prompt_ms > 0 and bool(early)
fit_detail = (f"first fit {first_fit}ms, prompt {prompt_ms}ms, "
              f"{len(early)}/{len(fit)} samples painting before {cutoff}ms"
              f" · frame sized at {r.get('sized_at_ms')}ms")

checks = [
    ("the guest reached a prompt IN AN IFRAME",
     r.get("ready_ms") is not None,
     f"{r.get('ready_ms')}ms, sandbox={res.get('sandbox')}"),

    ("the listing is an ANSWER, not a timeout",
     not r.get("could_not_look") and bool(ls),
     "could_not_look" if r.get("could_not_look") else f"{len(ls)} entries"),

    ("the GUEST sees both files the page wrote",
     "text-in.txt" in ls and "bin-in.dat" in ls,
     json.dumps(ls)),

    ("the guest READ them -- wc -c reports 33 and 256",
     guest[:2] == ["33", "256"],
     json.dumps(guest)),

    ("text came back byte-exact, and TRANSFORMED",
     text.get("equal") is True and text.get("got") == text.get("declared"),
     f"{text.get('got')}B declared {text.get('declared')}B · {text.get('why', 'equal to tr a-z A-Z')}"),

    ("binary came back byte-exact -- all 256 values, NUL and 0xFF included",
     binr.get("equal") is True and binr.get("got") == binr.get("declared"),
     f"{binr.get('got')}B declared {binr.get('declared')}B · {binr.get('why', 'equal')}"),

    # The guest's own verbs (build-guest.sh 3c). A host file lands under its BASE
    # name -- `some/dir/pushed.txt` must not create directories in the guest --
    # and a file the guest `send`s arrives at the host byte-exact and base-named.
    ("host -> guest: an x-file lands in the home directory under its base name",
     "pushed by the host" in (r.get("pushed_seen") or []),
     json.dumps(r.get("pushed_seen"))),

    ("guest -> host: `send FILE` hands the host exactly those bytes",
     (r.get("guest_sent") or {}).get("equal") is True
     and (r.get("guest_sent") or {}).get("name") == "made.txt",
     json.dumps(r.get("guest_sent"))),

    # ⚠ THE END STATE CANNOT MEASURE THIS. At the prompt the page fits itself, so
    # a frame that painted nothing for eight seconds and one that painted
    # throughout are IDENTICAL afterwards -- an end-state version of this
    # assertion passed with every fit trigger deleted. The claim is about the
    # MIDDLE of the boot, so it is sampled there.
    ("the terminal painted DURING the boot, not at the prompt",
     fit_ok,
     fit_detail),
]

ok = True
for name, passed, detail in checks:
    ok &= bool(passed)
    print(f"  {'PASS' if passed else 'FAIL'}  {name:56} {detail}")

if r.get("error"):
    ok = False
    print(f"\n  harness error: {r['error']}")

print()
if not ok:
    print(json.dumps(r, indent=1)[:2500])
sys.exit(0 if ok else 1)
