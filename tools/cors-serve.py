#!/usr/bin/env python3
"""Static file server that sends the CORS headers a browser consumer needs.

`python3 -m http.server` sends NONE, so a cross-origin fetch of a published tree
fails in a browser while `curl` succeeds — the exact gap
RUNBOOK-CDN-BROWSER-DEPLOYMENT was written for ("the corridor-close impl-evidence
(`python3 -m http.server`) is native and sends no CORS headers, so the cited demo
cannot replicate in a browser").

Headers, per runbook §2.1:

  Access-Control-Allow-Origin: *      fully-public, fully content-addressed — there
                                      is nothing CORS protects here that the hash
                                      does not. No credentials, no cookies.
  Access-Control-Allow-Methods: GET, HEAD
  Access-Control-Allow-Headers: Range

No `Access-Control-Allow-Credentials`: this path is credential-free by design —
authority lives in the signed entity/capability layer, never in HTTP cookies.

Also sets `Cache-Control: no-store` on the two MUTABLE endpoints (the signed root
and any `.list`), because the same-origin shape's real hazard is a cache serving
a stale `published-root` and silently defeating the `seq`-monotonicity check
(runbook §3). Content blobs are hash-addressed and immutable, so they cache hard.

    ./tools/cors-serve.py [DIR] [PORT]
"""
import functools
import http.server
import sys

MUTABLE_SUFFIXES = ("published-root", ".list")


class Handler(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Methods", "GET, HEAD")
        self.send_header("Access-Control-Allow-Headers", "Range")
        if self.path.endswith(MUTABLE_SUFFIXES):
            self.send_header("Cache-Control", "no-store")
        else:
            self.send_header("Cache-Control", "public, max-age=31536000, immutable")
        super().end_headers()

    def log_message(self, fmt, *args):
        sys.stderr.write("%s %s\n" % (self.address_string(), fmt % args))


def main():
    directory = sys.argv[1] if len(sys.argv) > 1 else "."
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 8099
    handler = functools.partial(Handler, directory=directory)
    with http.server.ThreadingHTTPServer(("0.0.0.0", port), handler) as httpd:
        print(f"serving {directory} on http://localhost:{port} with CORS + cache headers")
        httpd.serve_forever()


if __name__ == "__main__":
    main()
