+++
title = "The Specification"
content_class = "authored"
source = "content/entity-core-protocol/spec/index.md (via stage0 scrub)"
+++

# The Specification

The Entity Core Protocol is the irreducible layer every peer must speak: the wire
format, the type system, the identity / capability / dispatch core, and the basic
tree operations (put/get). Two peers that agree on it interoperate at the
substrate; everything above it — the extensions, the SDK, the applications — is
optional and lives with the [Entity System Architecture](https://entitychurchfoundation.org).

> **Canonical reference.** These documents are the normative specification,
> maintained in the [`entity-core-protocol`](https://github.com/EntityChurch/entity-core-protocol)
> repository and reproduced here in full.

Start with the protocol; the rest are its companions.

- **[Entity Core Protocol](entity-core-protocol/index.md)** — the protocol: the universal address space, the tree model, and put/get (Layers 0–4).
- **[CBOR Encoding](cbor-encoding/index.md)** — the Entity Canonical Form (ECF), the wire contract.
- **[Native Type System](native-type-system/index.md)** — the core type system and native types.

Each spec opens on its contents, with one page per section — the normative text
verbatim, in the sections it is written in.

## The vocabulary

- **[Glossary](glossary/index.md)** — the terms the specs are written in: each one
  defined, cited to the spec that defines it, and located at every place it occurs,
  in context. The way in other than reading front to back.
