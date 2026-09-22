#!/usr/bin/env python3
"""monitor-vm-probe.py -- the System Monitor sees a running VM, and the VM keyboard button toggles.

    ./package-app.sh && (cd ../.. && make site OUT=<tree> DEPLOY_CONFIG=1 APPS_DIST=tools/run-env/app-dist)
    GRID=http://127.0.0.1:4480 python3 probes/monitor-vm-probe.py http://127.0.0.1:<port>/

Field report 2026-09-14, Firefox: ASCII Aquarium and CMatrix in two Apps windows froze
the tab, and the monitor listed both windows as "Apps", both "idle". The browser measures
nothing inside a sandboxed frame, so the emulator reports for itself (vm-sdk `reportStats`,
x-stats). Per machine this probe checks, in a real v86 inside the real player:

  - the emulator's main loop is wrapped and reports reach the host (inside the frame);
  - the monitor names the window by its app and shows the app's own figures --
    busy share, instructions per second, memory -- labelled as reported by the app;
  - THE KEYBOARD BUTTON IS A TOGGLE. The report: once the phone keyboard was up, the
    button never put it away. On a phone a tap can blur the field before `click` fires;
    the old handler looked at focus in `click`, saw "closed" and opened it again. The
    probe reproduces that order (blur, then click) and requires the keyboard to stay
    closed -- the old handler fails this.
"""
import json
import os
import sys
import time
import urllib.request

GRID = os.environ.get("GRID", "http://127.0.0.1:4444")
URL = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:8214/?log=trace"
MACHINES = [m for m in os.environ.get("MACHINES", "KolibriOS,Alpine Linux").split(",") if m]

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
    try:
        with urllib.request.urlopen(req, timeout=300) as r:
            return json.loads(r.read() or b"{}")
    except urllib.error.HTTPError as e:
        raise RuntimeError(f"{method} {path}: {e.code} {e.read()[:600]!r}") from None


def check(name, cond, detail=""):
    checks.append((name, bool(cond)))
    print(f"  {'PASS' if cond else 'FAIL'}  {name:<62} {str(detail)[:160]}")


caps = {"capabilities": {"alwaysMatch": {"browserName": "firefox", "moz:firefoxOptions": {"args": ["-headless"]}}}}
sid = rq("POST", "/session", caps)["value"]["sessionId"]
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


# Kolibri's phone keyboard is a hidden textarea; Alpine's is xterm's helper textarea.
KBD = {
    "KolibriOS": "document.getElementById('kbdin')",
    "Alpine Linux": "document.querySelector('.xterm-helper-textarea')",
}


def keyboard_toggle(machine):
    field = KBD[machine]
    js("window.__m1.forcePhoneKeyboard = true; return 1")
    is_open = f"return document.activeElement === {field}"
    js("document.getElementById('kbd').click(); return 1")
    opened = poll(is_open, bool, 5, every=0.2)
    check(f"{machine}: the keyboard button opens the keyboard", opened)
    # The phone's order: the tap blurs the field, THEN the click arrives.
    js(f"""const b = document.getElementById('kbd');
      b.dispatchEvent(new PointerEvent('pointerdown', {{ bubbles: true, cancelable: true, pointerType: 'touch' }}));
      {field}.blur();
      b.click(); return 1""")
    time.sleep(0.8)
    closed = not js(is_open)
    check(f"{machine}: a tap that blurs first still CLOSES it", closed,
          "the old handler re-opened here: it read focus at click time")
    time.sleep(0.6)
    js("document.getElementById('kbd').click(); return 1")
    again = poll(is_open, bool, 5, every=0.2)
    check(f"{machine}: and the next tap opens it again", again)
    js(f"{field}.blur(); return 1")


def monitor_row(machine):
    return js(ROOT + f"""
      let mon = null;
      for (const s of root.querySelectorAll('section.window')) if (s.querySelector('[data-field="monitor-window"]')) {{ mon = s; break; }}
      if (!mon) return null;
      const clean = t => t.replace(/[\\u2068\\u2069]/g, '');
      const row = Array.from(mon.querySelectorAll('[data-field="monitor-window-row"]'))
        .find(r => clean(r.querySelector('td').textContent).includes({json.dumps(machine)}));
      const mem = mon.querySelector('[data-field="monitor-memory"]');
      const cpu = mon.querySelector('[data-field="monitor-cpu"]');
      return {{ load: row && row.getAttribute('data-app-load'), text: row && clean(row.textContent),
               memory: mem && clean(mem.textContent), tab: cpu && clean(cpu.textContent).slice(0, 220) }};""")


rq("POST", f"/session/{sid}/url", {"url": URL})
LOG = "return (window.__entity_browser_log || []).map(e => (e.args || []).join(' '));"
poll(LOG, lambda v: any("boot surface down" in l for l in (v or [])), 60)
check("opened the Apps window", spawn("Apps") == "clicked")
js(ROOT + r"""
  for (const s of root.querySelectorAll('section.window')) {
    if (getComputedStyle(s).zIndex !== '9999') continue;
    const b = [...s.querySelectorAll('header button')].find(b => b.textContent.trim() === '×');
    if (b) b.click();
  }
  return 1;""")
check("opened the System Monitor", spawn("System Monitor") == "clicked")

for machine in MACHINES:
    parent()
    js(APPS + "const b = sec && Array.from(sec.querySelectorAll('button')).find(b => b.textContent.trim().startsWith('←')); if (b) b.click(); return 1")
    # Wait for the previous machine's frame to be GONE before looking for the next
    # one: the back press rebuilds on a later frame, and a launch check that runs
    # first finds the old iframe, calls it launched, and enters a frame that is
    # about to be torn down (measured: the second machine then "reports" the
    # first machine's instructions, then the frame goes stale).
    poll(APPS + "return !!(sec && !sec.querySelector('iframe[sandbox]'));", bool, 30)
    launched = poll(APPS + f"""
      if (!sec) return 'no-window';
      if (sec.querySelector('iframe[sandbox]')) return 'clicked';
      const b = Array.from(sec.querySelectorAll('button')).find(b => !b.hasAttribute('data-chip') && b.textContent.includes({json.dumps(machine)}));
      if (!b) return 'no-card';
      b.click(); return 'clicked';""", lambda v: v == "clicked", 90)
    check(f"{machine}: launched", launched == "clicked", launched)
    poll(APPS + "return !!(sec && sec.querySelector('iframe[sandbox]'));", bool, 60)
    into_frame()
    stats = poll("return window.__m1 && window.__m1.stats && window.__m1.stats.reports >= 3 ? window.__m1.stats : null", bool, 120, every=1)
    check(f"{machine}: the emulator reports its own load to the host", stats and stats.get("wrapped", 0) >= 1,
          json.dumps(stats)[:160])
    # A machine can reload its own frame while it boots (a snapshot that does not
    # fit falls back to a cold boot): re-enter the live frame before touching it.
    time.sleep(3)
    into_frame()
    poll("return !!(window.__m1 && window.__m1.stats && document.getElementById('kbd'))", bool, 60, every=1)
    keyboard_toggle(machine)
    parent()
    row = None
    t = time.time()
    while time.time() - t < 30:
        row = monitor_row(machine)
        if row and row.get("load") in ("busy", "active", "idle") and "instructions" in (row.get("text") or ""):
            break
        time.sleep(1)
    check(f"{machine}: the monitor names the window by its app", row and row.get("text"), (row or {}).get("text"))
    check(f"{machine}: and shows the app's own load, not 'not reporting'",
          row and row.get("load") in ("busy", "active", "idle") and "instructions" in (row.get("text") or ""),
          f"load={(row or {}).get('load')}")
    check(f"{machine}: its memory is in the Memory pane", row and f"{machine}:" in (row.get("memory") or ""),
          (row or {}).get("memory", "")[-120:])
    print(f"        tab: {(row or {}).get('tab')}")

rq("DELETE", f"/session/{sid}")
failed = [n for n, ok in checks if not ok]
print(f"\n{len(checks) - len(failed)} / {len(checks)} passed" + (f"; FAILED: {failed}" if failed else ""))
sys.exit(1 if failed else 0)
