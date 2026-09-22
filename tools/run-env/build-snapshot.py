#!/usr/bin/env python3
"""build-snapshot.py -- a snapshot of the Alpine Environment taken at the prompt,
so a launch RESUMES the machine instead of booting it (PLAN-2026-09-13 S1, §6).

    make serve PORT=8216 DIST=tools/run-env/alpine-guest     # from the repo root
    GRID=http://127.0.0.1:4480 python3 build-snapshot.py http://127.0.0.1:8216/

Writes  alpine-guest/guest/snapshot.bin.zst  and  alpine-guest/guest/snapshot.json.
package-app.sh ships both in the `image` bundle.

HOW: opens the standalone page with ?snapshot=build in a real browser. The page
boots cold with the same MACHINE definition it uses to run, waits for the prompt
and the guest agent, asks the agent to zero free memory (`zerofill`), saves the
state and gzips it; this script copies the gzip out and recompresses it with
zstd, which is what the engine's restore path reads.

WHAT IT RECORDS: the identities the PAGE computed for the engine, the image
index, the package index and the machine settings. The page resumes only when
all four match what it is given at launch, so a rebuilt image, a new package set
or a changed kernel command line boots cold instead of resuming a snapshot that
does not fit. Rebuild the snapshot after build-guest.sh (which clears guest/, so
the old one goes with it) and after build-packages.sh.

zstd -19 WITHOUT --long, deliberately: a long window makes the decoder allocate
it, and the decoder runs inside v86's own wasm memory on a phone.
"""
import base64
import gzip
import json
import os
import pathlib
import subprocess
import sys
import time
import urllib.request

HERE = pathlib.Path(__file__).resolve().parent
GRID = os.environ.get("GRID", "http://127.0.0.1:4444")
URL = (sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:8216/").rstrip("/") + "/index.html?snapshot=build"
OUT = HERE / "alpine-guest" / "guest"


def rq(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(GRID + path, data=data, method=method, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=300) as r:
        return json.loads(r.read() or b"{}")


def js(sid, script):
    return rq("POST", f"/session/{sid}/execute/sync", {"script": script, "args": []})["value"]


if not (OUT / "fs.json").exists():
    sys.exit(f"no built guest at {OUT} -- run ./build-guest.sh first")

print(f"==> building a snapshot from {URL} (grid {GRID})")
caps = {"capabilities": {"alwaysMatch": {"browserName": "firefox", "moz:firefoxOptions": {"args": ["-headless"]}}}}
sid = rq("POST", "/session", caps)["value"]["sessionId"]
try:
    rq("POST", f"/session/{sid}/url", {"url": URL})
    deadline, last = time.time() + 420, None
    while time.time() < deadline:
        out = js(sid, "const o = window.__snapshotOut; return o ? {phase: o.phase, error: o.error || null, raw: o.raw || null, "
                      "gzip: o.gzip ? o.gzip.length : null, ids: o.ids || null} : "
                      "{phase: 'page', error: window.__m1 && window.__m1.error};")
        if out.get("phase") != last:
            print(f"    {out.get('phase')}")
            last = out.get("phase")
        if out.get("phase") in ("ready", "error") or (out.get("phase") == "page" and out.get("error")):
            break
        time.sleep(1)
    if out.get("phase") != "ready":
        sys.exit(f"the page did not produce a snapshot: {out}")
    ids = out["ids"]
    chunk = 6 * 1024 * 1024
    gz = bytearray()
    for off in range(0, out["gzip"], chunk):
        gz += base64.b64decode(js(sid, f"""
          const s = window.__snapshotOut.gzip.subarray({off}, {off + chunk});
          let bin = ''; for (let i = 0; i < s.length; i += 32768) bin += String.fromCharCode.apply(null, s.subarray(i, i + 32768));
          return btoa(bin);"""))
finally:
    try:
        rq("DELETE", f"/session/{sid}")
    except Exception:
        pass

raw = gzip.decompress(bytes(gz))
if len(raw) != out["raw"]:
    sys.exit(f"copied state is {len(raw)} bytes, the page saved {out['raw']}")
# zstd in a container: the host needs only make + podman.
zst = subprocess.run(["podman", "run", "--rm", "-i", "docker.io/library/alpine:3.21", "sh", "-c",
                      "apk add --no-cache zstd >/dev/null 2>&1 && zstd -19 -T0 -q -c"],
                     input=raw, capture_output=True, check=True).stdout
(OUT / "snapshot.bin.zst").write_bytes(zst)
meta = {"engine": ids["engine"], "image": ids["image"], "packages": ids["packages"], "machine": ids["machine"],
        "raw_bytes": len(raw), "zstd_bytes": len(zst)}
(OUT / "snapshot.json").write_text(json.dumps(meta, indent=2) + "\n")
print(f"    state {len(raw) / 1048576:.1f} MiB -> zstd {len(zst) / 1048576:.1f} MiB")
print(f"    packages: {'included' if ids['packages'] else 'none'}")
print(f"==> {OUT / 'snapshot.bin.zst'}")
