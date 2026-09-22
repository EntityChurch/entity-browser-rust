#!/usr/bin/env python3
"""Loopback static server that sends NO cross-origin isolation headers.

The deliberate opposite of coi-serve.py, and the instrument for M1: an origin with
no COOP/COEP is not cross-origin isolated, so SharedArrayBuffer is absent. Whether an
engine still boots there is the question that decides whether the isolation posture
(rung 0) is forced for every path or only for the Nix-capable one.

Bound to 127.0.0.1: this serves mirrored third-party bytes.
"""
import functools
import sys
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer


class Handler(SimpleHTTPRequestHandler):
    extensions_map = {
        **SimpleHTTPRequestHandler.extensions_map,
        ".wasm": "application/wasm",
        ".js": "text/javascript",
    }

    def end_headers(self):
        # No COOP/COEP — any isolation header added here voids the M1 measurement.
        #
        # ACAO *is* sent, and it is not a contradiction: entity-apps' EMBEDDING.md
        # §5 says load an app `sandbox="allow-scripts"` and NOT allow-same-origin,
        # which puts the app in an OPAQUE origin. Its document still loads (frame
        # navigation is not CORS-gated) but every CORS-gated subresource fetch --
        # notably WebAssembly.instantiateStreaming -- then arrives with
        # `Origin: null` and needs an explicit allow. Without this the VM boots at
        # the top level and fails only inside the sandbox that is the actual
        # deployment shape.
        self.send_header("Access-Control-Allow-Origin", "*"); self.send_header("Cross-Origin-Opener-Policy", "same-origin"); self.send_header("Cross-Origin-Embedder-Policy", "require-corp"); self.send_header("Cross-Origin-Resource-Policy", "cross-origin")
        super().end_headers()

    def log_message(self, fmt, *args):
        sys.stderr.write("%s %s\n" % (self.address_string(), fmt % args))


port = int(sys.argv[1])
root = sys.argv[2]
# Third arg = bind address. Default stays loopback: this serves mirrored
# third-party bytes, so reaching the LAN is a deliberate act, not the default.
bind = sys.argv[3] if len(sys.argv) > 3 else "127.0.0.1"
print(f"plain (no COOP/COEP) on {bind}:{port} from {root}", flush=True)
ThreadingHTTPServer((bind, port), functools.partial(Handler, directory=root)).serve_forever()
