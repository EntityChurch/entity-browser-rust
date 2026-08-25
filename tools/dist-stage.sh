#!/bin/sh
# dist-stage.sh — collect Tauri's bundler output into artifacts/ under the
# fleet-wide release filename scheme.
#
#   usage: dist-stage.sh <name> <version> <os> <arch> <bundle-root> <out-dir>
#
# ADR-0023 fixes the scheme fleet-wide as `{name}_{version}_{os}_{arch}`, so a
# human (and a download page) can read platform off the filename without
# knowing each packager's local convention. Tauri does NOT emit that shape —
# every bundler names its output its own way, using its own arch vocabulary:
#
#   Entity Browser_0.8.0_amd64.deb        (Debian:   amd64)
#   Entity-Browser-0.8.0-1.x86_64.rpm     (RPM:      x86_64)
#   entity-browser-tauri_0.8.0_amd64.AppImage
#   Entity Browser_0.8.0_aarch64.dmg      (Apple:    aarch64/x64)
#   Entity Browser_0.8.0_x64_en-US.msi    (WiX:      x64 + a LOCALE)
#
# Three of those spell the same architecture three ways, one carries a locale,
# and several carry a SPACE — which makes them miserable to `curl` and to
# checksum. So we rename rather than publish what the bundler happened to call
# it. We copy (not move): the bundler's output stays where it was for anyone
# debugging a build, and re-running the target is idempotent.
#
# Deliberately dumb about which files exist: it stages whatever bundles were
# actually produced and fails only if NOTHING was. `make dist` decides which
# bundles to ask for (DIST_BUNDLES); this script does not second-guess it, so
# adding a bundle format is a one-word change there and needs nothing here.
set -eu

if [ $# -ne 6 ]; then
    echo "usage: $0 <name> <version> <os> <arch> <bundle-root> <out-dir>" >&2
    exit 2
fi

name=$1
version=$2
os=$3
arch=$4
bundle_root=$5
out_dir=$6

if [ ! -d "$bundle_root" ]; then
    echo "dist-stage: no bundle output at '$bundle_root'" >&2
    echo "  the packaging step did not run, or wrote somewhere else" >&2
    exit 1
fi

mkdir -p "$out_dir"
staged=0

# Match on EXTENSION, not on the bundler's filename. `.app` is a directory
# (macOS), so it is tarred rather than copied — a bare directory is not a
# downloadable artifact.
#
# `find … -print` into a while-read loop, not `for f in $(...)`: these paths
# contain spaces by default ("Entity Browser_0.8.0_amd64.deb").
for ext in deb rpm AppImage dmg msi exe; do
    find "$bundle_root" -type f -name "*.${ext}" -print | while IFS= read -r src; do
        # NSIS emits the installer as a bare .exe, which is indistinguishable
        # from a plain executable at a glance — so it gets `_setup.exe`, the
        # same thing Tauri's own name says ("…_x64-setup.exe"). Everything else
        # keeps its extension as-is.
        if [ "$ext" = "exe" ]; then
            dest="${out_dir}/${name}_${version}_${os}_${arch}_setup.exe"
        else
            dest="${out_dir}/${name}_${version}_${os}_${arch}.${ext}"
        fi
        cp -f "$src" "$dest"
        echo "  staged  $(basename "$src")  ->  $(basename "$dest")"
    done
done

find "$bundle_root" -maxdepth 2 -type d -name "*.app" -print | while IFS= read -r src; do
    dest="${out_dir}/${name}_${version}_${os}_${arch}.app.tar.gz"
    tar -czf "$dest" -C "$(dirname "$src")" "$(basename "$src")"
    echo "  staged  $(basename "$src")  ->  $(basename "$dest")"
done

# The loops above run in subshells (pipeline), so a counter incremented inside
# them would be lost — count the result on disk instead.
staged=$(find "$out_dir" -maxdepth 1 -type f -name "${name}_${version}_${os}_${arch}*" | wc -l)
if [ "$staged" -eq 0 ]; then
    echo "dist-stage: found no installable artifacts under '$bundle_root'" >&2
    echo "  looked for: .deb .rpm .AppImage .dmg .msi .exe .app" >&2
    exit 1
fi

echo ""
echo "=== $staged artifact(s) → ${out_dir}/ ==="
find "$out_dir" -maxdepth 1 -type f -name "${name}_${version}_${os}_${arch}*" \
    -exec ls -lh {} \; | awk '{print "  " $NF "  (" $5 ")"}'
