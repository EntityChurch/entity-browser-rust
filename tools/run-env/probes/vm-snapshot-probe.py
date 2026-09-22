#!/usr/bin/env python3
"""S1's measurement: how big is a machine snapshot taken at the prompt, and how
fast does a restore reach a shell that answers?

Launches Apps -> Alpine Linux exactly as `apps-window-vm-probe.py` does, waits
for the prompt and the guest agent, then inside the frame:

  1. `emulator.save_state()` -- raw size, time, and how many 4 KiB pages are
     all zero (the 256 MiB of guest RAM is expected to be mostly zeros);
  2. gzip via `CompressionStream` -- the compression every browser has, so the
     number a page could produce or undo without a library;
  3. copies the gzip out (base64, in chunks) to OUT so the host can compare
     zstd and xz, which the pinned libv86 can decompress (zstd) or not;
  4. `stop()` -> `restore_state()` -> `run()` IN PLACE, then times a command
     that reads a file the guest had not read before the snapshot, so a restore
     that lost the 9p filesystem cannot answer.

Bound, stated: step 4 restores into the emulator that took the snapshot. A
fresh launch would also pay for fetching the snapshot and building a new
emulator; the page does not support `initial_state` from the host yet.

    GRID=http://127.0.0.1:4480 OUT=/tmp/snap python3 vm-snapshot-probe.py http://127.0.0.1:8213/?log=trace
"""
import base64
import json
import os
import pathlib
import sys
import time
import urllib.request

GRID = os.environ.get("GRID", "http://127.0.0.1:4444")
URL = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:8211/?log=trace"
OUT = pathlib.Path(os.environ.get("OUT", "snapshot-out"))
IDLE_S = float(os.environ.get("IDLE_S", "5"))

APPS = r"""
  const layer = document.getElementById('dom-layer');
  const root = layer && (layer.shadowRoot || layer);
  let sec = null;
  if (root) for (const s of root.querySelectorAll('section.window')) {
    const h3 = s.querySelector('header h3');
    if (h3 && h3.textContent.trim() === 'Apps') { sec = s; break; }
  }
"""
STRIP = "replace(/\\x1b\\[[0-9;?]*[a-zA-Z]/g, '')"


def rq(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(GRID + path, data=data, method=method,
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=300) as r:
        return json.loads(r.read() or b"{}")


def js(sid, script):
    return rq("POST", f"/session/{sid}/execute/sync", {"script": script, "args": []})["value"]


def poll(sid, script, ok, budget, every=0.5):
    deadline = time.time() + budget
    last = None
    while time.time() < deadline:
        last = js(sid, script)
        if ok(last):
            return last
        time.sleep(every)
    return last


def die(msg):
    print(f"\n*** {msg} ***")
    sys.exit(1)


caps = {"capabilities": {"alwaysMatch": {"browserName": "firefox",
                                         "moz:firefoxOptions": {"args": ["-headless"]}}}}
print(f"\nVM SNAPSHOT MEASUREMENT -- {URL}  (grid {GRID})\n")
sid = rq("POST", "/session", caps)["value"]["sessionId"]
try:
    rq("POST", f"/session/{sid}/url", {"url": URL})
    LOG = "return (window.__entity_browser_log || []).map(e => (e.args || []).join(' '));"
    poll(sid, LOG, lambda v: any("boot surface down" in l for l in (v or [])), 60)
    opened = poll(sid, r"""
      const layer = document.getElementById('dom-layer');
      const root = layer && (layer.shadowRoot || layer);
      if (!root) return 'no-dom-layer';
      for (const b of root.querySelectorAll('button.spawn-btn'))
        if (b.textContent.trim() === '+ Apps') { b.click(); return 'clicked'; }
      return 'no-btn';""", lambda v: v == "clicked", 60)
    if opened != "clicked":
        die(f"could not open the Apps window: {opened}")
    launched = poll(sid, APPS + r"""
      if (!sec) return 'no-window';
      if (sec.querySelector('iframe[sandbox]')) return 'clicked';
      const b = Array.from(sec.querySelectorAll('button'))
        .find(b => !b.hasAttribute('data-chip') && b.textContent.includes('Alpine Linux'));
      if (!b) return 'no-card';
      b.click(); return 'clicked';""", lambda v: v == "clicked", 90)
    if launched != "clicked":
        die(f"could not launch Alpine: {launched}")
    frame = poll(sid, APPS + "return sec && sec.querySelector('iframe[sandbox]');", lambda v: bool(v), 60)
    rq("POST", f"/session/{sid}/frame", {"id": frame})
    state = poll(sid, "return window.__m1 ? {prompt: window.__m1.promptMs, error: window.__m1.error} : null;",
                 lambda v: bool(v) and (v.get("prompt") or v.get("error")), 240, every=1.0)
    if not (state and state.get("prompt")):
        die(f"the guest never reached a prompt: {state}")
    boot_ms = state["prompt"]
    agent = poll(sid, "return window.__m1.agent || null;", lambda v: bool(v), 30)
    print(f"  boot -> prompt: {boot_ms} ms in-frame; agent: {json.dumps(agent)}")
    time.sleep(IDLE_S)  # let the boot's tail (agent, restore, first save) settle

    def guest(cmd, pattern, budget=60):
        tag = "T" + os.urandom(4).hex()
        js(sid, "window.__gFrom = (window.__m1.serial || '').length; window.emulator.serial_send_bytes(0, "
                f"new TextEncoder().encode({json.dumps(cmd + '; echo ' + tag + '-$((1+1))DONE')} + '\\n')); return 1;")
        out = poll(sid, f"const s = (window.__m1.serial || '').slice(window.__gFrom).{STRIP};"
                        f"return s.includes({json.dumps(tag + '-2DONE')}) ? s : null;", lambda v: bool(v), budget) or ""
        body = out[out.find(tag + "-$((1+1))DONE"):]
        import re
        m = re.search(pattern, body)
        return m.group(0) if m else None

    mem = guest("echo MEM-$(awk '/MemFree|^Cached|Buffers|AnonPages|Slab/ {printf \"%s%s,\", $1, $2}' /proc/meminfo)-", r"MEM-[^\r\n]*-")
    print(f"  guest memory (kB): {mem}")
    # What zeroing freed pages costs: 300 fork+exec of a tiny binary, timed by
    # the guest itself. Run on both kernel command lines to compare.
    bench = guest("s=$(date +%s%N); for i in $(seq 300); do /bin/true; done; e=$(date +%s%N); "
                  "echo BENCH-$(( (e - s) / 1000000 ))ms-", r"BENCH-\d+ms-", 180)
    print(f"  300x fork+exec in the guest: {bench}")
    if os.environ.get("ZERO_FILL"):
        # Zero the guest's free memory once, at snapshot time, instead of paying
        # init_on_free on every page the kernel ever frees: fill a tmpfs file
        # with zeros up to what is free, then delete it -- the freed pages keep
        # the zeros. `mount` first in case /dev/shm is not there.
        margin = int(os.environ.get("ZF_MARGIN_M", "12"))
        filled = guest("mkdir -p /run/zf && mount -t tmpfs -o size=250m tmpfs /run/zf 2>/dev/null; "
                       f"n=$(( $(awk '/MemFree/ {{print $2}}' /proc/meminfo) / 1024 - {margin} )); "
                       "dd if=/dev/zero of=/run/zf/z bs=1M count=$n 2>/dev/null; rm -f /run/zf/z; umount /run/zf; "
                       "echo ZF-${n}M-$(awk '/MemFree/ {print $2}' /proc/meminfo)kB-", r"ZF-[^\r\n]*-", 240)
        print(f"  zero-filled free memory: {filled}")
    if os.environ.get("DROP_CACHES"):
        dropped = guest("sync; echo 3 > /proc/sys/vm/drop_caches; echo DROP-$(awk '/MemFree|^Cached/ {printf \"%s%s,\", $1, $2}' /proc/meminfo)-", r"DROP-[^\r\n]*-")
        print(f"  after drop_caches (kB): {dropped}")
        time.sleep(2)

    # ── 1 + 2: save, count zero pages, gzip ────────────────────────────────
    js(sid, r"""
      window.__snap = { phase: 'saving' };
      (async () => {
        try {
          const e = window.emulator;
          let t = performance.now();
          const st = await e.save_state();
          const saveMs = performance.now() - t;
          const u8 = new Uint8Array(st);
          const pages = Math.floor(u8.length / 4096);
          let zero = 0;
          for (let p = 0; p < pages; p++) {
            const o = p * 4096; let z = true;
            for (let i = 0; i < 4096; i++) { if (u8[o + i]) { z = false; break; } }
            if (z) zero++;
          }
          t = performance.now();
          const gz = await new Response(new Blob([st]).stream()
            .pipeThrough(new CompressionStream('gzip'))).arrayBuffer();
          const gzMs = performance.now() - t;
          t = performance.now();
          const back = await new Response(new Blob([gz]).stream()
            .pipeThrough(new DecompressionStream('gzip'))).arrayBuffer();
          const gunzipMs = performance.now() - t;
          window.__snapBuf = st;
          window.__snapGz = new Uint8Array(gz);
          window.__snap = { phase: 'saved', raw: st.byteLength, gzip: gz.byteLength,
                            roundTrip: back.byteLength === st.byteLength,
                            saveMs, gzMs, gunzipMs, pages, zeroPages: zero };
        } catch (err) { window.__snap = { phase: 'error', error: String(err && err.stack || err) }; }
      })();
      return 1;""")
    snap = poll(sid, "return window.__snap;", lambda v: bool(v) and v.get("phase") != "saving", 180, every=1.0)
    if not snap or snap.get("phase") != "saved":
        die(f"save_state failed: {snap}")
    mib = lambda n: f"{n / 1048576:.1f} MiB"
    print(f"  save_state: {mib(snap['raw'])} in {snap['saveMs']:.0f} ms; "
          f"{snap['zeroPages']}/{snap['pages']} 4 KiB pages all zero "
          f"({mib((snap['pages'] - snap['zeroPages']) * 4096)} non-zero)")
    print(f"  gzip: {mib(snap['gzip'])} in {snap['gzMs']:.0f} ms; gunzip {snap['gunzipMs']:.0f} ms; "
          f"round trip intact: {snap['roundTrip']}")

    # ── 3: copy the gzip out ───────────────────────────────────────────────
    OUT.mkdir(parents=True, exist_ok=True)
    chunk = 6 * 1024 * 1024
    with open(OUT / "state.bin.gz", "wb") as f:
        for off in range(0, snap["gzip"], chunk):
            b64 = js(sid, f"""
              const s = window.__snapGz.subarray({off}, {off + chunk});
              let bin = ''; for (let i = 0; i < s.length; i += 32768)
                bin += String.fromCharCode.apply(null, s.subarray(i, i + 32768));
              return btoa(bin);""")
            f.write(base64.b64decode(b64))
    print(f"  copied out: {OUT / 'state.bin.gz'} ({(OUT / 'state.bin.gz').stat().st_size} bytes)")

    # ── 4: restore in place, then a command reading a file not read before ─
    js(sid, r"""
      window.__restore = { phase: 'restoring' };
      (async () => {
        try {
          const e = window.emulator;
          const from = (window.__m1.serial || '').length;
          e.stop();
          const t = performance.now();
          await e.restore_state(window.__snapBuf);
          const restoreMs = performance.now() - t;
          e.run();
          window.__restore = { phase: 'restored', restoreMs, from, at: performance.now() };
        } catch (err) { window.__restore = { phase: 'error', error: String(err && err.stack || err) }; }
      })();
      return 1;""")
    rest = poll(sid, "return window.__restore;", lambda v: bool(v) and v.get("phase") != "restoring", 120)
    if not rest or rest.get("phase") != "restored":
        die(f"restore_state failed: {rest}")
    js(sid, "window.emulator.serial_send_bytes(0, new TextEncoder().encode("
            "'echo SNAP-$(wc -l < /usr/share/apk/keys/x86/*.pub 2>/dev/null | head -1)-$(ls /usr/bin | wc -l)-OK\\n')); return 1;")
    answered = poll(sid, f"const s = (window.__m1.serial || '').slice(window.__restore.from).{STRIP};"
                         "const m = s.match(/SNAP-[0-9]*-[0-9]+-OK/);"
                         "return m ? {text: m[0], ms: performance.now() - window.__restore.at} : null;",
                    lambda v: bool(v), 60)
    print(f"  restore_state: {rest['restoreMs']:.0f} ms; after run(), a command answered: {json.dumps(answered)}")
    result = {"boot_to_prompt_ms": boot_ms, "snapshot": snap, "restore": rest, "answered": answered}
    (OUT / "result.json").write_text(json.dumps(result, indent=2))
    print(f"\n  result: {OUT / 'result.json'}")
    if not answered:
        die("the restored machine did not answer a command")
finally:
    try:
        rq("DELETE", f"/session/{sid}")
    except Exception:
        pass
