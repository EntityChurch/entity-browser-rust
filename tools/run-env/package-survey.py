#!/usr/bin/env python3
"""package-survey.py -- price Alpine packages for the package set before adding them.

    ./package-survey.py btop pcc nethack          # what each ADDS to packages.txt
    ./package-survey.py --total                   # what packages.txt costs as a whole

For each name: the dependency closure from Alpine's own APKINDEX (x86, main +
community), MINUS what the built image already ships and MINUS what packages.txt
already pulls in -- so the number is what adding that name costs. Names that do
not exist for x86 say so. Sizes are downloads (S:) and installed (I:).

It is an estimate: provider choice is a heuristic and version constraints are
ignored. It has come out ~5% under build-packages.sh, which is authoritative
(PLAN-2026-09-13 §10). Needs the built guest (for the image's package list) and
curl; the indexes are cached in .survey/.
"""
import json, os, pathlib, re, subprocess, sys, tarfile

HERE = pathlib.Path(__file__).resolve().parent
ALPINE = os.environ.get("ALPINE", "3.21")
CACHE = HERE / ".survey" / ALPINE
PKG, PROV = {}, {}


def load():
    CACHE.mkdir(parents=True, exist_ok=True)
    for repo in ("main", "community"):
        idx = CACHE / f"{repo}.APKINDEX"
        if not idx.exists():
            tgz = CACHE / f"{repo}.tar.gz"
            subprocess.run(["curl", "-sSf", "-o", str(tgz),
                            f"https://dl-cdn.alpinelinux.org/alpine/v{ALPINE}/{repo}/x86/APKINDEX.tar.gz"], check=True)
            with tarfile.open(tgz) as t:
                idx.write_bytes(t.extractfile("APKINDEX").read())
        cur = {}
        for line in idx.read_text(encoding="utf-8", errors="replace").splitlines() + [""]:
            if not line:
                if cur:
                    PKG[cur["P"]] = cur
                cur = {}
            else:
                cur[line[0]] = line[2:]
    for n, p in PKG.items():
        for q in p.get("p", "").split():
            PROV.setdefault(bare(q), []).append(n)


def bare(dep):
    return re.split(r"[<>=~]", dep, maxsplit=1)[0]


def provider(dep):
    dep = bare(dep)
    if dep in PKG:
        return dep
    c = PROV.get(dep, [])
    return sorted(c, key=lambda n: (-int(PKG[n].get("k") or 0), len(n)))[0] if c else None


def closure(names):
    seen, stack = set(), list(names)
    while stack:
        d = stack.pop()
        n = None if d.startswith("!") else provider(d)
        if n and n not in seen:
            seen.add(n)
            stack.extend(PKG[n].get("D", "").split())
    return seen


def mib(names, key="S"):
    return sum(int(PKG[n][key]) for n in names) / 1048576


def image_packages():
    fs = json.load(open(HERE / "alpine-guest/guest/fs.json"))
    nodes = fs["fsroot"]
    for part in ("lib", "apk", "db", "installed"):
        node = next(n for n in nodes if n[0] == part)
        nodes = node[6]
    text = (HERE / "alpine-guest/guest/blobs" / node[6]).read_text(encoding="utf-8", errors="replace")
    return {l[2:].strip() for l in text.splitlines() if l.startswith("P:")}


def main():
    if not (HERE / "alpine-guest/guest/fs.json").exists():
        sys.exit("no built guest -- run ./build-guest.sh first")
    load()
    image = image_packages()
    listed = re.sub(r"#.*", "", (HERE / "packages.txt").read_text()).split()
    have = image | closure(listed)
    if sys.argv[1:] == ["--total"] or not sys.argv[1:]:
        new = closure(listed) - image
        print(f"packages.txt: {len(listed)} names -> {len(new)} packages, "
              f"{mib(new):.1f} MiB download, {mib(new, 'I'):.1f} MiB installed")
        return
    for name in sys.argv[1:]:
        if not provider(name):
            print(f"{name:22} not available for x86 in {ALPINE}")
            continue
        new = sorted(closure([name]) - have)
        desc = PKG[provider(name)].get("T", "")[:60]
        print(f"{name:22} +{len(new):3} pkgs {mib(new):6.2f} MiB down {mib(new, 'I'):6.2f} MiB inst  {desc}")
        deps = [n for n in new if n != name]
        if deps:
            print(f"{'':22}   with: {' '.join(deps[:10])}{' ...' if len(deps) > 10 else ''}")


main()
