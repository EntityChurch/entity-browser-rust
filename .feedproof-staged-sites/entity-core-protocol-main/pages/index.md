+++
title = "Entity Core Protocol"
content_class = "authored"
source = "content/entity-core-protocol/index.md (via stage0 scrub)"
+++

# Entity Core Protocol

The Entity Core Protocol is a working specification for distributed information systems. Its surface is small:

- **Content-addressed typed data** as the basic unit. Every value is `{type, data}`; the unit's identity is the hash of its canonical encoding.
- **A mutable named tree over an immutable content store.** The content store maps `hash → entity`; the tree maps `path → hash`. Paths are mutable; content is not.
- **Universal addressing.** Every entity has a name, a location, and an address: `peer/path` resolves to a content hash in some peer's tree. The namespace is peer-isolated by construction, so trees from different peers compose without conflict; identical content shared across peers deduplicates automatically. Whether a given entity is reachable from where you sit is a separate question --- capability-gated, network-dependent --- but addressability itself is unconditional.
- **Two message types.** `EXECUTE` carries a typed operation against a path; `EXECUTE_RESPONSE` returns the result. There is no separate query, subscribe, or update message --- they are all `EXECUTE` against handlers registered in the tree.
- **Four-dimensional capability grants** (handlers, operations, resources, peers) with cryptographic attenuation. Capabilities are themselves entities, content-addressed and verifiable independently of session.
- **Peer-to-peer communication.** Each peer has content-derived identity, hosts its own tree, and exchanges entities with other peers under capability constraints.

The core protocol has been re-derived across more than forty languages (OCaml,
Swift, Haskell, Lean, Rust, Go, TypeScript, Elixir, Dart, Forth, Smalltalk, APL,
SQL, … ), generated from the spec — one peer per language — alongside three
independently-written reference implementations (Go, Rust, Python). The claim is
*spec-forced convergence* — the same specification re-derived across languages —
**not** that many separate codebases independently agree; the generated peers
share a generation lineage.

Conformance is tracked, not claimed once. Every verdict is pinned to a specific
oracle commit, and re-measurement against a newer oracle supersedes it rather
than preserving it — so a peer's status is a current reading, not a badge it
keeps. Peers that do not pass are published with a named defect rather than
omitted. The current tally and the per-peer detail behind it (spec version,
oracle commit, codec, crypto floor, known gaps) live in the
[conformance matrix](keystone/conformance-matrix.md), which is the authoritative
number and is re-measured rather than restated.

## Read the specification

- **[The Specification](spec/index.md)** — the complete normative spec and its companions: the protocol, the wire format, the type system, the core tree operations, and the machine boundary.
- **[Keystone](keystone/index.md)** — the generator that derives a conforming peer from this spec in any language, and the published gate that says which peers pass.
- **[Formalization](formalization.md)** — the machine-checked models behind the design.

The reference implementations and applications live with the [Entity Church Foundation](https://entitychurchfoundation.org).
