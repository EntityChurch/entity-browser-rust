#!/usr/bin/env python3
"""fs2json.py -- turn a directory tree into v86's 9p filesystem index + a
content-addressed blob store.

    ./fs2json.py <root-dir> <out-dir>
      -> <out-dir>/fs.json          the index  (v86 `filesystem.basefs`)
      -> <out-dir>/blobs/<sha256>   file bodies (v86 `filesystem.baseurl`)

FORMAT, READ OUT OF THE ENGINE, not out of a README -- `N.prototype.LoadRecursive`
in libv86.js. A node is an ARRAY, not an object:

    [ name, size, mtime, mode, uid, gid, extra ]

and the low nibble-pair of `mode` selects what `extra` means:

    mode & 0xF000 == 0x4000  directory  -> extra = [child, child, ...]
    mode & 0xF000 == 0x8000  regular    -> extra = sha256 hex   (status = 2)
    mode & 0xF000 == 0xA000  symlink    -> extra = target string
    mode & 0xF000 == 0xC000  socket     -> skipped by the engine
    anything else                       -> the engine calls its assert hook

Top level is { "version": 3, "size": <bytes>, "fsroot": [...] }; version MUST be
3 or `load_from_json` throws.

*** THE FINDING THIS TOOL EXISTS TO EXPLOIT ***
A file body is fetched as `baseurl + sha256hex` -- flat, and named by the SHA-256
of its contents. v86's image format is not merely *like* a content-addressed
store, it IS one, and it is faulted lazily: nothing is fetched because it is in
the image, only because the guest read it. That is why the entity content store
can back this directly (one `handle9p`, or one URL rewrite) instead of us
building a second blob store beside it.

We write our own rather than vendoring v86's `tools/fs2json.py` for two reasons:
the format came from the engine so there is nothing to trust, and this is the
code that will eventually emit entity paths instead of a blobs/ directory.
"""
import hashlib
import json
import os
import shutil
import stat
import sys


def build(path, out_blobs, stats):
    """Return a v86 node array for `path`, or None if it cannot be represented."""
    st = os.lstat(path)
    mode, name = st.st_mode, os.path.basename(path)
    # mtime: v86 wants seconds. A build must be reproducible, so honour
    # SOURCE_DATE_EPOCH when it is set -- otherwise two builds of one tree
    # differ in the index and every consumer re-fetches nothing but sees a
    # different fs.json hash.
    mtime = int(os.environ.get("SOURCE_DATE_EPOCH", st.st_mtime))
    node = [name, st.st_size, mtime, mode, st.st_uid, st.st_gid]

    if stat.S_ISDIR(mode):
        children = []
        for entry in sorted(os.listdir(path)):          # sorted => deterministic
            child = build(os.path.join(path, entry), out_blobs, stats)
            if child is not None:
                children.append(child)
        node.append(children)
        stats["dirs"] += 1
        return node

    if stat.S_ISREG(mode):
        h = hashlib.sha256()
        with open(path, "rb") as fh:
            for chunk in iter(lambda: fh.read(1 << 20), b""):
                h.update(chunk)
        digest = h.hexdigest()
        blob = os.path.join(out_blobs, digest)
        if not os.path.exists(blob):                    # content-addressed => dedup is free
            shutil.copyfile(path, blob)
            stats["blobs"] += 1
            stats["blob_bytes"] += st.st_size
        else:
            stats["deduped"] += 1
            stats["deduped_bytes"] += st.st_size
        node.append(digest)
        stats["files"] += 1
        stats["bytes"] += st.st_size
        return node

    if stat.S_ISLNK(mode):
        node.append(os.readlink(path))
        stats["links"] += 1
        return node

    # Sockets the engine skips on its own; char/block/fifo it would assert on,
    # so drop them here and SAY SO rather than emitting a node that trips the
    # engine's assert hook at mount time. /dev is devtmpfs in the guest anyway.
    stats["skipped"].append(os.path.relpath(path, stats["root"]))
    return None


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    root, out = os.path.abspath(sys.argv[1]), os.path.abspath(sys.argv[2])
    blobs = os.path.join(out, "blobs")
    os.makedirs(blobs, exist_ok=True)

    stats = {"dirs": 0, "files": 0, "links": 0, "blobs": 0, "deduped": 0,
             "bytes": 0, "blob_bytes": 0, "deduped_bytes": 0,
             "skipped": [], "root": root}

    fsroot = [build(os.path.join(root, e), blobs, stats)
              for e in sorted(os.listdir(root))]
    fsroot = [n for n in fsroot if n is not None]

    with open(os.path.join(out, "fs.json"), "w") as fh:
        json.dump({"version": 3, "size": stats["bytes"], "fsroot": fsroot},
                  fh, separators=(",", ":"))

    idx = os.path.getsize(os.path.join(out, "fs.json"))
    mb = lambda n: f"{n / (1 << 20):.1f} MiB"
    print(f"  index      {idx:>10,} bytes  (fs.json)")
    print(f"  dirs       {stats['dirs']:>10,}")
    print(f"  files      {stats['files']:>10,}   {mb(stats['bytes'])}")
    print(f"  symlinks   {stats['links']:>10,}")
    print(f"  blobs      {stats['blobs']:>10,}   {mb(stats['blob_bytes'])} on disk")
    print(f"  deduped    {stats['deduped']:>10,}   {mb(stats['deduped_bytes'])} saved")
    if stats["skipped"]:
        print(f"  SKIPPED    {len(stats['skipped']):>10,}   (device/fifo nodes, "
              f"not representable): {', '.join(stats['skipped'][:6])}"
              + (" …" if len(stats["skipped"]) > 6 else ""))


if __name__ == "__main__":
    main()
