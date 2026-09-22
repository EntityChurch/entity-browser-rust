#!/usr/bin/env python3
"""kolibri-probe.py -- the KolibriOS machine, standalone, end to end.

    python3 probes/plain-serve.py 8292 kolibri &
    GRID=http://127.0.0.1:4480 python3 probes/kolibri-probe.py http://127.0.0.1:8292/

What it proves, in order, and why each is not implied by the one before:
  1. the page boots KolibriOS to its desktop, with the clock shim on and the boot
     menu answered (the two things that make it ~4 s rather than ~33 s);
  2. the transfer floppy is in drive A and the PAGE can read it;
  3. a file the page PUTS IN is seen by the GUEST -- KolibriOS's own shell copies it
     to a new name on the same floppy -- and the page reads that copy back
     byte-identical. That is the only assertion that crosses the guest's FAT driver
     in both directions: our FAT12 + long names read by KolibriOS, and KolibriOS's
     write read by our reader. Anything short of it tests our code against itself.

The guest is driven the way a person drives it, through REAL browser input
(kolibri-input.py -- WebDriver actions, never emulator.bus or keyboard_send_text):
the pointer to the SHELL icon, a double click, typed keys. Before that it measures
that the guest pointer sits under the host pointer after a long path, which is
what VMMOUSE.SYS is for (field report 2026-09-14: the two drifted apart).
"""
import base64
import json
import os
import pathlib
import sys
import time
import urllib.request
from importlib import import_module

sys.path.insert(0, str(pathlib.Path(__file__).parent))
KI = import_module("kolibri-input")

GRID = os.environ.get("GRID", "http://127.0.0.1:4444")
URL = (sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:8292/").rstrip("/") + "/index.html" + os.environ.get("QUERY", "")
SHOTS = pathlib.Path(os.environ.get("SHOTS", "/tmp"))
BUDGET = float(os.environ.get("BUDGET_S", "90"))
# KolibriOS 0.7.7.0-9197's default desktop at 1024x768: the SHELL icon.
SHELL_ICON = (853, 163)

fails = 0


def rq(method, path, body=None, timeout=300):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(GRID + path, data=data, method=method, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read() or b"{}")


def check(ok, what, detail=""):
    global fails
    print(f"  {'PASS' if ok else 'FAIL'}  {what:58} {str(detail)[:160]}")
    if not ok:
        fails += 1
    return ok


caps = {"capabilities": {"alwaysMatch": {"browserName": "firefox", "moz:firefoxOptions": {"args": ["-headless"]}}}}
sid = rq("POST", "/session", caps)["value"]["sessionId"]
js = lambda script, *a: rq("POST", f"/session/{sid}/execute/sync", {"script": script, "args": list(a)})["value"]


def until(expr, budget, poll=0.25):
    t = time.time()
    while time.time() - t < budget:
        v = js("return (" + expr + ")")
        if v:
            return v
        time.sleep(poll)
    return None


def shot(name):
    p = SHOTS / f"kolibri-{name}.png"
    p.write_bytes(base64.b64decode(rq("GET", f"/session/{sid}/screenshot")["value"]))
    return p


try:
    rq("POST", f"/session/{sid}/window/rect", {"width": 1280, "height": 900})
    print(f"==> {URL}")
    rq("POST", f"/session/{sid}/url", {"url": URL})
    t0 = time.time()

    print("-- boot --")
    ready = until("window.__m1 && window.__m1.floppy && window.__m1.floppy.lastSwap && window.__m1.desktopMs", BUDGET)
    m = js("return {desktopMs: __m1.desktopMs, bootMenuMs: __m1.bootMenuMs || null, shim: !!__m1.clockShim, snap: __m1.snapshot, err: __m1.error, status: document.getElementById('status').textContent}")
    check(bool(ready), "the desktop came up", f"{m['desktopMs']} ms in the page, {time.time() - t0:.1f} s wall; {m['status']}")
    check(m["shim"], "the clock shim is on (the origin is not isolated)")
    if not m["snap"]["used"]:
        check(m["bootMenuMs"] is not None, "the boot menu was answered with Enter", f"at {m['bootMenuMs']} ms")
    print(f"        snapshot: {m['snap']['why']}")
    if not ready:
        shot("no-desktop")
        raise SystemExit(1)

    print("-- the floppy, page side --")
    names = js("return __m1.floppy.names")
    check("README.TXT" in names, "a fresh transfer floppy is in drive A, readable by the page", names)

    note = "written by the page " + str(int(time.time()))
    js("putIn([{name: 'page-note.txt', data: new TextEncoder().encode(arguments[0])}]); return 1", note)
    until("__m1.floppy.received.includes('page-note.txt')", 10)
    names = js("return (listFloppy(), __m1.floppy.names)")
    check("page-note.txt" in names, "a file the page puts in is on the floppy, long name intact", names)

    print("-- real input: pointer --")
    # The image's VMMOUSE.SYS loads from autorun, a little after the desktop first settles.
    mode = until("__m1.pointer && __m1.pointer.mode === 'absolute' && __m1.pointer.mode", 30)
    check(mode == "absolute", "VMMOUSE.SYS put the guest pointer in absolute mode", js("return __m1.pointer"))
    scr = js("return __m1.screen")
    check(scr and scr["smoothing"] == (abs(scr["scale"] - round(scr["scale"])) >= 0.01 or scr["scale"] < 1),
          "the screen is smoothed at a non-integer scale, pixelated at a whole one", scr)
    # The launcher draws the desktop icons a few seconds after the screen first
    # settles, so wait for them before aiming at one.
    time.sleep(4)
    inp = KI.Input(rq, sid, js)
    # A long wander between parking and target: a relative mouse drifts on exactly
    # this (the field report); with the driver the guest cursor ends under the host's.
    # Measured without the driver: the cursor was not within 120 px of any target.
    for target in [(300, 250), (600, 520), (150, 550)]:
        err, where = inp.pointer_error((950, 80), target, path=[(100, 100), (900, 600), (50, 700), (512, 384)])
        check(err is not None and err <= 3, f"after a long path the guest pointer is on {target}", f"off by {err} px, {where}")

    print("-- real input: click and keys --")
    inp.click(*SHELL_ICON, times=2)
    time.sleep(3)
    shot("shell-open")
    got = None
    for attempt in (1, 2):
        inp.type("cp /fd/1/page-note.txt /fd/1/guest-copy.txt\n")
        got = until("(listFloppy(), __m1.floppy.names.includes('guest-copy.txt'))", 25, poll=1)
        if got:
            break
        print(f"        attempt {attempt}: no copy yet (a dropped key?)")
    shot("after-cp")
    check(bool(got), "KolibriOS copied the page's file to a new name on the floppy", js("return __m1.floppy.names"))
    if got:
        same = js("""const f = EntityFat.readFloppy(emulator.get_disk_fda());
                     const a = f.find(x => x.path === 'page-note.txt'), c = f.find(x => x.path === 'guest-copy.txt');
                     return a && c && a.size === c.size && a.data.every((v, i) => v === c.data[i]);""")
        check(same, "the guest's copy reads back byte-identical through the page's reader")

    print("-- chrome --")
    about = js("document.getElementById('info').click(); const p = document.getElementById('aboutpanel'); return {open: !p.hidden, text: p.textContent}")
    check(about["open"] and "GPL-2.0" in about["text"] and "b0055ba4721a" in about["text"],
          "ⓘ names KolibriOS, its licence and the source commit", about["text"][:90])
    js("document.body.click(); return 1")
    menu = js("document.getElementById('power').click(); return [...document.querySelectorAll('#powermenu [data-power]')].filter(b => !b.hidden).map(b => b.dataset.power)")
    check(menu == ["restart", "cold", "off"], "⏻ offers restart, restart showing the boot, turn off", menu)
    print(f"        screenshots: {SHOTS}/kolibri-*.png")
finally:
    try:
        rq("DELETE", f"/session/{sid}")
    except Exception:
        pass

print("*** KOLIBRIOS RUNS AND MOVES FILES BOTH WAYS ***" if not fails else f"*** {fails} FAILED ***")
sys.exit(1 if fails else 0)
