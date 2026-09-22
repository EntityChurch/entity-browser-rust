#!/usr/bin/env python3
"""build-about.py -- the package half of the Alpine machine's ⓘ panel.

    ./build-about.py            # after build-guest.sh (and build-packages.sh, if used)

Writes alpine-guest/guest/about.json: every package the IMAGE ships and every
package the PACKAGE SET offers, with version, licence, source origin, repository
and the exact aports directory its source is built from.

READ FROM WHAT SHIPS, NEVER FROM A LIST: the image's rows come from its own apk
database (/lib/apk/db/installed inside fs.json) and the set's rows from the signed
APKINDEX inside the package bundle. A panel generated from packages.txt would
describe what we meant to ship; this describes what we did.

The repository (main/community) is not in either database, so it comes from
Alpine's own indexes, cached in .survey/ (the cache package-survey.py uses) and
fetched once if absent. A package found in neither is reported with no repository
rather than guessed.

DESIGN-2026-09-14 §3/§4.4. Information, not a compliance claim.
"""
import io
import json
import pathlib
import sys
import tarfile
import urllib.request

HERE = pathlib.Path(__file__).resolve().parent
GUEST = HERE / "alpine-guest" / "guest"
BRANCH = "3.22"
SURVEY = HERE / ".survey" / BRANCH


def find(nodes, parts):
    for n in nodes:
        if n[0] == parts[0]:
            return n if len(parts) == 1 else find(n[6], parts[1:])
    return None


def records(text):
    cur = {}
    for line in text.splitlines() + [""]:
        if not line:
            if cur:
                yield cur
            cur = {}
        elif len(line) > 2 and line[1] == ":":
            cur[line[0]] = line[2:]


def repo_index():
    repo = {}
    for r in ("main", "community"):
        p = SURVEY / f"{r}.APKINDEX"
        if not p.exists():
            SURVEY.mkdir(parents=True, exist_ok=True)
            url = f"https://dl-cdn.alpinelinux.org/alpine/v{BRANCH}/{r}/x86/APKINDEX.tar.gz"
            print(f"    fetching {url}")
            data = urllib.request.urlopen(url, timeout=60).read()
            with tarfile.open(fileobj=io.BytesIO(data)) as t:
                p.write_bytes(t.extractfile("APKINDEX").read())
        for rec in records(p.read_text(errors="replace")):
            repo.setdefault(rec.get("P"), r)
    return repo


def check_packs(image_recs, available_recs):
    """Every name a pack installs must exist offline: a package in the image or the
    set, or something one of them provides (p7zip is provided by 7zip)."""
    packs = HERE / "packs.txt"
    if not packs.exists():
        return
    have = set()
    for rec in image_recs + available_recs:
        have.add(rec.get("P"))
        for p in rec.get("p", "").split():
            have.add(p.split("=")[0])
    bad = []
    for line in packs.read_text().splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        name, _, names = [x.strip() for x in line.split("|")]
        bad += [f"{name}: {n}" for n in names.split() if n not in have]
    if bad:
        sys.exit("packs.txt names packages this machine cannot install offline:\n  " + "\n  ".join(bad))
    print(f"    packs.txt: every pack installs offline")


def row(rec, repo):
    origin = rec.get("o") or rec.get("P")
    r = repo.get(rec.get("P"))
    url = f"https://gitlab.alpinelinux.org/alpine/aports/-/tree/{BRANCH}-stable/{r}/{origin}" if r else ""
    return [rec.get("P"), rec.get("V"), rec.get("L", ""), origin, url]


def main():
    fs_json = GUEST / "fs.json"
    if not fs_json.exists():
        sys.exit(f"no built guest at {GUEST} -- run ./build-guest.sh first")
    repo = repo_index()
    fs = json.loads(fs_json.read_text())
    node = find(fs["fsroot"], ["lib", "apk", "db", "installed"])
    if not node:
        sys.exit("the image has no /lib/apk/db/installed")
    image_recs = list(records((GUEST / "blobs" / node[6]).read_text(errors="replace")))
    image = [row(r, repo) for r in image_recs]

    available, available_recs = [], []
    pkg_fs = GUEST / "packages" / "fs.json"
    if pkg_fs.exists():
        p = json.loads(pkg_fs.read_text())
        idx = find(p["fsroot"], ["entity-packages", "x86", "APKINDEX.tar.gz"])
        if not idx:
            sys.exit("the package set has no entity-packages/x86/APKINDEX.tar.gz")
        with tarfile.open(fileobj=io.BytesIO((GUEST / "packages" / "blobs" / idx[6]).read_bytes())) as t:
            text = t.extractfile("APKINDEX").read().decode(errors="replace")
        available_recs = list(records(text))
        available = [row(r, repo) for r in available_recs]
    check_packs(image_recs, available_recs)

    image.sort(key=lambda r: r[0]); available.sort(key=lambda r: r[0])
    unknown = sorted({r[0] for r in image + available if not r[4]})
    out = {"branch": BRANCH, "arch": "x86", "image": image, "available": available,
           "distfiles": f"https://distfiles.alpinelinux.org/distfiles/v{BRANCH}/"}
    (GUEST / "about.json").write_text(json.dumps(out, separators=(",", ":")) + "\n")
    print(f"==> {GUEST / 'about.json'}: {len(image)} image packages, {len(available)} in the package set"
          + (f", {len(unknown)} with no repository found: {unknown[:6]}" if unknown else ""))


if __name__ == "__main__":
    main()
