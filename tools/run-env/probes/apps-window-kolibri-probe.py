#!/usr/bin/env python3
"""apps-window-kolibri-probe.py -- KolibriOS as an app in the Apps window, two visits.

    ./package-app.sh && (cd ../.. && make site OUT=<tree> DEPLOY_CONFIG=1 APPS_DIST=tools/run-env/app-dist)
    GRID=http://127.0.0.1:4480 python3 probes/apps-window-kolibri-probe.py http://127.0.0.1:<port>/

The standalone probe (kolibri-probe.py) proves the guest reads and writes the page's
floppy. This one proves what only a HOST can show:
  visit 1  real input: a click gives the frame the keyboard, the guest pointer sits
           under the host pointer, typed keys run a command; every machine file
           comes through the player (no guest file by URL); a
           file the HOST sends lands on the floppy; a file the page takes out
           reaches the host (x-file-result ok); the floppy is saved to the host.
  visit 2  a reload -- the frame killed without warning -- and the floppy that comes
           back holds the host's file. Restored from the host, not rebuilt fresh:
           `lastSwap.why` says which, and a fresh floppy cannot contain that name.
"""
import json
import os
import secrets
import sys
import time
import urllib.request
from importlib import import_module
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
KI = import_module("kolibri-input")

GRID = os.environ.get("GRID", "http://127.0.0.1:4444")
URL = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:8214/?log=trace"
NAME = f"host-note-{secrets.token_hex(3)}.txt"
TEXT = "sent by the host " + secrets.token_hex(6)

APPS = r"""
  const layer = document.getElementById('dom-layer');
  const root = layer && (layer.shadowRoot || layer);
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
    print(f"  {'PASS' if cond else 'FAIL'}  {name:<58} {str(detail)[:150]}")


caps = {"capabilities": {"alwaysMatch": {"browserName": "firefox", "moz:firefoxOptions": {"args": ["-headless"]}}}}
sid = rq("POST", "/session", caps)["value"]["sessionId"]
js = lambda s: rq("POST", f"/session/{sid}/execute/sync", {"script": s, "args": []})["value"]
rq("POST", f"/session/{sid}/timeouts", {"script": 90000})
ajs = lambda s: rq("POST", f"/session/{sid}/execute/async", {"script": "const cb = arguments[arguments.length - 1];" + s, "args": []})["value"]


def poll(script, ok, budget, every=0.5):
    t, last = time.time(), None
    while time.time() - t < budget:
        last = js(script)
        if ok(last):
            return last
        time.sleep(every)
    return last


def into_frame():
    rq("POST", f"/session/{sid}/frame/parent", {})
    f = js(APPS + "return sec && sec.querySelector('iframe[sandbox]');")
    rq("POST", f"/session/{sid}/frame", {"id": f})


def visit(n):
    rq("POST", f"/session/{sid}/url", {"url": URL})
    LOG = "return (window.__entity_browser_log || []).map(e => (e.args || []).join(' '));"
    poll(LOG, lambda v: any("boot surface down" in l for l in (v or [])), 60)
    opened = poll(r"""
      const layer = document.getElementById('dom-layer'); const root = layer && (layer.shadowRoot || layer);
      if (!root) return 'no-dom-layer';
      for (const b of root.querySelectorAll('button.spawn-btn')) if (b.textContent.trim() === '+ Apps') { b.click(); return 'clicked'; }
      return 'no-btn';""", lambda v: v == "clicked", 60)
    check("opened the Apps window", opened == "clicked", opened)
    # A deployment that boots a MAXIMIZED window (surface=window) keeps it on top
    # (z-index 9999) of every window opened after it -- including this one. Script
    # clicks do not care; real input lands on whatever is on top. Close it the way a
    # person does. (Recorded as a window-manager finding, not worked around silently.)
    covered = js(r"""
      const layer = document.getElementById('dom-layer'); const root = layer && (layer.shadowRoot || layer);
      const closed = [];
      for (const s of root.querySelectorAll('section.window')) {
        if (getComputedStyle(s).zIndex !== '9999') continue;
        const b = [...s.querySelectorAll('header button')].find(b => b.textContent.trim() === '×');
        if (b) { closed.push(s.querySelector('header h3').textContent.trim()); b.click(); }
      }
      return closed;""")
    if covered:
        print(f"        closed a maximized window covering the Apps window: {covered}")
    launched = poll(APPS + r"""
      if (!sec) return 'no-window';
      if (sec.querySelector('iframe[sandbox]')) return 'clicked';
      const b = Array.from(sec.querySelectorAll('button')).find(b => !b.hasAttribute('data-chip') && b.textContent.includes('KolibriOS'));
      if (!b) return 'no-card:' + sec.textContent.slice(0, 160);
      b.click(); return 'clicked';""", lambda v: v == "clicked", 90)
    check("launched the KolibriOS card", launched == "clicked", launched)
    t_launch = time.time()
    frame = poll(APPS + "return sec && sec.querySelector('iframe[sandbox]');", bool, 60)
    sandbox = js(APPS + "const f = sec && sec.querySelector('iframe[sandbox]'); return f && f.getAttribute('sandbox');")
    check("the frame is the opaque third-party tier", sandbox and "allow-same-origin" not in sandbox, sandbox)
    rq("POST", f"/session/{sid}/frame", {"id": frame})
    ready = poll("return !!(window.__m1 && window.__m1.floppy && window.__m1.floppy.lastSwap)", bool, 90)
    t_ready = time.time()
    m = js("""return {mode: __m1.assets.mode, assets: __m1.assets, snap: __m1.snapshot, floppy: __m1.floppy, shim: !!__m1.clockShim,
                      urls: performance.getEntriesByType('resource').filter(r => /kolibri\\.img|v86\\.wasm|bios\\.bin/.test(r.name)).length,
                      status: document.getElementById('status').textContent}""")
    check("the desktop came up in the player", ready, f"{t_ready - t_launch:.1f} s after the click · {m['status']}")
    # The only refusal allowed is an unpublished snapshot: `not-found` for snapshot.json.
    refusals_ok = m["assets"]["refused"] == 0 or (m["assets"]["refused"] == 1 and "snapshot.json: not-found" in (m["assets"]["lastRefusal"] or ""))
    check("every machine file came from the host", m["mode"] == "host" and m["urls"] == 0 and refusals_ok,
          f"mode={m['mode']} by-url={m['urls']} requests={m['assets']['requests']} refused={m['assets']['refused']} {m['assets']['lastRefusal'] or ''}")
    if os.environ.get("EXPECT_SNAPSHOT") == "1":
        check("the machine resumed from its snapshot", m["snap"].get("used") is True, json.dumps(m["snap"])[:160])
    check("the clock shim is on inside the player frame", m["shim"])

    if n == 1:
        # REAL input inside the frame (kolibri-input.py). The field report: keys went
        # to the host page because clicking the screen never gave the frame focus, and
        # the guest pointer drifted from the host pointer. Both only show in a frame,
        # through browser input -- the API calls the rest of this probe uses bypass them.
        inp = KI.Input(rq, sid, js)
        before = js("return document.hasFocus()")
        mode = poll("return __m1.pointer && __m1.pointer.mode", lambda v: v == "absolute", 30)
        check("the guest pointer is absolute inside the player", mode == "absolute", mode)
        if not m["snap"].get("used"):
            time.sleep(4)          # a cold desktop draws its icons a few seconds after it settles
        inp.click(512, 300)
        focus = js("return {has: document.hasFocus(), shown: document.getElementById('kbd').classList.contains('on')}")
        # THIS is the keyboard gate. Falsified 2026-09-14: without the page's pointerdown
        # focus handler it reads has=False. The typed-keys check below does NOT see that
        # bug on its own -- WebDriver's key dispatch focused the frame itself on a retry
        # -- so it proves keys reach the guest, not that a person's click lets them.
        check("clicking the screen gives the frame the keyboard", focus["has"] and focus["shown"],
              f"focused before the click: {before}; after: {focus}")
        err, where = inp.pointer_error((950, 80), (300, 250), path=[(100, 100), (900, 600), (50, 700), (512, 384)])
        check("after a long path the guest pointer is under the host's", err is not None and err <= 3, f"off by {err} px, {where}")
        inp.click(853, 163, times=2)       # SHELL
        time.sleep(3)
        typed = None
        for attempt in (1, 2):
            inp.type("cp /fd/1/README.TXT /fd/1/typed.txt\n")
            # KolibriOS writes a name that fits 8.3 as a bare short name: typed.txt lands as TYPED.TXT.
            typed = poll("return (listFloppy(), __m1.floppy.names.some(n => n.toLowerCase() === 'typed.txt'))", bool, 25, every=1)
            if typed:
                break
            print(f"        attempt {attempt}: no copy yet (a dropped key?)")
        check("keys typed in the browser run a command in KolibriOS", bool(typed), js("return __m1.floppy.names"))
        # SOUND, measured where the samples are made: v86 hands the DAC its buffers as
        # transferables, so a listener after the audio worklet can see them emptied.
        js("""window.__au = {}; const sb = emulator.v86.cpu.devices.sb16; const orig = sb.dma_to_dac;
          sb.dma_to_dac = function (n) { const v = this.dsp_16bit ? (this.dsp_signed ? this.dma_buffer_int16 : this.dma_buffer_uint16) : (this.dsp_signed ? this.dma_buffer_int8 : this.dma_buffer_uint8);
            const mid = this.dsp_signed ? 0 : (this.dsp_16bit ? 32768 : 128); let m = 0; for (let i = 0; i < n; i++) m = Math.max(m, Math.abs(v[i] - mid));
            __au.peak = Math.max(__au.peak || 0, m); __au.samples = (__au.samples || 0) + n; return orig.call(this, n); }; return 1""")
        inp.type("/sys/media/ac97snd /sys/sine.mp3\n")
        au = poll("return window.__au.samples > 96000 ? window.__au : null", bool, 40, every=1)
        check("KolibriOS plays sound: SINE.MP3 reaches the Sound Blaster as real samples", bool(au) and au["peak"] > 1000, au or js("return window.__au"))
        check("and the page's audio is running after a real click", js("return __m1.audio && __m1.audio.state") == "running", js("return __m1.audio"))
        check("a fresh profile gets a fresh floppy", m["floppy"]["lastSwap"]["why"] == "fresh" and "README.TXT" in (m["floppy"]["names"] or []),
              json.dumps(m["floppy"]))
        # Posted as the host does after its "Send a file" picker.
        rq("POST", f"/session/{sid}/frame/parent", {})
        js(APPS + f"""
          const f = sec.querySelector('iframe[sandbox]');
          const data = new TextEncoder().encode({json.dumps(TEXT)}).buffer;
          f.contentWindow.postMessage({{source: 'entity-host', type: 'x-file', name: {json.dumps(NAME)}, media_type: 'text/plain', data}}, '*', [data]);
          return 1;""")
        into_frame()
        got = poll(f"return __m1.floppy.received.includes({json.dumps(NAME)}) && __m1.floppy.lastSave && __m1.floppy.lastSave.reason === 'put in' ? __m1.floppy : null", bool, 30)
        check("a file the host sends lands on the floppy", bool(got), json.dumps((got or {}).get("names")))
        check("and the floppy is saved to the host", bool(got) and got["lastSave"].get("saved") is True, json.dumps((got or {}).get("lastSave")))
        same = js(f"""const f = EntityFat.readFloppy(emulator.get_disk_fda()).find(x => x.path === {json.dumps(NAME)});
                      return !!f && new TextDecoder().decode(f.data) === {json.dumps(TEXT)};""")
        check("its bytes on the disk are the host's bytes", same)
        js("takeOut(EntityFat.readFloppy(emulator.get_disk_fda()).filter(f => f.path === 'README.TXT')); return 1")
        res = poll("const r = (__m1.files || {}).results || []; return r.length ? r : null;", bool, 30)
        check("a file taken out reaches the host", bool(res) and res[0]["ok"] and res[0]["name"] == "README.TXT", json.dumps(res))
        listing = ajs("EntityVm.work('x-work-list', {}).then(r => cb((r.files || []).map(f => f.path)))")
        check("the floppy is kept as FILES in the workspace, visible in the tree", f"floppy/{NAME}" in (listing or []) and "transfer.img" not in (listing or []), listing)
        # Put this profile back into the shape builds before 2026-09-14 left it in: the
        # whole floppy as one transfer.img. Visit 2 must migrate it, not lose it.
        legacy = ajs("""const img = liveFloppy().slice().buffer; __floppyKeep.stopSaving();
                       EntityVm.work('x-work-save', {put: [{path: 'transfer.img', mode: 420, mtime: Math.floor(Date.now()/1000), data: img}],
                                              remove: __floppyKeep.paths()}, 60000).then(r => EntityVm.work('x-work-list', {})).then(r => cb((r.files || []).map(f => f.path)))""")
        check("(set up) this profile now holds only the old one-image shape", legacy == ["transfer.img"], legacy)
        # The host's store is write-behind; give the save a moment before killing the page.
        time.sleep(3)
    else:
        f = m["floppy"]
        check("the floppy came back from the host, not fresh", f["lastSwap"]["why"] == "restored", json.dumps(f.get("lastSwap")))
        check("and it holds the file the host sent last visit", NAME in (f.get("names") or []), json.dumps(f.get("names")))
        check("an old transfer.img profile is read", js("return __m1.floppy.restoredFrom") == "image", js("return __m1.floppy.restoredFrom"))
        ajs("saveFloppy('look').then(cb)")
        listing = ajs("EntityVm.work('x-work-list', {}).then(r => cb((r.files || []).map(f => f.path)))")
        check("and migrated: its files are kept as files, the image is gone", f"floppy/{NAME}" in (listing or []) and "transfer.img" not in (listing or []), listing)
        about = js("document.getElementById('info').click(); const p = document.getElementById('aboutpanel'); return !p.hidden && p.textContent;")
        check("ⓘ names KolibriOS's licence and source commit", bool(about) and "GPL-2.0" in about and "b0055ba4721a" in about, (about or "")[:80])
    rq("POST", f"/session/{sid}/frame/parent", {})


try:
    print(f"\nAPPS WINDOW -- KolibriOS -- {URL}\n")
    for n in (1, 2):
        print(f"-- visit {n} --")
        visit(n)
finally:
    try:
        rq("DELETE", f"/session/{sid}")
    except Exception:
        pass
failed = [n for n, ok in checks if not ok]
print("\n*** KOLIBRIOS RUNS AS AN APP, AND ITS FLOPPY IS KEPT ***" if not failed else f"\n*** {len(failed)} FAILED: {', '.join(failed)} ***")
sys.exit(1 if failed else 0)
