#!/usr/bin/env python3
"""Probe server. argv: port root mode  (mode = plain | corp | credentialless)"""
import functools, sys
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
MODE = sys.argv[3]
class H(SimpleHTTPRequestHandler):
    extensions_map = {**SimpleHTTPRequestHandler.extensions_map, ".js": "text/javascript", ".bin": "application/octet-stream"}
    def end_headers(self):
        self.send_header("Cache-Control", "no-store")
        # opaque-origin sandboxed frames fetch with Origin: null -> CORS needed (same as plain-serve.py)
        self.send_header("Access-Control-Allow-Origin", "*")
        if MODE != "plain":
            self.send_header("Cross-Origin-Opener-Policy", "same-origin")
            self.send_header("Cross-Origin-Embedder-Policy", "require-corp" if MODE == "corp" else "credentialless")
            self.send_header("Cross-Origin-Resource-Policy", "cross-origin")
        if "-tao" in self.path:
            self.send_header("Timing-Allow-Origin", "*")
        super().end_headers()
    def log_message(self, fmt, *a):
        pass
port, root = int(sys.argv[1]), sys.argv[2]
print(f"{MODE} on 127.0.0.1:{port}", flush=True)
ThreadingHTTPServer(("127.0.0.1", port), functools.partial(H, directory=root)).serve_forever()
