#!/usr/bin/env python3
"""Gate: the apex is usable, and crawlable, by an agent that does not run JS.

# Why this exists

The apex serves a WASM SPA. To anything that does not execute JavaScript — a
search crawler that does not run JS, a text browser, a reader with JS off, a
link-preview fetcher — that is an **empty body**: no text, no links, nothing to
index, and a boot spinner frozen on "Loading Entity Browser..." forever. The
published sites are plain crawlable HTML sitting right there under `/sites/`,
and nothing linked to them.

`index.html` now carries a `<noscript>` landing. This is what proves it, and it
is the ONLY thing that can: the e2e suite drives a browser with JS **on** by
definition, so no phase there can see this surface. Same shape as every other
gap in this repo's connectivity arc — the mechanism is right and nothing
exercises the posture it exists for.

# The control is the point

Asserting "the landing is visible with JS off" proves nothing on its own: a
landing that is ALWAYS visible satisfies it, and that would mean every ordinary
visitor sees the fallback under the app. So this drives **both** postures and
requires the JS-on run to hide it. A gate never seen red is not a gate.

# Three properties, and they are not the same property

- **visible**  — with JS off, the landing renders (the human half).
- **spinner hidden** — with JS off, the boot spinner does NOT sit there
  claiming to be loading something that will never load.
- **anchor in source** — `href="sites/"` is in the served HTML. This is the
  *crawler* half and it is independent of rendering: a crawler parses bytes and
  never runs our CSS, so a link that is only reachable through a stylesheet is
  not reachable by it at all.

# Running it

    make noscript-check          # builds dist/ first
    python3 tools/noscript-check.py [dist-dir]

Needs the Selenium grid on :4444 (`make e2e-worker`'s container; see
tools/e2e/README.md). **Do not run it beside the e2e suite** — `setup()` there
DELETEs every session on the grid, so the two take each other down.

The static file server is a daemon thread in this process, so it dies when the
script exits and leaves nothing listening. That is deliberate: agent seats on
this box cannot kill processes, so a gate that backgrounds a server is a gate
that litters.
"""
import functools
import http.server
import json
import socket
import sys
import threading
import time
import urllib.error
import urllib.request

GRID = "http://localhost:4444"
# Generous: a healthy JS-on boot is well under this, and it is only paid by the
# control run. An upper bound, not an expectation.
BOOT_BUDGET_S = 3


def free_port():
    s = socket.socket()
    s.bind(("0.0.0.0", 0))
    port = s.getsockname()[1]
    s.close()
    return port


def rq(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(
        GRID + path, data=data, method=method,
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.loads(r.read().decode())


def probe(url, js_enabled):
    """Load `url` with JS on or off; report what a visitor and a crawler see."""
    caps = {"capabilities": {"alwaysMatch": {
        "browserName": "firefox",
        "moz:firefoxOptions": {"prefs": {"javascript.enabled": js_enabled}},
    }}}
    sid = rq("POST", "/session", caps)["value"]["sessionId"]
    try:
        rq("POST", f"/session/{sid}/url", {"url": url})
        if js_enabled:
            time.sleep(BOOT_BUDGET_S)
        out = {}
        for name, sel in [("landing", ".noscript-landing"), ("spinner", "#loading")]:
            try:
                el = rq("POST", f"/session/{sid}/element",
                        {"using": "css selector", "value": sel})["value"]
                eid = list(el.values())[0]
                out[name] = rq("GET", f"/session/{sid}/element/{eid}/displayed")["value"]
            except urllib.error.HTTPError:
                out[name] = "absent"
        out["anchor_in_source"] = 'href="sites/"' in rq("GET", f"/session/{sid}/source")["value"]
        return out
    finally:
        rq("DELETE", f"/session/{sid}")


class QuietHandler(http.server.SimpleHTTPRequestHandler):
    """Static handler with the request log off."""

    def log_message(self, *args, **kwargs):
        pass


class QuietServer(http.server.ThreadingHTTPServer):
    """...and with a broken-pipe traceback off.

    Tearing down a session mid-transfer resets the connection — the wasm
    preload is tens of MB and the browser is rarely finished with it — and the
    stock server dumps a 20-line `ConnectionResetError` traceback that reads
    like a failure in a run that passed. A client hanging up is not this
    gate's business.
    """

    def handle_error(self, request, client_address):
        exc = sys.exc_info()[1]
        if isinstance(exc, (ConnectionResetError, BrokenPipeError)):
            return
        super().handle_error(request, client_address)


def main():
    dist = sys.argv[1] if len(sys.argv) > 1 else "dist"
    port = free_port()
    srv = QuietServer(("0.0.0.0", port), functools.partial(QuietHandler, directory=dist))
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    url = f"http://localhost:{port}/"
    print(f"noscript-check: serving {dist}/ at {url}")

    off = probe(url, False)
    on = probe(url, True)
    print(f"  JS off: {off}")
    print(f"  JS on : {on}")

    failures = []
    if off["landing"] is not True:
        failures.append(
            "with JS off the no-script landing must be VISIBLE — a crawler and a "
            "reader with JS off currently see an empty page")
    if off["spinner"] is not False:
        failures.append(
            "with JS off the boot spinner must be HIDDEN — otherwise it sits on "
            "'Loading Entity Browser...' forever, claiming to load something that cannot")
    if not off["anchor_in_source"]:
        failures.append(
            'href="sites/" must be in the served HTML SOURCE — a crawler parses '
            "bytes and never runs our CSS, so a link reachable only through a "
            "stylesheet is not reachable by it")
    if on["landing"] is not False:
        failures.append(
            "CONTROL: with JS on the no-script landing must be HIDDEN — otherwise "
            "every ordinary visitor sees the fallback under the app, and the "
            "JS-off assertion above is vacuous")

    for f in failures:
        print(f"noscript-check: FAIL — {f}")
    if failures:
        return 1
    print("noscript-check: OK — no-script landing visible and linked, hidden with JS on")
    return 0


if __name__ == "__main__":
    sys.exit(main())
