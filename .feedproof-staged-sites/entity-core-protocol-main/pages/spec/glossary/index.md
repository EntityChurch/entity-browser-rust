+++
title = "Glossary"
content_class = "generated"
source = "entity-core-protocol/specs"
recipe = "authored vocabulary resolved against the corpus — definitions authored or pulled from canonical homes, occurrences word-boundary matched and reported per document, boilerplate suppressed (deterministic)."
+++

The vocabulary this corpus uses — 24 of 25 authored terms occur in `entity-core-protocol/specs` (3 files). Definitions are authored or pulled from each term's canonical home; occurrences are located deterministically.


## B

### Bootstrap type

One of the 14 seed types required to define all other types.

_— ENTITY-NATIVE-TYPE-SYSTEM §1.3 (Terminology)_

Also: `bootstrap types`.

See also: Type entity, Type resolution.

_Used 12 times across 2 documents._

- [Native Type System](/spec/native-type-system/index) — 11
- [Entity Core Protocol](/spec/entity-core-protocol/index) — 1

## C

### Capability

The authority a peer holds to act, transferred cryptographically and validated by chain-walking.

_— ENTITY-CORE-PROTOCOL §3.5_

Also: `capabilities`.

See also: Peer, Identity.

_Used 505 times across 3 documents._

- [Entity Core Protocol](/spec/entity-core-protocol/index) — 425
- [Native Type System](/spec/native-type-system/index) — 77
- [CBOR Encoding](/spec/cbor-encoding/index) — 3

### CBOR

Concise Binary Object Representation (RFC 8949) — the wire encoding. Chosen for type preservation, deterministic encoding, and being self-describing.

_— ENTITY-CBOR-ENCODING §1.3, §2_

See also: ECF, CDDL.

_Used 196 times across 3 documents._

- [Native Type System](/spec/native-type-system/index) — 87
- [CBOR Encoding](/spec/cbor-encoding/index) — 73
- [Entity Core Protocol](/spec/entity-core-protocol/index) — 36

### CDDL

Concise Data Definition Language (RFC 8610) — the notation used for type definitions.

_— ENTITY-CBOR-ENCODING §1.3 (Terminology)_

See also: CBOR, Type entity.

_Used 27 times across 2 documents._

- [CBOR Encoding](/spec/cbor-encoding/index) — 20
- [Native Type System](/spec/native-type-system/index) — 7

### Conformance

The contract a peer is measured against — not the version number. The full suite is the gate, and a published number is reproducible and oracle-pinned.

_— ADR-0012; AGENTS-STANDARD.md §Honesty & conformance_

Also: `conformance level`, `conformance levels`.

See also: Peer, Keystone.

_Used 87 times across 3 documents._

- [Entity Core Protocol](/spec/entity-core-protocol/index) — 60
- [CBOR Encoding](/spec/cbor-encoding/index) — 18
- [Native Type System](/spec/native-type-system/index) — 9

### Constraint

A value predicate attached to a field-spec: an entity `{type, data}` whose type selects the handler and whose data supplies the parameters.

_— EXTENSION-TYPE §1.6 (Terminology)_

Also: `constraints`.

See also: Field-spec, Narrowing, Handler.

_Used 93 times across 3 documents._

- [Native Type System](/spec/native-type-system/index) — 51
- [Entity Core Protocol](/spec/entity-core-protocol/index) — 39
- [CBOR Encoding](/spec/cbor-encoding/index) — 3

### Content hash

`format_code ‖ SHA256(ECF(type, data))` — the identifier derived from an entity's own content.

_— ENTITY-CORE-PROTOCOL §1.2, §7.1_

Also: `content_hash`, `content-addressed`, `content addressing`.

See also: Identity, ECF, Entity.

_Used 224 times across 3 documents._

- [Entity Core Protocol](/spec/entity-core-protocol/index) — 147
- [Native Type System](/spec/native-type-system/index) — 53
- [CBOR Encoding](/spec/cbor-encoding/index) — 24

## E

### ECF

Entity Canonical Form — the deterministic encoding used for hashing.

_— ENTITY-CBOR-ENCODING §1.3 (Terminology)_

Also: `Entity Canonical Form`.

See also: CBOR, Content hash.

_Used 88 times across 3 documents._

- [CBOR Encoding](/spec/cbor-encoding/index) — 56
- [Entity Core Protocol](/spec/entity-core-protocol/index) — 20
- [Native Type System](/spec/native-type-system/index) — 12

### Emit

The atomic Store-and-Bind: writing an entity and binding it into the tree as two independently observable events.

_— ENTITY-CORE-PROTOCOL §6.10_

See also: Tree, Execution.

_Used 37 times across 3 documents._

- [Entity Core Protocol](/spec/entity-core-protocol/index) — 34
- [Native Type System](/spec/native-type-system/index) — 2
- [CBOR Encoding](/spec/cbor-encoding/index) — 1

### Entity

The typed data unit — `{type, data, content_hash}`. Everything in the system is one.

_— ENTITY-CORE-PROTOCOL §1.1_

Also: `entities`.

See also: Identity, Tree, Content hash.

_Used 611 times across 3 documents._

- [Entity Core Protocol](/spec/entity-core-protocol/index) — 386
- [Native Type System](/spec/native-type-system/index) — 146
- [CBOR Encoding](/spec/cbor-encoding/index) — 79

### Envelope

The message container: a root entity plus a map of the entities included with it.

_— ENTITY-CORE-PROTOCOL §3.1_

See also: Entity, EXECUTE.

_Used 126 times across 3 documents._

- [Entity Core Protocol](/spec/entity-core-protocol/index) — 85
- [Native Type System](/spec/native-type-system/index) — 35
- [CBOR Encoding](/spec/cbor-encoding/index) — 6

### EXECUTE

One of the protocol's only two messages, with `EXECUTE_RESPONSE`. All dispatch goes through it.

_— ENTITY-CORE-PROTOCOL §3_

Also: `EXECUTE_RESPONSE`.

See also: Execution, Envelope, Handler.

_Used 182 times across 2 documents._

- [Entity Core Protocol](/spec/entity-core-protocol/index) — 154
- [Native Type System](/spec/native-type-system/index) — 28

### Execution

Typed dispatch via `EXECUTE` — the protocol's evaluator. Adding it to the substrate is the largest single-step unlock in the build-up sequence.

_— ENTITY-CORE-PROTOCOL §3.2–3.3_

See also: Emit, Peer, Handler.

_Used 22 times across 2 documents._

- [Entity Core Protocol](/spec/entity-core-protocol/index) — 20
- [Native Type System](/spec/native-type-system/index) — 2

## F

### Field-spec

A field specification defining the shape and optionality of a field.

_— ENTITY-NATIVE-TYPE-SYSTEM §1.3 (Terminology)_

Also: `field spec`, `field-specs`.

See also: Constraint, Type resolution.

_Used 52 times across 3 documents._

- [Native Type System](/spec/native-type-system/index) — 39
- [Entity Core Protocol](/spec/entity-core-protocol/index) — 9
- [CBOR Encoding](/spec/cbor-encoding/index) — 4

## H

### Handler

The unit that implements an operation, reached through the dispatch chain under the capability model.

_— EXTENSION-TYPE §2.1; ENTITY-CORE-PROTOCOL §3.2–3.3_

Also: `handlers`.

See also: Execution, EXECUTE, Capability.

_Used 541 times across 3 documents._

- [Entity Core Protocol](/spec/entity-core-protocol/index) — 473
- [Native Type System](/spec/native-type-system/index) — 67
- [CBOR Encoding](/spec/cbor-encoding/index) — 1

## I

### Identity

Content-derived naming: an entity's name IS a hash of its content, so the name cannot disagree with what it names.

_— ENTITY-CORE-PROTOCOL §1.2, §7.1_

See also: Entity, Content hash, Peer.

_Used 121 times across 3 documents._

- [Entity Core Protocol](/spec/entity-core-protocol/index) — 98
- [Native Type System](/spec/native-type-system/index) — 20
- [CBOR Encoding](/spec/cbor-encoding/index) — 3

## N

### Narrowing

The requirement that a child type's constraints are equal to or more restrictive than its parent's.

_— EXTENSION-TYPE §1.6 (Terminology)_

See also: Constraint, Type resolution.

_Used 23 times across 2 documents._

- [Entity Core Protocol](/spec/entity-core-protocol/index) — 13
- [Native Type System](/spec/native-type-system/index) — 10

## P

### Peer

Identity plus capabilities plus connection — the unit of distribution.

_— ENTITY-CORE-PROTOCOL §3.5_

Also: `peers`.

See also: Identity, Execution, Capability, Conformance.

_Used 536 times across 3 documents._

- [Entity Core Protocol](/spec/entity-core-protocol/index) — 475
- [Native Type System](/spec/native-type-system/index) — 44
- [CBOR Encoding](/spec/cbor-encoding/index) — 17

### Primitive

One of the six irreducible units the whole system is built from: entity, identity and tree (informational); emit and execution (temporal); peer (spatial). Everything above them is composition of them.

_— ENTITY-CORE-PROTOCOL §1_

Also: `primitives`.

See also: Entity, Identity, Tree, Emit, Execution, Peer.

_Used 444 times across 3 documents._

- [Native Type System](/spec/native-type-system/index) — 272
- [Entity Core Protocol](/spec/entity-core-protocol/index) — 154
- [CBOR Encoding](/spec/cbor-encoding/index) — 18

## S

### Structural compatibility

The property of two types expanding to the same effective field set — the basis on which peers decide whether they can meaningfully process each other's data.

_— ENTITY-NATIVE-TYPE-SYSTEM §1.3, §12_

See also: Type resolution, Peer.

_Used 7 times across 1 document._

- [Native Type System](/spec/native-type-system/index) — 7

## T

### Tree

A mutable namespace of `path → hash` bindings laid over the immutable content store. The tree is what changes; the entities it points at do not.

_— ENTITY-CORE-PROTOCOL §1.4, §1.7_

Also: `trees`.

See also: Entity, Emit, Peer.

_Used 400 times across 3 documents._

- [Entity Core Protocol](/spec/entity-core-protocol/index) — 293
- [Native Type System](/spec/native-type-system/index) — 104
- [CBOR Encoding](/spec/cbor-encoding/index) — 3

### Type entity

An entity of type `system/type` that defines a type. Type definitions are themselves entities in the tree — this is the self-description fixed point.

_— ENTITY-NATIVE-TYPE-SYSTEM §1.3 (Terminology)_

See also: Entity, Type path, Bootstrap type.

_Used 5 times across 1 document._

- [Native Type System](/spec/native-type-system/index) — 5

### Type path

The short path identifying a type — e.g. `primitive/string`, `system/protocol/connect/hello`.

_— ENTITY-NATIVE-TYPE-SYSTEM §1.3 (Terminology)_

See also: Type entity, Type resolution.

_Used 11 times across 2 documents._

- [Native Type System](/spec/native-type-system/index) — 9
- [Entity Core Protocol](/spec/entity-core-protocol/index) — 2

### Type resolution

Expanding a type path to its effective field set.

_— ENTITY-NATIVE-TYPE-SYSTEM §1.3 (Terminology)_

See also: Type path, Structural compatibility.

_Used 12 times across 2 documents._

- [Native Type System](/spec/native-type-system/index) — 10
- [Entity Core Protocol](/spec/entity-core-protocol/index) — 2

## Not used in this corpus

Authored terms that do not occur here — they belong to another part of the system.

Keystone.
