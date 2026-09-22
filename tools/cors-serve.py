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

  …/content/{aa}/{bb}/{hash}    immutable — content-addressed by construction,
                                where `aa`/`bb` are the hash's OWN first four hex
                                characters
  name-<8+ hex>.wasm|.js        immutable — trunk stamps the hash into the name,
                                so a new build is a new URL
  everything else               no-store — including the signed root, whose
                                staleness silently defeats the `seq` floor
                                (runbook §3), and the four above

**C15: this rule is not this file's to define.** It is expressed four times in
three languages — here, `src/cache_policy_rule.rs` (which `src-tauri` also
compiles), and `docs/PUBLISHING-QUICKSTART.md` §6.2, the recipe an operator
copies into a CDN. `REVIEW-2026-08-25` §2.1 found all four disagreeing, with
`GOTCHAS.md` asserting they could not. They are pinned to ONE file now —
`tools/cache-policy-vectors.txt` — which the Rust tests and
`tools/cache-policy-lint.sh` (in `make lint`) both run. **Change the rule there
and here in the same commit, or the lint reds.**

Two divergences this file specifically had, both fixed here:

  * it tested `"/content/" in path` — a SUBSTRING, so an ingested Hugo/Zola site
    published at `/{peer}/sites/<site>/content/about.html` was pinned for a year
    while its bytes stayed mutable at a stable URL. That is the dangerous
    direction, and this is the file the quickstart tells operators to copy.
  * it tested the RAW TARGET including the query string, so
    `/index.html?x=/content/` classified immutable.

Matching the shard structure instead is exact in both directions and it is
self-verifying — and it keeps PREFIXED deployments (`/docs/content/…`), which a
`startswith("/content/")` test silently strips of all immutable caching.

    ./tools/cors-serve.py [DIR] [PORT] [--tls CERT KEY]

TLS is opt-in and exists for one reason: **service workers require a secure
context**, whose only plain-HTTP exceptions are `localhost` and `127.0.0.1`.
Served over HTTP on a LAN address — which is how every phone and second machine
reaches a dev build — no service worker registers, nothing is cached, and there
is no offline path to test. `tools/dev-cert.sh` mints the cert; that file
explains why a *trusted* one is required and a self-signed one is not enough.
"""
import functools
import http.server
import ssl
import sys

import re

# A filename whose own name carries its hash: trunk emits `entity-browser-<16
# hex>_bg.wasm` / `.js`, so a rebuild changes the URL and the old one may be
# cached forever. Anchored on a hex run of 8+ that must REACH the extension, with
# a non-empty head, so an ordinary hyphenated name is not mistaken for one.
HASHED_ASSET = re.compile(r"[^/]-[0-9a-f]{8,}(_bg)?\.(wasm|js)$")

# The content-addressed blob store. The two shard directories must be the hash's
# OWN first four hex characters — that is what makes this a statement about
# content-addressing rather than about a directory that happens to be named
# `content`, and it is what keeps an ingested Hugo/Zola `content/` tree out.
# Any prefix may precede it, so prefixed deployments keep immutable caching.
CONTENT_BLOB = re.compile(r"(?:^|/)content/([0-9a-f]{2})/([0-9a-f]{2})/([0-9a-f]{32,})$")


def is_immutable(path: str) -> bool:
    """Only bytes whose NAME is their hash may be cached hard.

    Deliberately a whitelist. The previous shape whitelisted *mutability* and
    defaulted to a one-year immutable cache, which silently applied to
    `entity-deployment.json`, `transport-profile`, `index.html` and `sw.js`.

    Mirrors `src/cache_policy_rule.rs`. Both are pinned to
    `tools/cache-policy-vectors.txt` by `tools/cache-policy-lint.sh` — the
    mirroring is GATED, not asked for in a comment, because the comment version
    is what drifted four ways (C15 / REVIEW-2026-08-25 §2.1).
    """
    # Strip the query/fragment: this used to run against the raw target, so
    # `/index.html?x=/content/` classified immutable.
    path = path.split("?", 1)[0].split("#", 1)[0]
    m = CONTENT_BLOB.search(path)
    if m and m.group(3).startswith(m.group(1) + m.group(2)):
        return True
    return bool(HASHED_ASSET.search(path))


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
    argv = sys.argv[1:]
    cert = key = None
    if "--tls" in argv:
        i = argv.index("--tls")
        try:
            cert, key = argv[i + 1], argv[i + 2]
        except IndexError:
            sys.exit("cors-serve: --tls needs CERT and KEY paths")
        argv = argv[:i] + argv[i + 3:]

    directory = argv[0] if len(argv) > 0 else "."
    port = int(argv[1]) if len(argv) > 1 else 8099
    handler = functools.partial(Handler, directory=directory)

    with http.server.ThreadingHTTPServer(("0.0.0.0", port), handler) as httpd:
        scheme = "http"
        if cert:
            # `SimpleHTTPRequestHandler` speaks HTTP/1.0 by default, which makes
            # every response a connection teardown. Harmless over plain HTTP;
            # over TLS it is a full handshake per asset, and the shell pulls
            # tens of MB of wasm. HTTP/1.1 keeps the connection alive.
            # Set on the CLASS, not on the `functools.partial` — the partial
            # constructs `Handler`, so an attribute on it is read by nothing.
            Handler.protocol_version = "HTTP/1.1"
            ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            ctx.load_cert_chain(cert, key)
            httpd.socket = ctx.wrap_socket(httpd.socket, server_side=True)
            scheme = "https"
        print(
            f"serving {directory} on {scheme}://localhost:{port} "
            "with CORS + cache headers"
        )
        if scheme == "https":
            print(
                "  TLS on — reachable by LAN address as a SECURE CONTEXT, so "
                "service workers register.\n"
                "  The testing device must trust the CA (tools/dev-cert.sh "
                "prints how); clicking\n"
                "  through a certificate warning is NOT enough for Chrome to "
                "register a worker."
            )
        httpd.serve_forever()


if __name__ == "__main__":
    main()
