+++
title = "Keystone"
content_class = "authored"
source = "content/entity-core-protocol/keystone/index.md (via stage0 scrub)"
+++

# Keystone

A specification is a claim about what can be built from it. Keystone is how that
claim gets tested: a generator that emits a complete core-protocol peer, in any
target language, from the published specification — and a gate that says, per
peer, whether the result actually conforms.

The result is a cohort of core peers spanning functional, imperative, array,
stack, logic and declarative-query substrates — OCaml, Haskell, Lean, Smalltalk,
Forth, APL, SQL, Datalog, COBOL, Rexx and thirty-odd more — each built from the
same spec and measured against the same oracle.

## What that does and does not prove

It shows the specification is complete and unambiguous enough that a peer can be
derived from it without recourse to a reference implementation's source. When a
language whose number model, dispatch discipline or memory model has nothing in
common with the last one still reaches the same wire behaviour, the property
being demonstrated belongs to the spec rather than to any implementation.

It is **not** evidence that many separate teams independently agreed. The peers
share a generation lineage — one generator, one spec — which makes the cohort
*consistent*, not *independent*. The honest name for what it shows is
**spec-forced convergence across languages**. Independent agreement is a
different claim, and the three separately written reference implementations
(Go, Rust, Python) are what carries it.

## The gate

Conformance is a tracked, moving state, not a badge. The gate is
`validate-peer --profile core` — the extension-free categories: connectivity,
encoding, type system, origination, resource bounds, concurrency, plus the
register and conformance-handler gates. Most of the cohort passes it; the peers
that do not are published with a named defect rather than dropped from the count,
and one is a deliberately exploratory probe that was never in scope for the gate.

Read the row, not the summary. A peer sitting a spec version behind, or lacking
Ed448 agility, or carrying a known gap, is a documented state with a name and a
tier — that is the point of publishing the matrix rather than a percentage. Green
rows are pinned to a specific oracle commit and are superseded by re-measurement,
not preserved by it.

- **[Conformance Matrix](conformance-matrix.md)** — the transparency contract: every peer's spec version, oracle commit, gate result, codec strategy, crypto floor and known gaps. Authoritative over this page.
- **[validate-peer — the harness contract](validate-peer.md)** — how the gate is invoked and how its output is read, including what a SKIP means next to a FAIL.
- **[Conformance Invariants (N1–N8)](conformance-invariants.md)** — the eight places a correct-looking implementation silently diverges on the wire. Each was found in the reference implementations first; they are pinned so the generator prevents them rather than rediscovering them.

The generator, the cohort and the matrix are maintained in
[`entity-core-keystone`](https://github.com/EntityChurch/entity-core-keystone).

Convergence is one of two evidence classes this domain publishes. The other is
proof: see **[Formalization](../formalization.md)**.
