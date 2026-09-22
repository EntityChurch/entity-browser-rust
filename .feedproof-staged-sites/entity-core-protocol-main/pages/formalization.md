+++
title = "Formalization"
content_class = "authored"
source = "content/entity-core-protocol/formalization.md (via stage0 scrub)"
+++

# Formalization

Running peers agree with each other; that is what [Keystone](keystone/index.md)
measures. It is a different question whether the design they agree on holds — and
that one is answered by machine-checked models rather than by more peers.

The protocol's formal models live in
[`entity-core-formalization`](https://github.com/EntityChurch/entity-core-formalization):
TLA+ for the state machine and its invariants, Spin for the concurrent protocol
behaviour, and Tamarin and ProVerif for the cryptographic surface — the handshake,
identity binding and capability delegation.

The two classes answer different questions and neither substitutes for the other.
A model checker will find a design that cannot be made to work no matter how
carefully it is implemented; a conformance gate will find an implementation that
does not do what a working design says. What is proved, and under which
assumptions, is stated in that repository, which is the authority on its own
results.
