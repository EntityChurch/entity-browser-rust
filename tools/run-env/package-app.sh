#!/usr/bin/env bash
# package-app.sh -- package the v86 machines as an entity-app `dist/`.
#
#   ./package-app.sh [outdir]                        default: ./app-dist, every machine that is built
#   ./package-app.sh [outdir] --app alpine           only the named machine(s); repeatable
#   ./package-app.sh <outdir> --onto <dist>          the machines ADDED to an existing entity-apps dist/
#
# THE MACHINES (DESIGN-2026-09-14 §4):
#   alpine   alpine-guest/  Alpine Linux 3.22 on a 9p root   bundles: engine, image, packages?
#   kolibri  kolibri/       KolibriOS from a floppy image    bundles: engine, image
# A machine whose inputs are not built is skipped with the command that builds it,
# unless it was named with --app, where it is an error.
#
# --onto EXISTS BECAUSE `--ingest-apps` IS THE WHOLE APP SET, NOT AN ADDITION.
# A publish replaces every app under {peer}/apps/** with what that one directory
# holds, so publishing ./app-dist alone to a domain that serves entity-apps'
# catalog REMOVES those apps from it. --onto builds one directory holding both:
# the base's files hardlinked in, index.json = the base's entries plus ours (an
# entry with one of our ids is replaced, and it says so). The base is only read.
#
# Produces the shape `publish --ingest-apps` (and `make site APPS_DIST=…`) reads:
#   <out>/index.json            one catalog entry per machine
#   <out>/<id>.html             the page, every local script and stylesheet INLINED
#   <out>/<id>.assets/<bundle>/ the files the page asks the host for by key
#
# WHY SEPARATE BUNDLES: they change for different reasons and on different clocks.
# The engine moves with OUR builds; the image moves when someone rebuilds the
# environment. A republish of one must not re-address the other, and the index-per-
# bundle design (src/apps/assets.rs) means it does not. Two machines each carry an
# `engine` bundle; the store dedupes the bytes by hash.
#
# WHY INLINE THE SCRIPTS: an entity-app bundle is one self-contained HTML file.
# The inliner is the same rule as entity-apps' build.py -- every local
# <script src> and <link rel=stylesheet href>, resolved against the page's own
# directory -- so the shared SDK (vm-sdk/) inlines into each page with no list to
# keep in step. A remote reference is refused: nothing a machine needs may come
# from a third origin.
#
# FILES ARE HARDLINKED, NOT SYMLINKED: `make site` stages APPS_DIST with `cp -r`
# into a container that mounts only this repo, and a symlink to a path outside
# that mount (or an absolute one) arrives dangling. Falls back to a copy.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT_ARG="" ONTO="" APPS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --onto) ONTO="${2:?--onto needs a directory}"; shift 2 ;;
    --onto=*) ONTO="${1#--onto=}"; shift ;;
    --app) APPS+=("${2:?--app needs a name}"); shift 2 ;;
    --app=*) APPS+=("${1#--app=}"); shift ;;
    -*) echo "unknown option $1" >&2; exit 2 ;;
    *) OUT_ARG="$1"; shift ;;
  esac
done
if [ -n "$ONTO" ]; then
  [ -f "$ONTO/index.json" ] || { echo "--onto $ONTO: no index.json -- not an entity-apps dist/" >&2; exit 1; }
  [ -n "$OUT_ARG" ] || { echo "--onto needs an explicit output directory" >&2; exit 2; }
  ONTO="$(cd "$ONTO" && pwd)"
fi
OUT="$(mkdir -p "${OUT_ARG:-$HERE/app-dist}" && cd "${OUT_ARG:-$HERE/app-dist}" && pwd)"
[ "$ONTO" != "$OUT" ] || { echo "--onto and the output are the same directory; the base is read-only" >&2; exit 1; }

exec python3 - "$HERE" "$OUT" "$ONTO" "${APPS[@]+"${APPS[@]}"}" <<'PY'
import json, os, pathlib, re, shutil, sys

here, out, onto = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]), sys.argv[3]
named = sys.argv[4:]
engine = here / "v86-m1"

def engine_bundle():
    return {"v86.wasm": engine / "build/v86.wasm", "seabios.bin": engine / "bios/seabios.bin", "vgabios.bin": engine / "bios/vgabios.bin"}

def alpine():
    g = here / "alpine-guest" / "guest"
    image = {f: g / f for f in ("vmlinuz", "initramfs", "fs.json")}
    optional = {"snapshot.bin.zst": g / "snapshot.bin.zst", "snapshot.json": g / "snapshot.json", "about.json": g / "about.json"}
    bundles = {"engine": {"files": engine_bundle()}, "image": {"files": image, "optional": optional, "dirs": {"blobs": g / "blobs"}}}
    notes = []
    if not (g / "snapshot.bin.zst").exists(): notes.append("no snapshot -- launches boot cold; see build-snapshot.py")
    if not (g / "about.json").exists(): notes.append("no about.json -- the ⓘ package list will say it could not load; run ./build-about.py")
    if (g / "packages" / "fs.json").exists():
        bundles["packages"] = {"files": {"fs.json": g / "packages" / "fs.json"}, "dirs": {"blobs": g / "packages" / "blobs"}}
    else:
        notes.append("no package set -- run ./build-packages.sh to include one")
    return {
        "id": "alpine", "page": here / "alpine-guest" / "index.html",
        "requires": [g / "vmlinuz", g / "fs.json"], "build": "./fetch-engine.sh && ./build-guest.sh",
        "entry": {"id": "alpine", "name": "Alpine Linux",
                  "description": "A real Linux machine in a tab: Alpine 3.22 with bash, on v86. Files move in and out with send and receive; apk add installs tools.",
                  "saves": False, "type": "tool", "category": "developer", "glyph": "🐧",
                  "x-files": True, "x-workspace": True},
        "bundles": bundles, "notes": notes,
    }

def kolibri():
    g = here / "kolibri" / "guest"
    return {
        "id": "kolibri", "page": here / "kolibri" / "index.html",
        "requires": [g / "kolibri.img"], "build": "./fetch-engine.sh && kolibri/fetch-image.sh && kolibri/build-image.sh",
        "entry": {"id": "kolibri", "name": "KolibriOS",
                  "description": "A whole graphical operating system from a 1.44 MB floppy, on v86: a desktop, a file manager, editors and games. Files move in and out through its floppy drive, which is kept.",
                  "saves": False, "type": "tool", "category": "developer", "glyph": "🦅",
                  "x-files": True, "x-workspace": True},
        "bundles": {"engine": {"files": engine_bundle()},
                    "image": {"files": {"kolibri.img": g / "kolibri.img"},
                              "optional": {"snapshot.bin.zst": g / "snapshot.bin.zst", "snapshot.json": g / "snapshot.json"}}},
        "notes": [],
    }

MACHINES = {"alpine": alpine, "kolibri": kolibri}
for n in named:
    if n not in MACHINES: sys.exit(f"unknown machine {n!r} (known: {', '.join(MACHINES)})")
apps = []
for name in (named or list(MACHINES)):
    a = MACHINES[name]()
    missing = [p for p in a["requires"] + [engine / "build/libv86.js", engine / "build/v86.wasm"] if not p.exists()]
    if missing:
        msg = f"{name}: not built ({missing[0]}) -- run {a['build']}"
        if named: sys.exit(msg)
        print(f"    skipped {msg}")
        continue
    apps.append(a)
if not apps: sys.exit("no machine is built; nothing to package")
ours = {a["id"] for a in apps}

def place(src, dst):
    dst.parent.mkdir(parents=True, exist_ok=True)
    try: os.link(src, dst)
    except OSError: shutil.copy2(src, dst)

def place_dir(src, dst):
    for root, _, files in os.walk(src):
        for f in files:
            s = pathlib.Path(root) / f
            place(s, dst / s.relative_to(src))

# The output: emptied only if this script made it (marker) or it is already empty.
marker = out / ".package-app"
if any(out.iterdir()) and not marker.exists():
    stale = [p.name for p in out.iterdir() if not (p.name == "index.json" or re.match(r"^(alpine|kolibri)(\.html|\.assets)$", p.name))]
    if onto or stale:
        sys.exit(f"{out} is not empty and was not made by package-app.sh; refusing to clear it")
for p in out.iterdir():
    shutil.rmtree(p) if p.is_dir() and not p.is_symlink() else p.unlink()
marker.touch()
if onto:
    for p in pathlib.Path(onto).iterdir():
        stem = p.name.split(".")[0]
        if p.name == "index.json" or stem in ours: continue
        (place_dir(p, out / p.name) if p.is_dir() else place(p, out / p.name))

TAG = re.compile(r'<script[^>]*\ssrc="([^"]+)"[^>]*>\s*</script>|<link[^>]*rel="stylesheet"[^>]*href="([^"]+)"[^>]*>')
def inline(page):
    html = page.read_text()
    def sub(m):
        ref = m.group(1) or m.group(2)
        if re.match(r"^(https?:)?//", ref): sys.exit(f"{page}: {ref} is remote -- a machine may not load from a third origin")
        body = (page.parent / ref).resolve().read_text()
        if m.group(1):
            assert "</script" not in body.lower(), f"{ref} contains </script>; cannot inline safely"
            return "<script>\n" + body + "\n</script>"
        return "<style>\n" + body + "\n</style>"
    html, n = TAG.subn(sub, html)
    return html, n

entries = []
for a in apps:
    i = a["id"]
    html, n = inline(a["page"])
    (out / f"{i}.html").write_text(html)
    names = []
    for bname, b in a["bundles"].items():
        d = out / f"{i}.assets" / bname
        for key, src in b.get("files", {}).items(): place(src, d / key)
        for key, src in b.get("optional", {}).items():
            if src.exists(): place(src, d / key)
        for key, src in b.get("dirs", {}).items(): place_dir(src, d / key)
        names.append(bname)
    e = dict(a["entry"]); e["x-assets"] = names
    entries.append(e)
    files = sum(1 for _ in (out / f"{i}.assets").rglob("*") if _.is_file())
    size = sum(p.stat().st_size for p in (out / f"{i}.assets").rglob("*") if p.is_file())
    print(f"==> {i}: {i}.html {len(html.encode()):,} bytes ({n} files inlined); bundles {', '.join(names)}: {files} files, {size / 1048576:.0f} MiB")
    for note in a["notes"]: print(f"    {note}")

catalog = entries
if onto:
    base = json.loads((pathlib.Path(onto) / "index.json").read_text())
    assert isinstance(base, list), "the base index.json must be a JSON array"
    clash = sorted(ours & {e.get("id") for e in base})
    if clash: print(f"    base already has {clash} -- replaced by ours")
    catalog = [e for e in base if e.get("id") not in ours] + entries
    print(f"    catalog: {len(base)} from the base + {len(entries)} = {len(catalog)} entries")
(out / "index.json").write_text(json.dumps(catalog, indent=2, ensure_ascii=False) + "\n")
print(f"==> {out}")
print(f"    publish it:    make site-dist APPS_DIST={out}   (then make serve DIST=dist-site, open Apps)")
PY
