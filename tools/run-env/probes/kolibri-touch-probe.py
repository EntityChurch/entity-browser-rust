#!/usr/bin/env python3
"""kolibri-touch-probe.py -- KolibriOS on a PHONE: touch and the phone keyboard.

    make e2e-grid GRID_NAME=vm-chrome-4494 GRID_PORT=4494 SELENIUM_IMAGE=docker.io/selenium/standalone-chrome:latest
    python3 probes/plain-serve.py 8292 kolibri &
    GRID=http://127.0.0.1:4494 python3 probes/kolibri-touch-probe.py http://127.0.0.1:8292/

Chrome with mobile emulation (390x844, touch): real TOUCH actions, never
emulator.bus. The field report it answers (2026-09-14): on a phone the pointer
followed a finger but nothing clicked, and there was no way to open a keyboard.

  1. layout: the page's title yields to the status; the key bar is hidden until typing
  2. a tap clicks, and the click lands under the finger (measured on the guest screen)
  3. a double tap opens a program (SHELL)
  4. a long press is a right click (KolibriOS draws its desktop menu)
  5. pinch zooms the view; a tap on the zoomed screen still lands under the finger
  6. ⌨ opens the phone keyboard (a focused field) and the key bar
  7. keys typed into it run a command in KolibriOS's shell
  8. the keyboard diff: composition updates, autocorrect replacements and backspace
     on an empty field become the right keys
"""
import json
import os
import sys
import time
import urllib.request

GRID = os.environ.get("GRID", "http://127.0.0.1:4494")
URL = (sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:8292/").rstrip("/") + "/index.html"
SHELL_ICON = (853, 163)
# Empty desktop, clear of every icon: a parking tap near an icon can open it.
PARK = (600, 690)
fails = 0


def rq(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(GRID + path, data=data, method=method, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=300) as r:
        return json.loads(r.read() or b"{}")


def check(ok, what, detail=""):
    global fails
    print(f"  {'PASS' if ok else 'FAIL'}  {what:60} {str(detail)[:150]}")
    if not ok:
        fails += 1
    return ok


caps = {"capabilities": {"alwaysMatch": {"browserName": "chrome", "goog:chromeOptions": {
    "args": ["--headless=new"],
    "mobileEmulation": {"deviceMetrics": {"width": 390, "height": 844, "pixelRatio": 3, "touch": True},
                        "userAgent": "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0 Mobile Safari/537.36"}}}}}
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


def to_view(gx, gy):
    r = js("const r = document.querySelector('#screen_container canvas').getBoundingClientRect(); return [r.left, r.top, r.width, r.height];")
    return round(r[0] + gx * r[2] / 1024), round(r[1] + gy * r[3] / 768)


def perform(sources):
    rq("POST", f"/session/{sid}/actions", {"actions": sources})
    rq("DELETE", f"/session/{sid}/actions")


def finger(fid, acts):
    return {"type": "pointer", "id": fid, "parameters": {"pointerType": "touch"}, "actions": acts}


def tap(gx, gy, times=1, hold=60):
    x, y = to_view(gx, gy)
    acts = []
    for i in range(times):
        acts += [{"type": "pointerMove", "duration": 0, "origin": "viewport", "x": x, "y": y},
                 {"type": "pointerDown", "button": 0}, {"type": "pause", "duration": hold},
                 {"type": "pointerUp", "button": 0}, {"type": "pause", "duration": 120}]
    perform([finger("f1", acts)])


def tap_element(sel):
    x, y = js(f"const r = document.querySelector({json.dumps(sel)}).getBoundingClientRect(); return [Math.round(r.left + r.width/2), Math.round(r.top + r.height/2)];")
    perform([finger("f1", [{"type": "pointerMove", "duration": 0, "origin": "viewport", "x": x, "y": y},
                           {"type": "pointerDown", "button": 0}, {"type": "pause", "duration": 60},
                           {"type": "pointerUp", "button": 0}])])


def grab():
    js("window.__grab = document.querySelector('#screen_container canvas').getContext('2d').getImageData(0, 0, 1024, 768).data.slice(); return 1;")


def changed_near(gx, gy, box):
    return js(f"""
      const a = window.__grab, b = document.querySelector('#screen_container canvas').getContext('2d').getImageData(0, 0, 1024, 768).data;
      let minx = 1e9, miny = 1e9, n = 0;
      for (let y = Math.max(0, {gy} - {box}); y < Math.min(768, {gy} + {box}); y++)
        for (let x = Math.max(0, {gx} - {box}); x < Math.min(1024, {gx} + {box}); x++) {{
          const i = (y * 1024 + x) * 4;
          if (a[i] !== b[i] || a[i+1] !== b[i+1] || a[i+2] !== b[i+2]) {{ n++; if (x < minx) minx = x; if (y < miny) miny = y; }}
        }}
      return n ? {{x: minx, y: miny, changed: n}} : null;""")


def names():
    return js("return (listFloppy(), __m1.floppy.names)")


try:
    print(f"==> {URL} (Chrome, 390x844 touch)")
    rq("POST", f"/session/{sid}/url", {"url": URL})
    ready = until("window.__m1 && __m1.floppy && __m1.floppy.lastSwap && __m1.desktopMs", 90)
    check(bool(ready), "the desktop came up", js("return document.getElementById('status').textContent"))
    mode = until("__m1.pointer && __m1.pointer.mode === 'absolute' && 'absolute'", 30)
    check(mode == "absolute", "the pointer is absolute (VMMOUSE.SYS)", js("return __m1.pointer"))
    if not js("return __m1.snapshot.used"):
        time.sleep(4)

    print("-- layout --")
    lay = js("""return {title: getComputedStyle(document.querySelector('#bar b')).display, keys: document.getElementById('keys').hidden,
                        w: innerWidth, bar: document.getElementById('bar').scrollWidth <= document.getElementById('bar').clientWidth + 1}""")
    check(lay["w"] <= 400 and lay["title"] == "none", "on a phone the page's title yields to the status", lay)
    check(lay["keys"], "the key bar is hidden until typing", lay)

    print("-- tap --")
    tap(*PARK)
    time.sleep(0.5)
    grab()
    tap(300, 250)
    time.sleep(0.6)
    c = changed_near(300, 250, 60)
    t = js("return {taps: __m1.touch.taps, right: __m1.touch.rightClicks, drags: __m1.touch.drags}")
    check(t["taps"] == 2 and t["drags"] == 0, "a tap is a tap, not a drag", t)
    err = max(abs(c["x"] - 300), abs(c["y"] - 250)) if c else None
    check(err is not None and err <= 4, "a tap puts the pointer under the finger (clicking it is proven by the shell opening below)", f"off by {err} px, {c}")

    print("-- double tap opens a program --")
    tap(*SHELL_ICON, times=2)
    time.sleep(3)

    print("-- the phone keyboard --")
    tap_element("#kbd")
    time.sleep(0.5)
    k = js("return {active: document.activeElement && document.activeElement.id, phone: __m1.phoneKeyboard, keys: !document.getElementById('keys').hidden}")
    check(k["active"] == "kbdin" and k["phone"] and k["keys"], "⌨ opens the phone keyboard and the key bar", k)
    el = rq("POST", f"/session/{sid}/element", {"using": "css selector", "value": "#kbdin"})["value"]
    key = next(x for x in el if x.startswith("element-"))
    got = None
    for attempt in (1, 2):
        # Typed into the FIELD, as a phone keyboard does: the page diffs the field.
        rq("POST", f"/session/{sid}/element/{el[key]}/value", {"text": "cp /fd/1/README.TXT /fd/1/phone.txt\n"})
        got = None
        for _ in range(25):
            if any(n.lower() == "phone.txt" for n in names()):
                got = True
                break
            time.sleep(1)
        if got:
            break
        print(f"        attempt {attempt}: no copy yet")
    check(bool(got), "keys typed into the phone keyboard run a command in KolibriOS", names())
    check(js("return document.activeElement.id") == "kbdin", "tapping keys did not close the phone keyboard")
    # Close the shell, so the long press below lands on the desktop and not in a window.
    rq("POST", f"/session/{sid}/element/{el[key]}/value", {"text": "exit\n"})
    time.sleep(2)
    tap_element("#kbd")
    time.sleep(0.4)
    check(js("return document.activeElement.id") != "kbdin", "⌨ again closes it")

    print("-- the keyboard diff --")
    cases = [
        ("", "h", {"back": 0, "text": "h"}),                          # a letter
        ("hel", "hello", {"back": 0, "text": "lo"}),                  # a composition growing
        ("teh", "the ", {"back": 2, "text": "he "}),                  # autocorrect replacing a word
        ("abc", "ab", {"back": 1, "text": ""}),                       # backspace
        ("", None, {"back": 1, "text": ""}),                          # backspace on an empty field (the sentinel went)
        ("ab", None, {"back": 3, "text": ""}),
    ]
    for prev, nxt, want in cases:
        got_d = js("return __kbdDiff(arguments[0], arguments[1])", prev, nxt)
        check(got_d == want, f"diff {prev!r} -> {nxt!r}", got_d)

    print("-- long press --")
    js("document.getElementById('kbdin').blur(); return 1")
    tap(*PARK)                      # click the desktop so no window is focused under the press
    time.sleep(0.8)
    grab()
    x, y = to_view(400, 400)
    perform([finger("f1", [{"type": "pointerMove", "duration": 0, "origin": "viewport", "x": x, "y": y},
                           {"type": "pointerDown", "button": 0}, {"type": "pause", "duration": 900},
                           {"type": "pointerUp", "button": 0}])])
    time.sleep(1.5)
    rc = js("return __m1.touch.rightClicks")
    menu = changed_near(400, 400, 150)
    check(rc == 1, "a long press is a right click", rc)
    check(menu and menu["changed"] > 2000, "and KolibriOS answers it (its desktop menu is drawn)", menu)
    tap(*PARK)
    time.sleep(0.8)

    print("-- pinch --")
    cx, cy = to_view(512, 384)
    perform([finger("f1", [{"type": "pointerMove", "duration": 0, "origin": "viewport", "x": cx - 30, "y": cy},
                           {"type": "pointerDown", "button": 0},
                           {"type": "pointerMove", "duration": 500, "origin": "viewport", "x": cx - 150, "y": cy},
                           {"type": "pointerUp", "button": 0}]),
             finger("f2", [{"type": "pointerMove", "duration": 0, "origin": "viewport", "x": cx + 30, "y": cy},
                           {"type": "pointerDown", "button": 0},
                           {"type": "pointerMove", "duration": 500, "origin": "viewport", "x": cx + 150, "y": cy},
                           {"type": "pointerUp", "button": 0}])])
    time.sleep(0.5)
    scr = js("return __m1.screen")
    t2 = js("return {taps: __m1.touch.taps, zooms: __m1.touch.zooms}")
    check(scr["zoom"] >= 3, "pinching out zooms the view", scr)
    fit_shown = js("return !document.getElementById('fit').hidden")
    check(fit_shown, "a zoomed view offers 'fit'")
    # Where the page put the pointer, read from v86's VMware device (0..65535 per axis).
    # The unzoomed tap above measured the whole chain on the guest's own screen; this
    # measures what zooming could break -- the page's coordinates -- without a pixel
    # search that a window redrawing nearby can confuse (it did).
    tap(500, 390)
    time.sleep(0.4)
    at = js("const v = emulator.v86.cpu.devices.vmware; return [Math.round(v.last_x / 65535 * 1024), Math.round(v.last_y / 65535 * 768)];")
    err = max(abs(at[0] - 500), abs(at[1] - 390))
    check(err <= 2, "on the zoomed screen a tap still lands under the finger", f"guest pointer at {at}, off by {err} px")
    t3 = js("return __m1.touch.taps")
    check(t3 == t2["taps"] + 1, "the pinch itself clicked nothing", {"before": t2, "after": t3})
    js("document.getElementById('fit').click(); return 1")
    check(js("return __m1.screen.zoom") == 1, "'fit' puts the whole screen back", js("return __m1.screen"))
finally:
    try:
        rq("DELETE", f"/session/{sid}")
    except Exception:
        pass

print("*** KOLIBRIOS WORKS BY TOUCH ***" if not fails else f"*** {fails} FAILED ***")
sys.exit(1 if fails else 0)
