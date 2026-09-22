#!/usr/bin/env python3
"""C16 — what is each domain serving, and are its cache headers safe?

Two halves of one probe, because they answer the two questions a deploy leaves
open and they need the same requests:

**Inventory — *which build is each domain on?*** A branch is bookkeeping; the
only authority on what a user is running is the artifact the domain is serving.
Measured 2026-08-26: both live domains were on `archive/dev-0.9.0`, reachable
from **neither `dev` nor `master`**, and no branch comparison would have said so
(design §4A.0). `tools/build-stamp.sh` had already made this answerable and
nobody had asked. This half needs no code and no deploy — it is a `curl` per
domain — and it belongs in the deploy runbook.

**Headers — *can this deployment be corrected?*** Every mutable URL served with
`immutable` or a long `max-age` is a file that cannot be fixed for the life of
the TTL. Brick-matrix cells #7/#8, and **#9 — both browser and CDN — has no
remedy at all**. This half was blocked on C15, because a probe needs a rule to
check against and there were four disagreeing ones. It now imports the same
`is_immutable` that `tools/cors-serve.py` serves with and that
`tools/cache-policy-vectors.txt` pins.

## GET, never HEAD

Cloudflare does not populate its cache from a `HEAD`, so a `HEAD` probe can
report headers the cached path never produces (`REVIEW-2026-08-25` §1). Every
request here is a `GET`, and the body is read and discarded.

## What it cannot tell you

It sees what the origin says *to this client, now*. A CDN can answer differently
per POP, per cache state, and to a browser sending different `Accept` headers.
A clean run is evidence, not proof — the same limit the deployment docs state
about checking with `curl` instead of a browser.

Usage:

    tools/fleet-probe.py https://example.org [https://other.org ...]
    make fleet-probe DOMAINS="https://example.org https://other.org"

Exit status: 0 clean, 1 a mutable URL is cached dangerously, 2 a domain could not
be reached at all. Under-caching an immutable file is reported and does NOT fail
the run — it is slow, not unrecoverable, and conflating the two directions is how
the rule got wrong in the first place.
"""
import argparse
import importlib.util
import re
import sys
import urllib.error
import urllib.request

# The shared rule. Imported, not restated — C15 exists because this policy had
# four hand-written expressions that drifted.
_spec = importlib.util.spec_from_file_location(
    "cors_serve", __file__.rsplit("/", 1)[0] + "/cors-serve.py"
)
_cors = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_cors)
is_immutable = _cors.is_immutable

# A mutable file may be cached briefly — the live Cloudflare config uses
# `max-age=60, must-revalidate`, which stores AND revalidates and is fine. What
# is not fine is a TTL long enough that a fix cannot land. Anything above this
# is treated as unrecoverable rather than as tuning.
MAX_MUTABLE_AGE_S = 300

def meta_content(name, html):
    """The content of `<meta name="{name}">`, or None if the shell has no such tag.

    ONE expression for "read a build stamp out of a shell", because
    `tools/build-stamp.sh` now writes THREE of them and a second hand-written
    regex is how two of them end up matched differently (C15's rule, applied to
    this file rather than re-learned in it).

    Note what the None means and keep it apart from an empty-ish value: no tag
    at all is "this deployment predates the stamp", while a tag reading
    `unknown` is "the build ran and could not tell" — build-stamp emits that
    deliberately. Two different facts; do not collapse them.
    """
    m = re.search(
        r'<meta\s+name=["\']' + re.escape(name) + r'["\']\s+content=["\']([^"\']*)["\']',
        html,
        re.I,
    )
    return m.group(1) if m else None
# Hashed assets referenced by the shell. These are the URLs a stale-cache
# incident actually pins, so they are probed rather than assumed.
HASHED_REF = re.compile(r'["\'\(]([^"\'\)\s]*-[0-9a-f]{8,}(?:_bg)?\.(?:wasm|js))["\'\)]')
# The main bundle's hash = the BUILD ID (§3.1). Mirrors the shape
# `src/build_id.rs::parse_bundle_hash` and `sw.js` match — needle, 8+ hex, then a
# REQUIRED extension. Applied to a filename already matched by `HASHED_REF`, not
# to the raw document, so it cannot be hijacked by prose the way an unanchored
# scan of `index.html` was on 2026-09-02.
BUNDLE_ID = re.compile(r"^entity-browser-([0-9a-f]{8,})(?:_bg)?\.(?:js|wasm)$")

# Always probed, whether or not the shell references them. Each is a file whose
# staleness has its own failure mode: the shell nobody reloads, the worker that
# pins it, the config that carries the registry pin and the home site.
ALWAYS = ["/", "/index.html", "/sw.js", "/entity-deployment.json"]


def get(url, timeout):
    """GET (never HEAD — see the module note) and return (status, headers, body)."""
    req = urllib.request.Request(url, method="GET", headers={"User-Agent": "entity-fleet-probe"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status, dict(r.headers), r.read()
    except urllib.error.HTTPError as e:
        # A 404 is an ANSWER — a deployment that serves no config on purpose is
        # not a deployment nobody could reach. Keep them apart.
        return e.code, dict(e.headers or {}), b""
    except Exception as e:  # noqa: BLE001 — every transport fault reads the same here
        return None, {"__error__": str(e)}, b""


def max_age(cache_control):
    m = re.search(r"max-age\s*=\s*(\d+)", cache_control or "", re.I)
    return int(m.group(1)) if m else None


def classify(path, cache_control):
    """-> (verdict, note). `verdict` in {ok, DANGEROUS, under-cached, unknown}."""
    cc = (cache_control or "").strip()
    want_immutable = is_immutable(path)
    if not cc:
        # "No Cache-Control" is NOT "uncached": browsers apply heuristic
        # freshness off Last-Modified and serve a stale shell without
        # revalidating. For a mutable file that is the bug, not the absence of one.
        return ("DANGEROUS", "no Cache-Control at all — heuristic freshness applies") \
            if not want_immutable else ("under-cached", "no Cache-Control")
    has_immutable = "immutable" in cc.lower()
    age = max_age(cc)
    if want_immutable:
        if has_immutable or (age or 0) >= 86400:
            return "ok", cc
        return "under-cached", f"{cc} — content-addressed, could be cached for a year"
    # Mutable.
    if has_immutable:
        return "DANGEROUS", f"{cc} — mutable file marked immutable"
    if age is not None and age > MAX_MUTABLE_AGE_S:
        return "DANGEROUS", f"{cc} — mutable file with max-age {age}s (> {MAX_MUTABLE_AGE_S}s)"
    return "ok", cc


def probe_domain(base, timeout):
    base = base.rstrip("/")
    out = {
        "domain": base,
        "build": None,      # the commit LABEL
        "build_id": None,   # the bundle hash — the IDENTITY (§3.1)
        "core_ref": None,   # the SIBLING KERNEL commit — the other half of the pair
        "reachable": False,
        "rows": [],
        "notes": [],
    }

    status, headers, body = get(base + "/", timeout)
    if status is None:
        out["notes"].append(f"unreachable: {headers.get('__error__')}")
        return out
    out["reachable"] = True
    shell = body.decode("utf-8", "replace")

    out["build"] = meta_content("entity-build", shell)
    if not out["build"]:
        out["notes"].append(
            "no <meta name=\"entity-build\"> in the shell — this deployment predates "
            "the build stamp (C5), so nothing here can say which commit it is running"
        )

    # THE OTHER HALF OF THE PAIR. This crate links `entity-core-rust` by path
    # dependency across twenty paths with NO cross-repo lockfile, so the bundle
    # hash is a function of OUR commit *and* whatever sibling checkout was on
    # disk. `(entity-build, entity-core-ref)` is the smallest thing that
    # identifies a reproducible build; a deployed shell has stamped both since
    # 2026-09-05 and this probe could not read the second one until now, so the
    # deployed pair was RECORDED and not OBSERVABLE.
    out["core_ref"] = meta_content("entity-core-ref", shell)
    if out["build"] and not out["core_ref"]:
        out["notes"].append(
            "the shell stamps a commit but no <meta name=\"entity-core-ref\"> — built "
            "before 2026-09-05, so the kernel half of its build pair is unrecoverable "
            "from the artifact"
        )

    paths = list(ALWAYS)
    for ref in sorted(set(HASHED_REF.findall(shell))):
        p = ref if ref.startswith("/") else "/" + ref.lstrip("./")
        if p not in paths:
            paths.append(p)
    out["bundles"] = [p for p in paths if p not in ALWAYS]
    # The BUILD IDENTITY, per design §3.1 and `src/build_slots.rs`: a build is
    # identified by its main bundle hash and merely *labelled* by the commit.
    # Derived from the bundle references already extracted above rather than by a
    # second scan of the document — the extension is what makes a prose mention of
    # the needle a non-match, and `HASHED_REF` has already required it.
    for p in out["bundles"]:
        m2 = BUNDLE_ID.search(p.rsplit("/", 1)[-1])
        if m2:
            out["build_id"] = m2.group(1)
            break
    if not out["bundles"]:
        out["notes"].append(
            "no hash-named bundle referenced by the shell — either the shell did not "
            "parse or this is not an entity-browser deployment"
        )

    for p in paths:
        st, hd, _ = get(base + p, timeout)
        if st is None:
            out["rows"].append((p, None, "unknown", hd.get("__error__", "unreachable")))
            continue
        if st == 404:
            # Reported, never judged: a domain may serve no deployment document
            # by choice. Caching a 404 is a separate question this cannot see.
            out["rows"].append((p, st, "absent", "404 — not served"))
            continue
        cc = hd.get("Cache-Control") or hd.get("cache-control")
        verdict, note = classify(p, cc)
        out["rows"].append((p, st, verdict, note))
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("domains", nargs="+", help="origins, e.g. https://example.org")
    ap.add_argument("--timeout", type=float, default=10.0)
    args = ap.parse_args()

    results = [probe_domain(d, args.timeout) for d in args.domains]

    print("=== FLEET INVENTORY — which build is each domain serving? ===")
    for r in results:
        if not r["reachable"]:
            print(f"  {r['domain']:<44} UNREACHABLE")
            continue
        bundles = ", ".join(b.rsplit("/", 1)[-1] for b in r.get("bundles", [])) or "—"
        print(
            f"  {r['domain']:<44} build={r.get('build_id') or '(unidentified)'}"
            f"  pair=({r['build'] or '(unstamped)'}, {r.get('core_ref') or '(unstamped)'})"
            f"  {bundles}"
        )

    # Uniformity is a question about the BUILD, and the build is the bundle hash
    # (§3.1). Two docs-only commits produce byte-identical wasm and are ONE
    # rollback slot — `BuildsManifest::record` collapses them by construction, so
    # judging uniformity by the commit label would make this probe and the
    # publisher disagree about how many builds exist. Measured 2026-09-02: the two
    # live domains carried two commit labels on one identical bundle, and the
    # label-based verdict called that "NOT uniform".
    ids = {r["build_id"] for r in results if r["reachable"] and r.get("build_id")}
    labels = {r["build"] for r in results if r["reachable"] and r["build"]}
    if len(ids) > 1:
        print(f"  NOTE: the fleet is NOT uniform — {len(ids)} distinct builds: {sorted(ids)}")
        # Print the PAIR beside each divergent build, because the most common
        # cause of "same commit, different bundle" is the kernel half moving
        # under a local build — which is invisible if only the commit is shown.
        # Reported, never the verdict: identity is the bundle hash (§3.1), and
        # two kernel commits can legitimately produce one bundle when the
        # difference did not reach us.
        for r in results:
            if r["reachable"] and r.get("build_id"):
                print(
                    f"        {r['build_id']}  ← ({r['build'] or '(unstamped)'}, "
                    f"{r.get('core_ref') or '(unstamped)'})  {r['domain']}"
                )
    elif len(ids) == 1:
        print(f"  the fleet is uniform on build {ids.pop()}")

    # A DIRTY stamp on either half means the deployed bytes were built from a
    # working tree nobody can reconstruct. Reported and NOT a failure, on this
    # probe's own axis: the failure question here is "can this deployment be
    # corrected?", and the answer is yes — republish. It is still the single
    # most important thing to know before quoting a build id to a deployer.
    dirty = [
        r for r in results
        if r["reachable"] and (
            (r["build"] or "").endswith("-dirty") or (r.get("core_ref") or "").endswith("-dirty")
        )
    ]
    for r in dirty:
        print(
            f"  NOTE: {r['domain']} was built from a DIRTY tree — pair "
            f"({r['build']}, {r.get('core_ref')}). These bytes are not reproducible "
            "from any commit; republish from a clean pair before treating this as a "
            "rollback target"
        )

    # "We could not tell" is not "nobody recorded it" — build-stamp writes the
    # literal `unknown` when the sibling checkout was absent or unreadable, and
    # collapsing that into the unstamped case loses the one fact that says a
    # build ran and failed to answer.
    unknown_core = [
        r for r in results if r["reachable"] and r.get("core_ref") == "unknown"
    ]
    for r in unknown_core:
        print(
            f"  NOTE: {r['domain']} stamps entity-core-ref=unknown — the build could not "
            "read the sibling kernel checkout, so its pair is incomplete by measurement "
            "rather than by age"
        )

    if len(ids) <= 1 and len(labels) > 1:
        # Reported, never a failure: same code, different provenance stamps.
        print(
            f"  NOTE: one build, {len(labels)} distinct commit labels: {sorted(labels)} — the "
            "commit is a label, not the identity (§3.1), so this is ONE rollback slot"
        )

    print()
    print("=== CACHE HEADERS — can this deployment be corrected? ===")
    dangerous = 0
    unreachable = 0
    for r in results:
        print(f"  {r['domain']}")
        if not r["reachable"]:
            unreachable += 1
            for n in r["notes"]:
                print(f"      {n}")
            continue
        for n in r["notes"]:
            print(f"      note: {n}")
        for path, status, verdict, note in r["rows"]:
            if verdict == "DANGEROUS":
                dangerous += 1
            mark = {"ok": "  ", "DANGEROUS": "!!", "under-cached": " ~", "absent": " ·",
                    "unknown": " ?"}[verdict]
            print(f"    {mark} {str(status or '---'):>3} {path:<46} {note}")

    print()
    if dangerous:
        print(f"FAIL — {dangerous} mutable URL(s) cached in a way that cannot be corrected.")
        print("Every one of these is a file a fix will not reach until its TTL expires.")
        print("The rule is tools/cache-policy-vectors.txt; the CDN recipe is")
        print("docs/PUBLISHING-QUICKSTART.md §6.2.")
        return 1
    if unreachable:
        print(f"INCONCLUSIVE — {unreachable} domain(s) could not be reached. Nothing is")
        print("claimed about them: 'could not check' is not 'healthy'.")
        return 2
    print("OK — no mutable URL is cached beyond correction on any domain probed.")
    print("Under-cached (~) entries are slow, not unrecoverable, and do not fail this run.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
