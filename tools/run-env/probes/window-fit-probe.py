#!/usr/bin/env python3
"""window-fit-probe.py -- KolibriOS fits its window, in a real v86 inside the real player.

    GRID=http://127.0.0.1:4480 WIDTH=1600 HEIGHT=1000 python3 probes/window-fit-probe.py http://127.0.0.1:<port>/

Field report 2026-09-14 (BACKLOG B-5): in a normal window KolibriOS drew small with black
bars either side, and full screen was the only mode that fit. With the fit (src/window_size.rs)
the page reports its screen (vm-sdk reportView, x-view) and the host sizes the WINDOW so the
screen shows whole at the frame's width, capped at the visible window area. Checks:

  - the machine reports a 4:3 screen once the desktop is up;
  - the Apps window is sized (data-size is a number), and inside the frame the screen fills
    the stage in one direction: no bars left and right, or the window is as tall as the area;
  - the System Monitor row for the Apps window says what it downloaded (B-6);
  - a height set on the grip (keyboard: ArrowUp) is kept, and wins over the fit.
"""
import json
import os
import sys
import time
import urllib.request

GRID = os.environ.get("GRID", "http://127.0.0.1:4444")
URL = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:8213/"
WIDTH, HEIGHT = int(os.environ.get("WIDTH", "1600")), int(os.environ.get("HEIGHT", "1000"))
ROOT = r"""
  const layer = document.getElementById('dom-layer');
  const root = layer && (layer.shadowRoot || layer);
"""
APPS = ROOT + r"""
  let sec = null;
  if (root) for (const s of root.querySelectorAll('section.window')) {
    const h3 = s.querySelector('header h3');
    if (h3 && h3.textContent.trim() === 'Apps') { sec = s; break; }
  }
"""
checks = []


def rq(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(GRID + path, data=data, method=method, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=300) as r:
        return json.loads(r.read() or b"{}")


def check(name, cond, detail=""):
    checks.append((name, bool(cond)))
    print(f"  {'PASS' if cond else 'FAIL'}  {name:<66} {str(detail)[:200]}")


caps = {"capabilities": {"alwaysMatch": {"browserName": "firefox", "moz:firefoxOptions": {"args": ["-headless"]}}}}
sid = rq("POST", "/session", caps)["value"]["sessionId"]
rq("POST", f"/session/{sid}/window/rect", {"width": WIDTH, "height": HEIGHT})
js = lambda s: rq("POST", f"/session/{sid}/execute/sync", {"script": s, "args": []})["value"]


def poll(script, ok, budget, every=0.5):
    t, last = time.time(), None
    while time.time() - t < budget:
        last = js(script)
        if ok(last):
            return last
        time.sleep(every)
    return last


def parent():
    rq("POST", f"/session/{sid}/frame/parent", {})


def into_frame():
    parent()
    f = js(APPS + "return sec && sec.querySelector('iframe[sandbox]');")
    rq("POST", f"/session/{sid}/frame", {"id": f})


def spawn(label):
    return poll(ROOT + f"""
      if (!root) return 'no-dom-layer';
      for (const b of root.querySelectorAll('button.spawn-btn')) if (b.textContent.trim() === '+ {label}') {{ b.click(); return 'clicked'; }}
      return 'no-btn';""", lambda v: v == "clicked", 60)


def launch():
    parent()
    return poll(APPS + """
      if (!sec) return 'no-window';
      if (sec.querySelector('iframe[sandbox]')) return 'clicked';
      const b = Array.from(sec.querySelectorAll('button')).find(b => !b.hasAttribute('data-chip') && b.textContent.includes('KolibriOS'));
      if (!b) return 'no-card';
      b.click(); return 'clicked';""", lambda v: v == "clicked", 90)


SECTION = APPS + r"""
  if (!sec) return null;
  const f = sec.querySelector('iframe[sandbox]');
  return { size: sec.getAttribute('data-size'), height: sec.offsetHeight, area: sec.parentElement.clientHeight,
           frame: f ? [f.clientWidth, f.clientHeight] : null, fitPx: f && f.getAttribute('data-fit-px'),
           fitOffered: !!sec.querySelector('[data-field="app-fit-screen"]:not([hidden])') };
"""

rq("POST", f"/session/{sid}/url", {"url": URL})
LOG = "return (window.__entity_browser_log || []).map(e => (e.args || []).join(' '));"
poll(LOG, lambda v: any("boot surface down" in l for l in (v or [])), 60)
js(ROOT + r"""
  for (const s of root.querySelectorAll('section.window')) {
    if (getComputedStyle(s).zIndex !== '9999') continue;
    const b = [...s.querySelectorAll('header button')].find(b => b.textContent.trim() === '×');
    if (b) b.click();
  }
  return 1;""")
check("opened the System Monitor", spawn("System Monitor") == "clicked")
check("opened the Apps window", spawn("Apps") == "clicked")
check("launched KolibriOS", launch() == "clicked")
poll(APPS + "return !!(sec && sec.querySelector('iframe[sandbox]'));", bool, 60)
time.sleep(3)
into_frame()
view = poll("return window.__m1 && window.__m1.view || null", bool, 150, every=1)
check("the machine reports its screen (x-view)", view and abs(view["screen_h"] / view["screen_w"] - 0.75) < 0.01, json.dumps(view))
parent()
sized = poll(SECTION, lambda v: v and v.get("size") not in (None, "natural") and v.get("fitPx"), 20)
check("the Apps window is sized to the fit", sized and sized.get("size") not in (None, "natural"), json.dumps(sized))
check("Fit is offered", sized and sized.get("fitOffered"))
time.sleep(2)
sized = js(SECTION)
into_frame()
inside = poll("const st = document.getElementById('stage'); return window.__m1.screen && st ? { scale: window.__m1.screen.scale, W: st.clientWidth, H: st.clientHeight } : null", bool, 20)
parent()
if inside and view:
    sw, sh = view["screen_w"] * inside["scale"], view["screen_h"] * inside["scale"]
    no_side_bars = abs(inside["W"] - sw) <= 4
    capped = sized and abs(sized["height"] - (sized["area"] - 8)) <= 4
    check("the screen fills the frame's width, or the window is as tall as the area",
          no_side_bars or capped,
          f"stage {inside['W']}x{inside['H']}, screen drawn {sw:.0f}x{sh:.0f} (scale {inside['scale']}), window {sized}")
    print(f"        scale {inside['scale']}: screen {sw:.0f}x{sh:.0f} in a {inside['W']}x{inside['H']} stage; window {sized['height']} of area {sized['area']}")
else:
    check("the screen fills the frame", False, f"inside={inside} view={view}")

row = poll(ROOT + r"""
  let mon = null;
  for (const s of root.querySelectorAll('section.window')) if (s.querySelector('[data-field="monitor-window"]')) { mon = s; break; }
  const r = mon && Array.from(mon.querySelectorAll('[data-field="monitor-window-row"]')).find(r => r.getAttribute('data-window-type') === 'Apps');
  const f = r && r.querySelector('[data-field="monitor-window-fetched"]');
  return f ? Number(f.getAttribute('data-value')) : null;""", lambda v: v, 20)
check("the monitor says what the Apps window downloaded (B-6)", row, row)

before = js(SECTION)["height"]
js(APPS + """
  const g = sec.querySelector('[data-field="window-size-grip"]');
  g.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp', bubbles: true }));
  return 1;""")
after = poll(SECTION, lambda v: v and abs(v["height"] - (before - 40)) <= 2, 10)
check("ArrowUp on the grip makes the window 40 px shorter, over the fit", after and abs(after["height"] - (before - 40)) <= 2,
      f"{before} -> {after and after['height']}")
time.sleep(2)
rq("POST", f"/session/{sid}/url", {"url": URL})
poll(LOG, lambda v: any("boot surface down" in l for l in (v or [])), 60)
spawn("Apps")
launch()
kept = poll(SECTION, lambda v: v and v.get("frame") and abs(v["height"] - (before - 40)) <= 2, 30)
check("after a reload, KolibriOS opens at the height it was given", kept and abs(kept["height"] - (before - 40)) <= 2, json.dumps(kept))
js(APPS + "const b = sec.querySelector('[data-field=\"app-fit-screen\"]'); if (b) b.click(); return 1")

rq("DELETE", f"/session/{sid}")
failed = [n for n, ok in checks if not ok]
print(f"\n{len(checks) - len(failed)} / {len(checks)} passed" + (f"; FAILED: {failed}" if failed else ""))
sys.exit(1 if failed else 0)
