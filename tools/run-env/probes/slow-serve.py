#!/usr/bin/env python3
"""A rate-limited CORS server -- the phone link, on the desktop.

    python3 slow-serve.py <port> <root> [kbps] [bind]

Every measurement in this arc has been taken over loopback, where 14.1 MiB
arrives in 1.0s. That is not a fast version of the phone case, it is a DIFFERENT
case: the download phase -- the one the operator is worried about, the black box
before the guest says anything -- is compressed to nothing and cannot be observed
at all. A gate that never sees the slow path is green because the failing
configuration is not in the population.

Rate is applied per response in ~50ms slices, so several parallel requests share
nothing and the aggregate is higher than `kbps` -- this is a per-connection
shaper, not a link emulator, and it does not model latency or loss. It is enough
to make the download phase last long enough to watch, which is what it is for.
"""
import functools
import sys
import time
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer

KBPS = 2000
SLICE = 0.05


class Handler(SimpleHTTPRequestHandler):
    protocol_version = "HTTP/1.0"

    def end_headers(self):
        self.send_header("Access-Control-Allow-Origin", "*")
        super().end_headers()

    def copyfile(self, source, outputfile):
        chunk = max(1024, int(KBPS * 1024 / 8 * SLICE))
        while True:
            t = time.monotonic()
            buf = source.read(chunk)
            if not buf:
                break
            try:
                outputfile.write(buf)
            except (BrokenPipeError, ConnectionResetError):
                return
            left = SLICE - (time.monotonic() - t)
            if left > 0:
                time.sleep(left)

    def log_message(self, *a):
        pass


port = int(sys.argv[1])
root = sys.argv[2]
KBPS = int(sys.argv[3]) if len(sys.argv) > 3 else KBPS
bind = sys.argv[4] if len(sys.argv) > 4 else "127.0.0.1"
print(f"slow-serve {KBPS} kbps on {bind}:{port} from {root}", flush=True)
ThreadingHTTPServer((bind, port), functools.partial(Handler, directory=root)).serve_forever()
