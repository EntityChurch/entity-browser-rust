#!/usr/bin/env bash
# core-pin — MATERIALIZE the sibling kernel at a named commit, so a build stops
#            depending on what another seat happens to have checked out.
#
# `build-pair.sh` made the pair OBSERVABLE and then REFUSABLE: it stamps
# `(our commit, entity-core-rust commit)` into the shell, and `CORE_RUST_REF`
# refuses a build whose sibling is not sitting on the ref you asked for. That is
# detection. It leaves the one case that actually bites on this box untouched:
# **we share a machine, and the other seat is often working in
# `entity-core-rust` right now.** Detection can only tell you to come back later.
#
# Worse, and nobody had written it down: `--check` runs BEFORE the build. A
# commit landing in the sibling mid-`make wasm` produces a mixed artifact with a
# green guard in front of it. Verification cannot close a race it runs before.
#
# **THE PIN POINT IS THE CONTAINER MOUNT, NOT THE SOURCE TREE.** Every
# containerized verb bind-mounts the parent meta dir at `/src/entity-systems`, so
# the thirty-three path deps (twenty-five in `Cargo.toml`, eight in
# `src-tauri/Cargo.toml`) resolve through `/src/entity-systems/entity-core-rust`.
# A SECOND `-v` over that subpath overlays it — verified, not assumed:
#
#     podman run -v /tmp/parent:/src -v /tmp/pin:/src/sib alpine cat /src/sib/who.txt
#     → PINNED
#
# So pinning costs **zero `Cargo.toml` edits, zero symlinks, and zero writes to
# the sibling's git**. Cargo is never told anything; it resolves the same paths
# it always did and finds different bytes there.
#
# **`git archive`, deliberately NOT `git worktree add`.** A worktree registers
# itself in the SIBLING's `.git/worktrees/`, which is a mutation of a repo that
# is not ours — AGENTS-STANDARD is explicit that a sibling's git is read-only
# from here — and it leaks entries into the other seat's `git worktree list`.
# That is not theoretical: `entity-core-rust` carries two stale `/tmp` worktrees
# marked `prunable` today, from sessions that are long over. `git archive` reads
# and never writes, and everything it produces lands in ONE cache directory,
# which `make core-pin-clean` empties.
#
# **That directory is OUTSIDE our tree** (`${XDG_CACHE_HOME:-~/.cache}/entity-browser-core-pin`,
# override with `CORE_PIN_ROOT`), and it used to be `.core-pin/` inside it. Moved
# 2026-09-15 because a whole second repository inside ours is read as OURS by
# every tool that walks the tree: `tools/vocab-lint.sh`'s analyzer globbed
# `*.rs` into `.core-pin/<sha>/extensions/query/` and reported `single-seat
# app/user` against this seat — a tag the kernel uses and we do not. gitignore
# hides a directory from git, not from `rglob`. The container mount is its own
# `-v`, so the build never needed the pin inside the parent mount.
#
# **The pin is keyed by resolved COMMIT, never by the ref you typed.** Passing
# `CORE_RUST_REF=dev` pins to the commit `dev` names *at the moment you ask*; if
# `dev` moves, that is a different commit and therefore a different pin. Reuse is
# then automatic and can never be wrong, because the directory name IS the
# identity of its contents.
#
# **Extraction is atomic.** A `podman` interrupted mid-`tar` would otherwise
# leave a half-tree that looks materialized and builds nonsense; we extract into
# a scratch dir and `mv` it into place, so a pin directory either does not exist
# or is complete.
#
# Usage:
#   core-pin.sh --resolve   → print the full sha CORE_RUST_REF names (no I/O)
#   core-pin.sh --ensure    → materialize if needed; print the host path
#   core-pin.sh --prune     → remove every materialized pin
#   core-pin.sh             → say what is pinned and what is on disk
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/.." && pwd)"
# Same rule as `build-pair.sh`: resolve from THIS SCRIPT's location, never from
# `cwd`. The sibling sits beside the repo root in the host checkout and in the
# build image's bind mount alike, so `<script>/../..` is a stable answer where
# `../entity-core-rust` is a claim about who invoked us.
sibling="$(cd "$root/.." 2>/dev/null && pwd)/entity-core-rust"
pin_root="${CORE_PIN_ROOT:-${XDG_CACHE_HOME:-$HOME/.cache}/entity-browser-core-pin}"

say() { printf '%s\n' "$1" >&2; }

resolve() {
    local ref="${CORE_RUST_REF:-}" sha
    if [ -z "$ref" ]; then
        say "core-pin: CORE_RUST_REF is not set — nothing to pin."
        return 1
    fi
    if [ ! -d "$sibling/.git" ] && ! git -C "$sibling" rev-parse --git-dir >/dev/null 2>&1; then
        say "core-pin: REFUSED — $sibling is not a git checkout."
        say "          A pin is a commit, and there is no repository here to name one in."
        return 1
    fi
    # `^{commit}` so a tag or a branch resolves to the commit it points at and a
    # tree-ish that is not a commit is refused rather than silently accepted.
    sha=$(git -C "$sibling" rev-parse --verify "${ref}^{commit}" 2>/dev/null || true)
    if [ -z "$sha" ]; then
        say "core-pin: REFUSED — CORE_RUST_REF=$ref does not resolve in entity-core-rust."
        say "          Fetch it there first. This script never mutates a sibling checkout."
        return 1
    fi
    printf '%s' "$sha"
}

case "${1:-}" in
--resolve)
    resolve
    printf '\n'
    ;;

--ensure)
    sha=$(resolve)
    dest="$pin_root/$sha"
    if [ -d "$dest" ]; then
        say "core-pin: reusing $sha (already materialized)"
        printf '%s\n' "$dest"
        exit 0
    fi
    mkdir -p "$pin_root"
    # Scratch dir beside the destination so the `mv` is a rename within one
    # filesystem — an atomic publish, not a copy that can be interrupted.
    scratch="$pin_root/.staging-$sha.$$"
    rm -rf "$scratch"
    mkdir -p "$scratch"
    # No submodules and no `export-ignore` attributes in this repo (checked), so
    # `git archive` of a commit is a faithful tree. It reads the object store and
    # never touches the working tree, so the other seat can be mid-edit, mid-
    # rebase, or mid-anything, and this still produces exactly that commit.
    if ! git -C "$sibling" archive --format=tar "$sha" | tar -x -C "$scratch"; then
        rm -rf "$scratch"
        say "core-pin: REFUSED — could not export $sha from entity-core-rust."
        exit 1
    fi
    mv "$scratch" "$dest"
    say "core-pin: materialized entity-core-rust @ ${sha:0:7} → $pin_root/${sha:0:7}…"
    printf '%s\n' "$dest"
    ;;

--prune)
    if [ ! -d "$pin_root" ]; then
        say "core-pin: nothing materialized."
        exit 0
    fi
    n=$(find "$pin_root" -mindepth 1 -maxdepth 1 -type d | wc -l)
    rm -rf "$pin_root"
    say "core-pin: removed $n materialized pin(s)."
    ;;

"")
    if [ -n "${CORE_RUST_REF:-}" ]; then
        sha=$(resolve) || exit 1
        say "requested: CORE_RUST_REF=$CORE_RUST_REF → ${sha:0:7}"
        [ -d "$pin_root/$sha" ] && say "           materialized" || say "           not yet materialized"
    else
        say "requested: (none) — builds use the live sibling checkout"
    fi
    if [ -d "$pin_root" ]; then
        say "on disk:"
        for d in "$pin_root"/*/; do
            [ -d "$d" ] || continue
            say "  $(basename "$d" | cut -c1-7)  $(du -sh "$d" 2>/dev/null | cut -f1)"
        done
    fi
    ;;

*)
    say "usage: core-pin.sh [--resolve|--ensure|--prune]"
    exit 2
    ;;
esac
