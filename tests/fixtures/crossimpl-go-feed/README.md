# `crossimpl-go-feed` — corridor ①, cut both ways

**These bytes are not ours.** They are the byte-for-byte output of
`entity-workbench-go`'s `publish/cmd/crossimpl-feed`, vendored so the corridor
gate runs in `make test` with no Go toolchain on the host.

| | |
|---|---|
| produced by | `entity-workbench-go` `dev` @ `779616d` |
| delivered in | their `ROUTING-2026-09-15-c-entity-browser-rust-corridor-one-is-cut-both-ways-…` |
| read by | `src/crossimpl_feed.rs` (four gates, `make test`) |
| publisher | `2KAoCfAP6ZZyLmS9wYz4rUmpehd4JMeek32NLN58R3ehpi` |

Re-cut — deterministic, so a copy can be checked rather than trusted:

```sh
cd ../entity-workbench-go && go run ./publish/cmd/crossimpl-feed -out /tmp/corridor
diff -r /tmp/corridor tests/fixtures/crossimpl-go-feed   # README.md aside
```

## Two cuts, because one measures neither rule

| cut | declared prefix | bindings | entities | entry signatures committed |
|---|---|---|---|---|
| `peer-root/` | `/` | 438 | 459 | **34 / 34** |
| `feed-only/` | `app/feed/` | 37 | 42 | **0 / 34** |

The pair is **`FEED-14`** (arch's `A-36` proposal §4.5): a feed published over a
prefix that excludes the signature location, read by a static reader with no
second channel, must yield **zero attributed entries** — with the same feed
published over the peer root yielding **every** entry attributed as the
anti-vacuity arm.

⚠ **Neither cut is a recommended shape.** `peer-root/` is the `A-36` fix
measured, and measuring it is what produced `entity-workbench-go`'s `A-38`: the
widening that commits the signatures also commits that peer's device
declarations, folder declarations with local filesystem paths, and capability
policy table. Their product refuses that prefix without an explicit
acknowledgement. This fixture peer holds a feed and nothing private, which is
the case the guard is not for.

## Why 34 entries

Against their pinned **32**-entry page size, so the oldest entry lives on an
index page the head's `current` does not name. Every feed fixture on either seat
before this one was a single page, which left `FEED-R12` unfalsifiable on both
simultaneously.

## ⚠ This is a wire artifact

Regenerating it is a **wire event**, not a test fix. If these bytes move without
a deliberate change on their side, either their encoder moved or ours did, and
the question is which. Entry `created_at` is inside the hashed bytes, so an
unpinned clock on their side moves all 34 entry hashes *and* all 34 signature
paths — they pin the instant for exactly this reason.

## What a green run does and does not establish

**Does:** their publisher and our consumer agree end to end — manifest, root
signature, trie walk, §4.2 index across a page boundary, two-hop entry bodies,
and `FEED-R4`'s per-entry verdict. Per `[ADR-0012]` this is a different class of
evidence from `J-4`, which compared two **encoders**: two producers agreeing is
not evidence that a reader is right.

**Does not:** anything about the live road, about two peers, or about a
publisher that changes between reads.
