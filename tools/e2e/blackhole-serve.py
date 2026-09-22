#!/usr/bin/env python3
"""A static server for `dist/` that can ACCEPT a request and never answer it.

This exists for one gate — **G1, the black-hole origin** — and the distinction
it draws is the whole point:

* A network that **rejects** (interface down, DNS failure, connection refused)
  produces a prompt error. Every fallback path in the app and in `sw.js` reaches
  its `.catch` and works. This is the "sometimes offline reload works" case, and
  it is *not* the failure we are chasing.
* A network that **accepts and never answers** (captive portal, half-open
  socket, a foreign LAN blackholing an old IP, an overloaded CDN edge) produces
  no error at all, for as long as the OS is willing to wait — 75-130 s on Linux,
  effectively indefinite behind a portal. Nothing catches. An unbounded `await`
  on that fetch is a permanently blank page.

`python3 -m http.server` cannot produce the second case, so until this file
existed the failure mode that costs a user their whole session was the one the
harness could not express. Note that the suite already discovered this shape by
accident, from the other side: `start_dist_server`'s readiness probe was found
by mutation "holding :8092 with a socket that accepts and never answers".

Usage
-----

    blackhole-serve.py PORT [--directory dist] [--stall /path ...]
                            [--stall-body /path ...]

`--stall` seeds the set at startup. The set is also settable at runtime, which
is required for the service-worker half of the gate: the shell has to be fetched
and cached NORMALLY before the origin can be black-holed, or there is nothing
cached to fall back to and the test proves nothing.

    GET /__blackhole?stall=/a,/b     replace the stall set, answer 200
    GET /__blackhole?stall=          clear it
    GET /__blackhole?body=/a,/b      replace the HEADERS-THEN-STALL set
    GET /__blackhole                 report the current sets

There are **two** stall modes, and they are different bugs:

* `stall` — nothing at all is written. The fetch promise never settles.
* `body` — a `200` and a complete set of headers (including a truthful
  `Content-Length`) are written, and then the body never arrives. **The fetch
  promise RESOLVES**, because `fetch` resolves on headers. Every deadline that
  disarms at that moment is now disarmed, and whatever reads the body next —
  `resp.text()`, `cache.put(resp.clone())`, the page rendering the document —
  is the thing that hangs. This is the failure one step later, and a timeout
  written only against the first mode does not catch it.

The control path is never itself stalled — a harness that can turn the black
hole on and not off is a black hole.

Behaviour of a stalled path: the request line and headers are read, then
**nothing is written and the socket is not closed** until the server is torn
down or `STALL_HOLD_SECS` elapses. No status line, no headers, no body. That is
what "accepts and never responds" means at the wire, and anything less (a slow
response, a 5xx, a close) tests a different bug.

Threaded, with daemon threads, so a stalled connection holds one thread and
blocks nothing: the harness must still be able to reach the control endpoint,
and the browser must still be able to fetch every asset that is not black-holed.
"""

import argparse
import http.server
import os
import socketserver
import sys
import threading
import time
from urllib.parse import urlparse, parse_qs

CONTROL_PATH = "/__blackhole"

# How long one stalled request occupies its thread. It only has to outlast the
# test that is watching it; it is not a timeout under test, and nothing should
# ever be asserted about it. Bounded rather than infinite purely so a leaked
# server cannot pin threads forever.
STALL_HOLD_SECS = 600

_stall_paths = set()
_body_paths = set()
_stall_lock = threading.Lock()
_shutdown = threading.Event()


def _stalled(path):
    with _stall_lock:
        return path in _stall_paths


def _body_stalled(path):
    with _stall_lock:
        return path in _body_paths


class Handler(http.server.SimpleHTTPRequestHandler):
    # Keep the default HTTP/1.0 (connection-per-request), same as
    # `python3 -m http.server`: it is what makes one stalled connection cost
    # exactly one connection. Under keep-alive a stalled request would poison a
    # connection the browser intended to reuse for something else.

    def _control(self, query):
        global _stall_paths, _body_paths
        if "stall" in query:
            raw = query.get("stall", [""])[0]
            paths = {p.strip() for p in raw.split(",") if p.strip()}
            with _stall_lock:
                _stall_paths = paths
        if "body" in query:
            raw = query.get("body", [""])[0]
            paths = {p.strip() for p in raw.split(",") if p.strip()}
            with _stall_lock:
                _body_paths = paths
        with _stall_lock:
            body = (
                "stalling: " + (", ".join(sorted(_stall_paths)) or "(nothing)") + "\n"
                + "body-stalling: " + (", ".join(sorted(_body_paths)) or "(nothing)") + "\n"
            )
        body = body.encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/plain; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _hold_body(self):
        # Headers, honestly — including a `Content-Length` that promises bytes
        # that will never come. A short body or a wrong length would be a
        # different bug (a truncated response, which the browser reports as an
        # error); this one is a body that is simply never finished, which the
        # browser reports as nothing at all.
        self.log_message('"%s" STALL-BODY (headers sent, body never)', self.requestline)
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", "4096")
        self.end_headers()
        try:
            self.wfile.flush()
        except OSError:
            return
        _shutdown.wait(STALL_HOLD_SECS)

    def _hold(self):
        # Log it explicitly: `send_response` is what normally logs, and this
        # path deliberately never calls it. A stall that leaves no line in the
        # server log is indistinguishable from a request that never arrived,
        # and telling those two apart is the entire diagnostic value here.
        self.log_message('"%s" STALL (accepted, never answered)', self.requestline)
        # Wake on shutdown so teardown is prompt; otherwise hold the socket
        # open, silent, exactly as a black-holing middlebox does.
        _shutdown.wait(STALL_HOLD_SECS)

    def do_GET(self):
        parsed = urlparse(self.path)
        if parsed.path == CONTROL_PATH:
            self._control(parse_qs(parsed.query))
            return
        if _stalled(parsed.path):
            self._hold()
            return
        if _body_stalled(parsed.path):
            self._hold_body()
            return
        super().do_GET()

    def do_HEAD(self):
        parsed = urlparse(self.path)
        if parsed.path == CONTROL_PATH:
            self._control(parse_qs(parsed.query))
            return
        if _stalled(parsed.path):
            self._hold()
            return
        super().do_HEAD()

    def end_headers(self):
        # Entry files must revalidate — the same rule the deployment reference
        # states, applied here so the harness never accidentally tests a
        # browser-cached shell when it means to test the network path.
        self.send_header("Cache-Control", "no-store")
        super().end_headers()


class Server(socketserver.ThreadingTCPServer):
    daemon_threads = True
    allow_reuse_address = True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("port", type=int)
    ap.add_argument("--directory", default="dist")
    ap.add_argument("--stall", action="append", default=[])
    ap.add_argument("--stall-body", action="append", default=[])
    args = ap.parse_args()

    if not os.path.isdir(args.directory):
        print(f"no such directory: {args.directory}", file=sys.stderr)
        return 2

    with _stall_lock:
        _stall_paths.update(args.stall)
        _body_paths.update(args.stall_body)

    os.chdir(args.directory)
    httpd = Server(("0.0.0.0", args.port), Handler)
    print(
        f"blackhole-serve: :{args.port} serving {args.directory}, "
        f"stalling {sorted(_stall_paths) or '(nothing)'}",
        file=sys.stderr,
        flush=True,
    )
    try:
        httpd.serve_forever(poll_interval=0.2)
    except KeyboardInterrupt:
        pass
    finally:
        _shutdown.set()
        httpd.server_close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
