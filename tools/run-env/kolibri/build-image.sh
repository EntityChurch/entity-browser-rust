#!/usr/bin/env bash
# build-image.sh -- the floppy this app boots: KolibriOS's pinned image plus ONE
# driver of ours, VMMOUSE.SYS, and the autorun line that loads it.
#
#   ./fetch-image.sh && ./build-image.sh      # -> guest/kolibri.img
#
# Why a driver: without it the guest's pointer is a PS/2 mouse, relative and
# accelerated, and it drifts away from the host's pointer (vmmouse/vmmouse.asm
# says the whole of it). With it, the guest pointer IS the host pointer.
#
# Everything it fetches is pinned by sha256 -- fasm's release and the five
# KolibriOS driver headers, taken at the commit the image was built from. A
# mismatch is a result, not something to update past.
#
# What it changes in the upstream image, and nothing else:
#   + DRIVERS/VMMOUSE.SYS
#   ~ SETTINGS/AUTORUN.DAT  gains "/SYS/LOADDRV VMMOUSE 0" as its first program
#   ~ SETTINGS/ICON.INI     loses the desktop icons for programs this build lacks
# The driver refuses to load on a machine with no VMware interface, so the image
# still boots the same everywhere else.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
CACHE="$HERE/.build"
UP="$HERE/guest/kolibri-upstream.img"
OUT="$HERE/guest/kolibri.img"
mkdir -p "$CACHE"

KOS_COMMIT="b0055ba4721a68ed6352e4eeae6808ed4f813784"
FASM_URL="https://flatassembler.net/fasm-1.73.35.tgz"
FASM_SHA="a34dec7d0bc2dc79faabb68bd8bc2f62b6cfb31d69c01449367ce4cd8098934e"
declare -A INC_SHA=(
  [proc32.inc]=23b449679478ef12f5d957a90185994fbfdb5855431c1017ead5727a903ca3a3
  [struct.inc]=e4c735edf376f4ef2a429a848bfdf160702faba5ff9890a108bceec8a4ecc39a
  [macros.inc]=05f378cd5c145789e63c8e4496ec88a0395723d8f288b80a4f8a7415676dac83
  [kglobals.inc]=b4226f7e7511ea3cc3b43195ebd2ac46d1a6d4c4e9c6ddab5bb22ee29303c130
  [peimport.inc]=920f485b3109db12d860ae5d8d86cf0b40e122d742f82029048f3dde6e17b0de
)

fetch() { # url dest sha
  if [ ! -f "$2" ]; then curl -fsSL -o "$2.part" "$1" && mv "$2.part" "$2"; fi
  local got; got=$(sha256sum "$2" | cut -d' ' -f1)
  [ "$got" = "$3" ] || { printf "  MISMATCH %s\n     pinned %s\n     got    %s\n" "$1" "$3" "$got"; exit 1; }
}

[ -f "$UP" ] || { echo "  MISSING  guest/kolibri-upstream.img -- run ./fetch-image.sh"; exit 1; }
fetch "$FASM_URL" "$CACHE/fasm.tgz" "$FASM_SHA"
for f in "${!INC_SHA[@]}"; do
  fetch "https://git.kolibrios.org/KolibriOS/kolibrios/raw/commit/$KOS_COMMIT/drivers/$f" "$CACHE/$f" "${INC_SHA[$f]}"
done
cp "$HERE/vmmouse/vmmouse.asm" "$CACHE/vmmouse.asm"
cp "$UP" "$CACHE/kolibri.img"

# fasm is a static i386 binary; mtools edits the FAT image without mounting it.
# --security-opt label=disable for the same reason as build-guest.sh (a shared parent tree; no relabelling).
run() { podman run --rm --security-opt label=disable -v "$CACHE:/w" -w /w docker.io/library/alpine:3.21 sh -euc "$1"; }
run 'tar xzf fasm.tgz && ./fasm/fasm vmmouse.asm VMMOUSE.SYS >/dev/null'

# fasm stamps the assembly time into the PE header. Pin it to a fixed date
# and recompute the header checksum, so two builds of this image are the same bytes.
python3 - "$CACHE/VMMOUSE.SYS" <<'PY'
import struct, sys
p = sys.argv[1]; b = bytearray(open(p, 'rb').read())
pe = struct.unpack_from('<I', b, 0x3C)[0]
assert b[pe:pe + 4] == b'PE\0\0', 'not a PE file'
struct.pack_into('<I', b, pe + 8, 1788307200)          # 2026-09-01T00:00:00Z
cs = pe + 24 + 64                                      # OptionalHeader.CheckSum
struct.pack_into('<I', b, cs, 0)
total = 0
for i in range(0, len(b) - len(b) % 2, 2):
    total += b[i] | b[i + 1] << 8
    total = (total & 0xFFFF) + (total >> 16)
if len(b) % 2: total += b[-1]; total = (total & 0xFFFF) + (total >> 16)
struct.pack_into('<I', b, cs, ((total & 0xFFFF) + (total >> 16) & 0xFFFF) + len(b))
open(p, 'wb').write(b)
PY

MT='apk add -q mtools >/dev/null; printf "mtools_skip_check=1\n" > /tmp/mtoolsrc; export MTOOLSRC=/tmp/mtoolsrc'
run "$MT"'
  mcopy -o -i kolibri.img ::SETTINGS/AUTORUN.DAT autorun.orig
  mcopy -o -i kolibri.img ::SETTINGS/ICON.INI icon.orig
  mdir -/ -b -i kolibri.img :: > files.txt
'

# The two settings edits, with the checks that make them results rather than hopes.
python3 - "$CACHE" <<'PY'
import re, sys
w = sys.argv[1]
nl = lambda b: "\r\n" if b"\r\n" in b else "\n"

# AUTORUN.DAT: load the driver first, before SETUP and the taskbar, so the pointer
# is right from the first frame.
raw = open(f"{w}/autorun.orig", "rb").read(); eol = nl(raw)
lines = raw.decode("latin-1").split(eol)
first = next(i for i, l in enumerate(lines) if l.startswith("/SYS/"))
lines.insert(first, "/SYS/LOADDRV           VMMOUSE 0\t# absolute pointer (VMware interface; entity run-env)")
open(f"{w}/AUTORUN.DAT", "wb").write(eol.join(lines).encode("latin-1"))

# ICON.INI: this upstream build puts icons on the desktop for two programs it does
# not contain -- not on the floppy, and not on the 98.6 MB CD image either (checked
# 2026-09-14). Double-clicking one says "Error running program" (field report). Drop
# every icon whose program is not in the image, renumbering so the sections stay
# consecutive. A DIFFERENT dead set is a result: upstream changed, look at it.
EXPECTED_DEAD = {"XONIX", "KOSILKA"}
present = {l.strip().removeprefix("::").upper() for l in open(f"{w}/files.txt", encoding="latin-1") if l.strip()}
raw = open(f"{w}/icon.orig", "rb").read(); eol = nl(raw)
text = raw.decode("latin-1")
parts = re.split(r"(?m)^(?=\[)", text)
head = [p for p in parts if not re.match(r"\[[0-9A-F]{2}\]", p)]
icons = [p for p in parts if re.match(r"\[[0-9A-F]{2}\]", p)]
kept, dead = [], set()
for sec in icons:
    path = re.search(r"(?m)^path=(.*?)\r?$", sec).group(1).strip()
    full = path if path.startswith("/") else "/SYS/" + path
    if full.upper().removeprefix("/SYS") in present:
        kept.append(sec)
    else:
        dead.add(re.search(r"(?m)^name=(.*?)\r?$", sec).group(1).strip())
if dead != EXPECTED_DEAD:
    sys.exit(f"  ICON.INI: dead icons are {sorted(dead)}, expected {sorted(EXPECTED_DEAD)} -- upstream changed")
kept = [re.sub(r"^\[[0-9A-F]{2}\]", f"[{i:02X}]", sec) for i, sec in enumerate(kept)]
open(f"{w}/ICON.INI", "wb").write(("".join(head) + "".join(kept)).encode("latin-1"))
print(f"  icons    {len(kept)} kept, dropped {', '.join(sorted(dead))} (not in this build)")
PY

run "$MT"'
  # Fixed timestamps and -m, so two builds are the same bytes.
  touch -d "2026-09-14 00:00:00" VMMOUSE.SYS AUTORUN.DAT ICON.INI
  mcopy -m -o -i kolibri.img VMMOUSE.SYS ::DRIVERS/VMMOUSE.SYS
  mcopy -m -o -i kolibri.img AUTORUN.DAT ::SETTINGS/AUTORUN.DAT
  mcopy -m -o -i kolibri.img ICON.INI ::SETTINGS/ICON.INI
'
grep -q VMMOUSE "$CACHE/AUTORUN.DAT" || { echo "  autorun line not added"; exit 1; }
[ "$(stat -c %s "$CACHE/kolibri.img")" = 1474560 ] || { echo "  image is not 1.44 MB"; exit 1; }
cp "$CACHE/kolibri.img" "$OUT"
echo "  ok       guest/kolibri.img  VMMOUSE.SYS $(stat -c %s "$CACHE/VMMOUSE.SYS") bytes  sha256 $(sha256sum "$OUT" | cut -c1-16)…"
