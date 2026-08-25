# `registry-federation` — the committed cross-impl test vectors

A full local federation as `entity-browser` emits it: four content domains, each published
under its own key, plus a **registry** that binds one name to each. `MAPPING.txt` names the
peer-ids; the registry peer-id is the only string a consumer holds a priori.

## Why these bytes are in git

They were not, and that was the defect. `dist-federation/` is gitignored, so **no commit in
any repo contained the corpus** — while `entity-workbench-go`'s fixture is a *copy* of it.
Their own fixture README said so, and named the consequence: the revision it cited *"names
the tree they were cut beside rather than one that holds them."*

A cross-impl conformance claim pinned to something other than the bytes is not reproducible,
and this ecosystem has already paid for that once (the compute corpus drifting 330 → 343
undetected). Arch offered a tag, a committed fixture directory, or a release artifact
(`ROUTING-2026-08-21-f` §2 item 4). This is the committed fixture — the only one of the three
that pins the bytes *and* stays regenerable.

## Regenerating

```bash
make federation-vectors
```

The clock is pinned (`ISSUED_AT_MS`), because `issued_at` rides every binding body and so
decides every binding hash and the whole trie shape. Unpinned, two runs over identical
content emit different bytes and this directory could neither be regenerated nor diffed.

**A diff after regeneration means the emitter changed** — which is the point. Read it before
you commit it.

### The one thing that is *not* byte-stable, and why

`system/peer/published-root` and its `system/signature/{hex}` differ on every emission, in
every subtree. Measured, not assumed: regenerating changes **40 of 285 files' worth of
lines**, and all of them are those two artifacts plus their content blobs. Everything else —
every binding, every trie node, every transport profile, every page — is identical.

The cause is upstream and not ours to pin: `PublishedRootData.published_at` is a wall-clock
millisecond stamp written inside `Peer::publish_root` (`core/peer`, `core/types/src/lib.rs`).
So a consumer diffing this corpus should expect exactly those two files per peer to move, and
should treat a change **anywhere else** as a real emitter change.

## What a consumer should read

- **The registry's signed root** is the entry point. `binding/by-name/{name}` → a binding.
- **`transports` carries bare `system/hash`** naming `system/peer/transport/*` entities
  (`EXTENSION-REGISTRY` v1.21 D8). It is **not** an inline map — a decoder that accepts both
  is what `REG-BINDING-TRANSPORTS-SHAPE-1` row (b) exists to fail.
- **Those profile entities are served here** (D8a), both by hash out of the content closure
  and by path at `system/peer/transport/{target}/primary.bin`, and the signed root commits to
  them. `REG-PUBLISH-CLOSURE-1`'s discriminating row is enumeration-succeeds-then-resolve-fails;
  a registry publishing only `by-name/` passes the first and fails the second.
- **`…/by-name.list` is a `system/tree/listing` wire entity** in ECF, not newline text
  (`EXTENSION-NETWORK` §6.5.3.1 — *"No JSON form."*). Its `entries` map is canonical CBOR, so
  key order is **length-first**, not alphabetical. It is transport-trusted and commits to
  nothing: every name it offers must still be resolved through the signed root.

## Serving it

```bash
./tools/cors-serve.py tests/fixtures/registry-federation 8099
```

Use `cors-serve.py`, not `python3 -m http.server` — the latter sends no CORS headers, so a
cross-origin fetch fails in a browser while `curl` is perfectly happy.
