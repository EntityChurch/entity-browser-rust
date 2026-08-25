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

Cache-Control is **opt-IN to immutable**, not opt-in to mutable — and that
inversion is a bug fix, not a style choice.

The rule used to be "`published-root` and `.list` are mutable, EVERYTHING ELSE is
`immutable, max-age=31536000`". A deployment root holds four more mutable files
that nobody enumerated: `entity-deployment.json` (the registry pin, the home
site, the whole posture), `transport-profile` (where this publisher is served
from), `index.html` (the SPA entry point) and `sw.js`. Under the old rule a CDN
was told to cache each of them **for a year**: change your registry pin and no
returning visitor sees it; ship a new build and nobody loads it.

Invisible locally — a fresh container has no cache — and a one-year TTL is not
something you notice the day you set it. So the default is now the safe
direction: **immutable only for bytes whose NAME is their hash**, everything else
revalidated. An under-cached immutable file is slow; a mis-cached mutable file is
a deployment that cannot be corrected.

  /content/…                    immutable — content-addressed by construction
  name-<16 hex>.wasm|.js        immutable — trunk stamps the hash into the name,
                                so a new build is a new URL
  everything else               no-store — including the signed root, whose
                                staleness silently defeats the `seq` floor
                                (runbook §3), and the four above

    ./tools/cors-serve.py [DIR] [PORT]
"""
import functools
import http.server
import sys

import re

# A filename whose own name carries its hash: trunk emits `entity-browser-<16
# hex>_bg.wasm` / `.js`, so a rebuild changes the URL and the old one may be
# cached forever. Anchored on a hex run of 8+ to avoid calling an ordinary
# hyphenated name content-addressed.
HASHED_ASSET = re.compile(r"-[0-9a-f]{8,}(_bg)?\.(wasm|js)$")


def is_immutable(path: str) -> bool:
    """Only bytes whose NAME is their hash may be cached hard.

    Deliberately a whitelist. The previous shape whitelisted *mutability* and
    defaulted to a one-year immutable cache, which silently applied to
    `entity-deployment.json`, `transport-profile`, `index.html` and `sw.js`.
    """
    return "/content/" in path or bool(HASHED_ASSET.search(path))


class Handler(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Methods", "GET, HEAD")
        self.send_header("Access-Control-Allow-Headers", "Range")
        if is_immutable(self.path):
            self.send_header("Cache-Control", "public, max-age=31536000, immutable")
        else:
            self.send_header("Cache-Control", "no-store")
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
