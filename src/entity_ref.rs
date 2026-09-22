//! `entity-ref` — the reference atom and its string form.
//!
//! `APP-CONVENTION-REFERENCE` v0.1, the applications domain's **foundational
//! member**: one shape for *"this points at that"*, imported by every other
//! convention rather than restated in each. `APP-CONVENTION-EMBED` §3's
//! `child-payload.ref` is one, and `APP-CONVENTION-FEED` §2.2's `reply.root`,
//! `reply.parent`, `context` and `attachments` are all one.
//!
//! **This is why it is built first.** Nearly every FEED field is a reference,
//! and a convention that imports an atom cannot be implemented before the atom
//! has a home. It mints **no entity type** (§6.3) and lands in no published
//! surface: it is a format module, gated natively, with nothing on the wire
//! until a type that carries it ships.
//!
//! ## The two intents are two shapes, and the discriminator is a value
//!
//! A **pin** names *these exact bytes* — self-verifying, satisfiable by anyone
//! holding them, and its answer can never change. A **live** reference names
//! *whatever is at this address now* — only the publisher is authoritative, and
//! the answer is expected to change. §1.1 calls these the two hops of the path
//! model (a path resolves to a hash; a hash resolves to bytes) exposed as two
//! entry points, and *"a consumer that collapses them cannot express 'the
//! version I read' and 'the current version' as different things."*
//!
//! **A reader tells them apart by reading `tag`, never by observing which of
//! `hash` / `path` is present** (§2.2). That is not a spelling preference: a
//! presence rule makes the discriminator an *inference*, needs a third rule
//! interacting with the first two when a third intent arrives, and can only be
//! violated in a way a reader implementing the presence rule detects. **This is
//! the same tagged-union discipline `ShareTarget` and `AssetPayload` already
//! carry**, and for once we inherit it rather than earn it.
//!
//! ## What we refuse, and whose defect each refusal names
//!
//! [`RefError`] follows the discipline `A-30` cost us: **a deliberate schema
//! violation is not a corrupt byte, and an arm we have not built is neither.**
//!
//! | outcome | whose defect | vector |
//! |---|---|---|
//! | [`RefError::UnknownTag`] | **ours** — §2.2 says a third intent *is* a third tag, so this is an arm we have not built | — |
//! | [`RefError::TagIdentityMismatch`] | theirs — a `"pin"` with no `hash` | `REF-R2` / `REF-V4`,`V5` |
//! | [`RefError::ExcludedField`] | theirs — a schema violation, see below | (asked) |
//! | [`RefError::PeerIdNotText`] | theirs — the divergence with no error at either end | `REF-R3` / `REF-V10` |
//! | [`RefError::Malformed`] | theirs — a corrupt or unreadable body | — |
//!
//! **[`RefError::ExcludedField`] is the carve-out rule, applied at the first
//! opportunity after it went tier-wide.** §3.1 says a *string* carrying both a
//! `hash` parameter and a non-empty path is malformed and MUST be refused
//! (`REF-R9`); the **atom** form is silent about a `"pin"` carrying `path` or a
//! `"live"` carrying `hash`, and V7 §2.6's MUST-ignore would have us skip it in
//! silence. That launders exactly the confusion §2.3 spends a paragraph
//! excluding — *"one field name meaning the address of record in one shape and
//! a guess in the other is a discriminator a reader has to know the shape to
//! interpret."* So we refuse it, keep it apart from `Malformed`, and **ask**:
//! the asymmetry between §3.1's explicit refusal and §2.1's silence is routed,
//! and if arch rules MUST-ignore this is one arm to relax. The carve-out is
//! deliberately **narrow** — it covers the discriminator's own two slots and
//! nothing else, so an ordinary unknown field is still ignored per §2.6.
//!
//! ## What this module does NOT do, stated rather than implied
//!
//! **There is no resolver here**, so `REF-R5`, `REF-R6`, `REF-R19` and
//! `REF-R21` — every requirement phrased *"a reader that resolves…"* — are
//! **unmeasured**. What is measurable without one is that an unknown `via` hint
//! survives a decode and is excluded from [`EntityRef::hints_to_try`], which is
//! `REF-R7` and the *structural* half of `REF-R5`. Do not read this module's
//! green tests as `REF-V7` or `REF-V11` being covered.
//!
//! `REF-R17`/`REF-R18` are producer obligations on **link positions** and are
//! not this module's to satisfy — see the note on `entity://` below.
//!
//! ## ⚠ Shipping this atom changes what our SITE renderer owes
//!
//! §4: *"**[MUST NOT]** A producer **that emits the atom at all** MUST NOT emit
//! `entity://` in a link position."* We emit `entity://{peer}/sites/{id}/` in
//! link positions today ([`crate::content_site::location`], the static export,
//! the content-site output) and we emit no atom, so we are conformant now. **The
//! moment a type carrying an `entity-ref` ships — which is what phase 2a is
//! for — that clause engages and those emissions become non-conformant.** The
//! consumer half is already right: §4's `SHOULD` keeps `entity://` resolving,
//! which `classify_link` does. Routed as a scope question (what is the
//! granularity of *"a producer"* — the implementation, the document, or the
//! link position?) rather than pre-empted here, because the answer decides
//! whether a published-corpus migration is owed.
//!
//! ## The string form, and the four places §3 does not say enough
//!
//! §3.2 makes losslessness a **[MUST]** in both directions. Satisfying it
//! *jointly* — the only way it means anything — needs a spelling for every term
//! in the query and fragment, and §3 names none. Four instances, all routed as
//! one finding, all implemented here with a choice that is stated rather than
//! assumed:
//!
//! 1. **`hash` / `seen` spelling.** We emit [`Hash::to_hex`] — lowercase hex of
//!    the *self-describing* wire form, format varint first. **Not a new
//!    spelling:** it is the `{content_hash_hex}` segment of V7 §3.5's invariant
//!    pointer path, i.e. the one this system already publishes. Accepted in
//!    either case, emitted lowercase.
//! 2. **`via` spelling.** `via={tag}:{value}`, repeated, each half
//!    percent-encoded as a component so a `:` inside either is data.
//! 3. **`at` spelling.** The fragment is the anchor's field names,
//!    percent-encoded and joined with `/`.
//! 4. **Query parameter order.** Byte-identical re-serialization requires a
//!    canonical order and §3 states none. Ours is §2.1's field order:
//!    `hash` / `seen`, then `via`.
//!
//! And three shapes the string form **cannot represent**, so the losslessness
//! MUST is unsatisfiable for them rather than merely unspecified. **The rule in
//! all three cases is the same and is the transferable half: the wire must not
//! be able to express what the string cannot, so the collapse goes in the
//! ENCODER as well as the decoder** — a type that cannot express the confusion
//! cannot ship it:
//!
//! - **`via: []` and `at: {field: []}`.** Zero query occurrences *is* absent and
//!   an empty fragment is not a field path, so present-and-empty has no
//!   spelling. We collapse both to absent **in the encoder as well as the
//!   decoder**, so the wire cannot express what the string cannot — a type that
//!   cannot express the confusion cannot ship it. An empty anchor also means
//!   *the whole entity*, which is what an absent `at` means, so the collapse
//!   loses no fact.
//! - **The peer root** — `path: ""` or `path: "/"`. Rendered, it is
//!   `entity+ref://{peer}/`, which reads back as *no identity term at all*
//!   (`REF-R10`). This one was **missed on the first pass**: the decoder
//!   refused `""` and admitted `"/"`, which are the same address, so the rule
//!   above was stated and left an instance. [`is_addressable_path`] is the one
//!   expression now, consulted by the decoder and by [`EntityRef::to_uri`].
//! - **A peer-relative `path`.** The URI path component always begins with `/`,
//!   so a `live` atom whose `path` did not would come back changed. We
//!   normalize on construction ([`EntityRef::live`]), which makes our own atoms
//!   round-trip exactly and leaves the question — may a `live` atom carry an
//!   *absolute* `/{peer}/…` tree path, and how does that project? — routed. The
//!   authority component already names the peer, exactly as `entity://` does,
//!   so restating it inside the path would be the second source of truth EMBED
//!   deleted from `child-payload` by hand.

#![allow(dead_code)] // the FEED types that carry this atom land in phase 2a's next slice

use entity_ecf::{bytes as ecf_bytes, text, Value};
use entity_hash::Hash;

use crate::percent;

/// The scheme, with its authority marker. §3.1.
pub const SCHEME: &str = "entity+ref://";

/// §2.1's `pinned-ref` discriminator.
pub const TAG_PIN: &str = "pin";

/// §2.1's `live-ref` discriminator.
pub const TAG_LIVE: &str = "live";

// ---------------------------------------------------------------------------
// The atom
// ---------------------------------------------------------------------------

/// §2.4's `anchor` — *which part*, as a path of field names into the referenced
/// entity.
///
/// **Absent means the whole entity, so nothing changes for a consumer that does
/// not use it.** It is declared now rather than later because the alternative
/// is minting a second atom for *"the same thing, but part of it"*, and because
/// a field path is the one part of a reference that survives a change of bytes:
/// the hash changes, the field path does not.
///
/// **`field` is an opaque sequence of names in v0.1.** The core's address
/// primitives are content hash, tree path, type name and peer id; an
/// intra-entity field path is none of them, and *"a slot with no consumer does
/// not mint a grammar."*
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    pub field: Vec<String>,
}

/// §2.3's four hint kinds, plus the one an unknown tag lands in.
///
/// **[`HintTag::Other`] preserves the tag rather than dropping it**, and that is
/// `REF-R7` and `REF-R8` pulling in the same direction: an unknown kind must be
/// *ignored* rather than refused, and a re-serializer that dropped it would
/// *"silently rewrite other people's references"* (§3.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HintTag {
    /// Where the publisher serves bytes from.
    Origin,
    /// Somewhere else that had them.
    Mirror,
    /// Somebody who is likely to hold them.
    Peer,
    /// Where in the publisher's tree it was placed.
    Path,
    /// A kind this build does not know. Carried, never acted on.
    Other(String),
}

impl HintTag {
    /// The wire token.
    pub fn as_token(&self) -> &str {
        match self {
            HintTag::Origin => "origin",
            HintTag::Mirror => "mirror",
            HintTag::Peer => "peer",
            HintTag::Path => "path",
            HintTag::Other(t) => t,
        }
    }

    /// Parse a wire token. **Total** — an unrecognised token becomes
    /// [`HintTag::Other`], because refusing it would make an advisory term
    /// load-bearing, which is the failure §2.3 exists to prevent.
    pub fn from_token(t: &str) -> Self {
        match t {
            "origin" => HintTag::Origin,
            "mirror" => HintTag::Mirror,
            "peer" => HintTag::Peer,
            "path" => HintTag::Path,
            other => HintTag::Other(other.to_string()),
        }
    }

    /// Whether a resolver in this build knows what to do with it.
    pub fn is_known(&self) -> bool {
        !matches!(self, HintTag::Other(_))
    }
}

/// §2.3's `hint` — **advisory, ordered by descending confidence, and
/// droppable, or it is not a hint.**
///
/// > **[MUST]** A reader that ignores every `via` hint MUST reach the same
/// > answer as one that uses them, or fail. A hint MUST NOT be the only path to
/// > a correct resolution, MUST NOT be signed, and MUST NOT be treated as
/// > authority for anything. (`REF-R5`, `REF-R6`.)
///
/// **The `path` kind is the one that is easy to get wrong.** On a pinned
/// reference the hash is the identity, so a path can only say *where it was
/// put* — and a `404` there is **not** evidence the referent does not exist
/// (`REF-R21`), since the bytes validate against the hash from any source at
/// all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    pub tag: HintTag,
    pub value: String,
}

impl Hint {
    pub fn new(tag: HintTag, value: impl Into<String>) -> Self {
        Self { tag, value: value.into() }
    }
}

/// §2.1's `entity-ref` — **tagged; there is no untagged form.**
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntityRef {
    /// *These exact bytes.* Identity and expectation coincide.
    Pinned {
        /// WHO published it. A reference is routable iff it names one.
        peer: String,
        /// WHAT it is.
        hash: Hash,
        /// Which part. `None` = the whole entity.
        at: Option<Anchor>,
        /// Advisory. Empty = absent; see the module doc.
        via: Vec<Hint>,
    },
    /// *Whatever is at this address now.* Only the publisher is authoritative.
    Live {
        /// WHO publishes there.
        peer: String,
        /// The address of record — **authoritative**. Peer-relative, leading
        /// `/`; see [`EntityRef::live`].
        path: String,
        /// What the linker saw — **an expectation only** (§2.2.2).
        seen: Option<Hash>,
        at: Option<Anchor>,
        via: Vec<Hint>,
    },
}

impl EntityRef {
    /// A pinned reference with no anchor and no hints — §5's whole floor.
    pub fn pin(peer: impl Into<String>, hash: Hash) -> Self {
        EntityRef::Pinned { peer: peer.into(), hash, at: None, via: Vec::new() }
    }

    /// A live reference with no expectation, anchor or hints.
    ///
    /// **`path` is normalized to a leading `/`.** The URI path component always
    /// carries one, so an atom without it could not survive
    /// atom → string → atom, and §3.2 makes that round trip a MUST. See the
    /// module doc for what is routed about the absolute form.
    pub fn live(peer: impl Into<String>, path: impl Into<String>) -> Self {
        let path = path.into();
        let path = if path.starts_with('/') { path } else { format!("/{path}") };
        EntityRef::Live { peer: peer.into(), path, seen: None, at: None, via: Vec::new() }
    }

    /// WHO — present on both shapes, because a reference with no publisher
    /// names no holder you can go and ask.
    pub fn peer(&self) -> &str {
        match self {
            EntityRef::Pinned { peer, .. } | EntityRef::Live { peer, .. } => peer,
        }
    }

    /// Whether this is a **pin**.
    ///
    /// Exists because `APP-CONVENTION-FEED` §2.2.1 declares, per site, which
    /// atom that site accepts — `reply.root`, `reply.parent` and an index
    /// page's `entries` take a pinned reference **only**, while `context` and
    /// `attachments` take either. A carrying convention needs to ask, and
    /// asking by matching on the variant at each site would put the rule in as
    /// many places as there are fields.
    pub fn is_pinned(&self) -> bool {
        matches!(self, EntityRef::Pinned { .. })
    }

    /// The wire tag.
    pub fn tag(&self) -> &'static str {
        match self {
            EntityRef::Pinned { .. } => TAG_PIN,
            EntityRef::Live { .. } => TAG_LIVE,
        }
    }

    /// Which part, if any.
    pub fn anchor(&self) -> Option<&Anchor> {
        match self {
            EntityRef::Pinned { at, .. } | EntityRef::Live { at, .. } => at.as_ref(),
        }
    }

    /// Every hint carried, known or not — the round-trip view.
    pub fn hints(&self) -> &[Hint] {
        match self {
            EntityRef::Pinned { via, .. } | EntityRef::Live { via, .. } => via,
        }
    }

    /// The hints a resolver in this build may act on — **`REF-R7`'s structural
    /// half.** An unknown kind is carried by [`Self::hints`] and excluded here,
    /// so ignoring it costs nothing and refusing the atom is never reachable.
    pub fn hints_to_try(&self) -> Vec<&Hint> {
        self.hints().iter().filter(|h| h.tag.is_known()).collect()
    }

    /// Replace the anchor. An anchor naming no field is the whole entity, which
    /// is what absent means, so it collapses to `None`.
    pub fn with_at(mut self, anchor: Anchor) -> Self {
        let normalized = (!anchor.field.is_empty()).then_some(anchor);
        match &mut self {
            EntityRef::Pinned { at, .. } | EntityRef::Live { at, .. } => *at = normalized,
        }
        self
    }

    /// Replace the hint list.
    pub fn with_via(mut self, hints: Vec<Hint>) -> Self {
        match &mut self {
            EntityRef::Pinned { via, .. } | EntityRef::Live { via, .. } => *via = hints,
        }
        self
    }

    /// Replace a live reference's `seen` expectation. A no-op on a pin, where
    /// the hash *is* the identity and an expectation would be a second copy of
    /// it.
    pub fn with_seen(mut self, hash: Hash) -> Self {
        if let EntityRef::Live { seen, .. } = &mut self {
            *seen = Some(hash);
        }
        self
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// Why a reference was refused. **Each variant names whose defect it is** — see
/// the module doc's table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefError {
    /// **OURS.** A tag that is neither `"pin"` nor `"live"`. §2.2 says a third
    /// intent *is* a third tag, so this is an arm we have not built rather than
    /// a body that is wrong — the distinction `A-30` cost us one convention
    /// over, where reporting an excluded arm as malformed told an operator the
    /// publisher shipped a corrupt byte.
    UnknownTag { tag: String },
    /// The tag and its required identity term disagree — a `"pin"` with no
    /// `hash`, or a `"live"` with no `path`. `REF-R2`.
    ///
    /// **This is why there are two shapes rather than one with an optional
    /// hash:** under an optional hash, a reference arriving without one is
    /// indistinguishable between *the author wants the live version*, *the
    /// author's implementation did not populate it*, and *the author only ever
    /// had an address* — one intent and two defects, with nothing to separate
    /// them.
    TagIdentityMismatch { tag: &'static str, missing: &'static str },
    /// A schema violation: the *other* shape's identity term is present. See
    /// the module doc — narrow, deliberate, and routed.
    ExcludedField { tag: &'static str, field: &'static str },
    /// A `peer` that is not a text string. `REF-R3` / `REF-V10`.
    ///
    /// Called out because it is easy to reach by analogy — `content-hash` is a
    /// `bstr` and is self-describing, a peer id is also self-describing, so the
    /// two look like the same kind of term. **They are not: one is bytes the
    /// system hashes, the other is an identifier the system spells.** The
    /// failure it prevents has no error at either end.
    PeerIdNotText,
    /// A body that is unreadable, or a required field missing or ill-typed.
    Malformed(&'static str),

    // --- string form ---------------------------------------------------
    /// Not an `entity+ref://` string at all. **`REF-R20`: treat it as leaving
    /// the system**, never as something to re-anchor — *"a tolerant
    /// re-anchoring scan produces a well-formed wrong location and cannot
    /// report that it did."*
    NotAReferenceUri,
    /// `entity+ref:` with no `//` and a peer id. **The authority is mandatory**;
    /// it is the term that makes a reference routable at all.
    NoAuthority,
    /// Both a `hash` parameter and a non-empty path. `REF-R9` / `REF-V4`.
    BothIdentityTerms,
    /// Neither. `REF-R10` / `REF-V5`.
    NoIdentityTerm,
    /// A query parameter present with an empty value. `REF-R16` — an absent
    /// parameter and one present-but-empty are different, and the second is
    /// malformed.
    EmptyParameter { name: &'static str },
    /// A `.` or `..` segment in an **absolute-form** path. `REF-R15` /
    /// `REF-V8`.
    ///
    /// A tree path is not a filesystem path and `..` has no meaning in it; *"a
    /// resolver that borrows filesystem semantics here produces a different,
    /// well-formed, wrong address."* **Scoped to the absolute form** — §3.4's
    /// relative form is directory-relative, where `..` is meaningful and
    /// expected, and applying this to an unresolved relative string rejects
    /// ordinary correct links (`REF-V9`).
    DotSegment,
    /// A percent-escape that does not decode.
    BadEscape,
    /// A `hash` or `seen` parameter that is not a content hash.
    BadHash { param: &'static str },
}

impl std::fmt::Display for RefError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RefError::UnknownTag { tag } => {
                write!(f, "reference tag {tag:?} is an intent this build does not implement")
            }
            RefError::TagIdentityMismatch { tag, missing } => {
                write!(f, "a {tag:?} reference with no {missing}")
            }
            RefError::ExcludedField { tag, field } => {
                write!(f, "a {tag:?} reference carrying {field}, which its shape excludes")
            }
            RefError::PeerIdNotText => write!(f, "peer id is not a text string"),
            RefError::Malformed(what) => write!(f, "malformed reference: {what}"),
            RefError::NotAReferenceUri => write!(f, "not an entity+ref:// reference"),
            RefError::NoAuthority => write!(f, "reference has no authority (peer id)"),
            RefError::BothIdentityTerms => {
                write!(f, "reference carries both a hash and a path")
            }
            RefError::NoIdentityTerm => write!(f, "reference carries neither a hash nor a path"),
            RefError::EmptyParameter { name } => {
                write!(f, "parameter {name:?} is present with an empty value")
            }
            RefError::DotSegment => write!(f, "absolute-form path contains a . or .. segment"),
            RefError::BadEscape => write!(f, "reference contains a malformed percent-escape"),
            RefError::BadHash { param } => write!(f, "parameter {param:?} is not a content hash"),
        }
    }
}

// ---------------------------------------------------------------------------
// The atom, on the wire
// ---------------------------------------------------------------------------

fn anchor_value(anchor: &Anchor) -> Value {
    Value::Map(vec![(
        text("field"),
        Value::Array(anchor.field.iter().map(|s| text(s.clone())).collect()),
    )])
}

fn hint_value(hint: &Hint) -> Value {
    Value::Map(vec![
        (text("tag"), text(hint.tag.as_token().to_string())),
        (text("value"), text(hint.value.clone())),
    ])
}

fn field<'a>(map: &'a [(Value, Value)], name: &str) -> Option<&'a Value> {
    map.iter().find(|(k, _)| k.as_text() == Some(name)).map(|(_, v)| v)
}

/// Whether a `live` atom's `path` names anything at all.
///
/// **One expression, consulted in both directions**, because the two ends have
/// different wrong implementations and only one rule: `""` and `"/"` are both
/// *the peer root*, which is not an entity and not an address anyone links to.
/// Admitting either leaves the wire able to express a shape the string form
/// cannot — `entity+ref://{peer}/` reads back as *no identity term* — and §3.2
/// makes the atom → string → atom round trip a `[MUST]`. Same collapse the
/// module doc already applies to `via: []` and an empty anchor.
fn is_addressable_path(path: &str) -> bool {
    !path.is_empty() && path != "/"
}

fn decode_anchor(v: &Value) -> Result<Option<Anchor>, RefError> {
    let map = v.as_map().ok_or(RefError::Malformed("at is not a map"))?;
    let list = field(map, "field")
        .and_then(|v| v.as_array())
        .ok_or(RefError::Malformed("at has no field list"))?;
    let mut names = Vec::with_capacity(list.len());
    for item in list {
        let name = item
            .as_text()
            .ok_or(RefError::Malformed("at field name is not text"))?;
        names.push(name.to_string());
    }
    // Empty = the whole entity = absent. See the module doc.
    Ok((!names.is_empty()).then_some(Anchor { field: names }))
}

fn decode_hints(v: &Value) -> Result<Vec<Hint>, RefError> {
    let list = v.as_array().ok_or(RefError::Malformed("via is not an array"))?;
    let mut out = Vec::with_capacity(list.len());
    for item in list {
        let map = item.as_map().ok_or(RefError::Malformed("via hint is not a map"))?;
        let tag = field(map, "tag")
            .and_then(|v| v.as_text())
            .ok_or(RefError::Malformed("via hint has no tag"))?;
        let value = field(map, "value")
            .and_then(|v| v.as_text())
            .ok_or(RefError::Malformed("via hint has no value"))?;
        out.push(Hint { tag: HintTag::from_token(tag), value: value.to_string() });
    }
    Ok(out)
}

impl EntityRef {
    /// Encode the atom as the CBOR value a carrying type embeds in its `data`.
    ///
    /// **Key order here is not the wire order** — a carrying entity's `data`
    /// goes through `entity_ecf::to_ecf`, which canonicalizes recursively
    /// (length, then lexical) and the encoder gets no say. A test that means to
    /// pin the schema must not rely on the order fields are pushed in.
    pub fn to_value(&self) -> Value {
        let mut fields = vec![(text("tag"), text(self.tag()))];
        match self {
            EntityRef::Pinned { peer, hash, .. } => {
                fields.push((text("peer"), text(peer.clone())));
                // `content-hash = bstr`, SELF-DESCRIBING: the format-code varint
                // then the digest, whose length follows the code. Never a fixed
                // 32 bytes — baking a width re-locks the cage hash agility
                // removed, and §2.1 says so by hand.
                fields.push((text("hash"), ecf_bytes(hash.to_bytes())));
            }
            EntityRef::Live { peer, path, seen, .. } => {
                fields.push((text("peer"), text(peer.clone())));
                fields.push((text("path"), text(path.clone())));
                if let Some(seen) = seen {
                    fields.push((text("seen"), ecf_bytes(seen.to_bytes())));
                }
            }
        }
        if let Some(anchor) = self.anchor() {
            fields.push((text("at"), anchor_value(anchor)));
        }
        let via = self.hints();
        if !via.is_empty() {
            fields.push((
                text("via"),
                Value::Array(via.iter().map(hint_value).collect()),
            ));
        }
        Value::Map(fields)
    }

    /// Decode the atom from a carrying type's field.
    ///
    /// A stranger's tree is untrusted input, so this never panics and never
    /// half-fills a reference. Ordinary unknown fields are ignored (V7 §2.6);
    /// the **two** exceptions are the other shape's identity term — see the
    /// module doc on [`RefError::ExcludedField`].
    pub fn from_value(v: &Value) -> Result<Self, RefError> {
        let map = v.as_map().ok_or(RefError::Malformed("reference is not a map"))?;

        let tag = field(map, "tag")
            .ok_or(RefError::Malformed("reference has no tag"))?
            .as_text()
            .ok_or(RefError::Malformed("reference tag is not text"))?
            .to_string();

        let peer_value = field(map, "peer").ok_or(RefError::Malformed("reference has no peer"))?;
        let peer = match peer_value.as_text() {
            Some(p) => p.to_string(),
            // `REF-R3`: a peer id typed as bytes is a *different thing*, not a
            // different spelling. Its own refusal, because the divergence it
            // causes has no error at either end.
            None if peer_value.as_bytes().is_some() => return Err(RefError::PeerIdNotText),
            None => return Err(RefError::Malformed("reference peer is not text")),
        };
        if peer.is_empty() {
            return Err(RefError::Malformed("reference peer is empty"));
        }

        let at = match field(map, "at") {
            Some(v) => decode_anchor(v)?,
            None => None,
        };
        let via = match field(map, "via") {
            Some(v) => decode_hints(v)?,
            None => Vec::new(),
        };

        let hash_bytes = |name: &'static str| -> Result<Option<Hash>, RefError> {
            match field(map, name) {
                Some(v) => {
                    let bytes = v
                        .as_bytes()
                        .ok_or(RefError::Malformed("content hash is not a byte string"))?;
                    Hash::from_bytes(bytes)
                        .map(Some)
                        .map_err(|_| RefError::BadHash { param: name })
                }
                None => Ok(None),
            }
        };

        match tag.as_str() {
            TAG_PIN => {
                if field(map, "path").is_some() {
                    return Err(RefError::ExcludedField { tag: TAG_PIN, field: "path" });
                }
                let hash = hash_bytes("hash")?.ok_or(RefError::TagIdentityMismatch {
                    tag: TAG_PIN,
                    missing: "hash",
                })?;
                Ok(EntityRef::Pinned { peer, hash, at, via })
            }
            TAG_LIVE => {
                if field(map, "hash").is_some() {
                    return Err(RefError::ExcludedField { tag: TAG_LIVE, field: "hash" });
                }
                let path = field(map, "path")
                    .and_then(|v| v.as_text())
                    .ok_or(RefError::TagIdentityMismatch { tag: TAG_LIVE, missing: "path" })?
                    .to_string();
                if !is_addressable_path(&path) {
                    return Err(RefError::TagIdentityMismatch {
                        tag: TAG_LIVE,
                        missing: "path",
                    });
                }
                let seen = hash_bytes("seen")?;
                Ok(EntityRef::Live { peer, path, seen, at, via })
            }
            other => Err(RefError::UnknownTag { tag: other.to_string() }),
        }
    }
}

// ---------------------------------------------------------------------------
// The string form (§3)
// ---------------------------------------------------------------------------

/// Lowercase hex of a hash's self-describing wire form — V7 §3.5's
/// `{content_hash_hex}`, reused rather than minted. See the module doc.
fn hash_to_param(h: &Hash) -> String {
    h.to_hex()
}

/// The inverse. Accepts either case; see the module doc on why we emit
/// lowercase and what that costs a non-conformant input.
fn hash_from_param(s: &str, param: &'static str) -> Result<Hash, RefError> {
    if !s.len().is_multiple_of(2) || s.is_empty() {
        return Err(RefError::BadHash { param });
    }
    let mut bytes = Vec::with_capacity(s.len() / 2);
    let raw = s.as_bytes();
    for pair in raw.chunks(2) {
        let hi = (pair[0] as char).to_digit(16).ok_or(RefError::BadHash { param })?;
        let lo = (pair[1] as char).to_digit(16).ok_or(RefError::BadHash { param })?;
        bytes.push((hi * 16 + lo) as u8);
    }
    Hash::from_bytes(&bytes).map_err(|_| RefError::BadHash { param })
}

/// Percent-encode a `/`-joined path, segment by segment. `REF-R14`: reserved
/// characters *within a segment* are encoded; the `/` delimiter never is.
fn encode_path(path: &str) -> String {
    path.split('/')
        .map(|seg| percent::encode_component(seg).into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// `REF-R15`, and **only for the absolute form** — see [`RefError::DotSegment`].
fn refuse_dot_segments(path: &str) -> Result<(), RefError> {
    if path.split('/').any(|seg| seg == "." || seg == "..") {
        return Err(RefError::DotSegment);
    }
    Ok(())
}

impl EntityRef {
    /// §3.1's string form.
    ///
    /// Fallible because the atom can hold a `path` the *string* form must
    /// refuse (`REF-R15`); the CDDL does not exclude a dot segment from a
    /// `tree-path` and §3.3 does exclude it from an absolute-form URI, so the
    /// refusal belongs on this projection rather than on the type.
    pub fn to_uri(&self) -> Result<String, RefError> {
        let mut out = String::from(SCHEME);
        // The authority is a peer id and is CASE-SENSITIVE — emitted verbatim.
        // `REF-R12` / `REF-V3`: a general URL library lowercases the host by
        // default, and a peer id that survives such a parser names a *different
        // peer*, failing as a clean 404 at a well-formed address.
        out.push_str(self.peer());

        let mut query: Vec<String> = Vec::new();
        match self {
            EntityRef::Pinned { hash, .. } => {
                // §3.1: a pinned reference has no path-shaped identity, so the
                // path is empty and the hash is a required query parameter.
                // That reads oddly and it is the honest encoding — putting the
                // hash in the path would make the two shapes structurally
                // indistinguishable to a generic URI parser.
                out.push('/');
                query.push(format!("hash={}", hash_to_param(hash)));
            }
            EntityRef::Live { path, seen, .. } => {
                // The same predicate the decoder uses. A caller can still build
                // `EntityRef::Live { path: "/" }` by hand — the emitter is where
                // that stops, so the two ends cannot disagree about what an
                // address is.
                if !is_addressable_path(path) {
                    return Err(RefError::TagIdentityMismatch {
                        tag: TAG_LIVE,
                        missing: "path",
                    });
                }
                refuse_dot_segments(path)?;
                out.push_str(&encode_path(path));
                if let Some(seen) = seen {
                    query.push(format!("seen={}", hash_to_param(seen)));
                }
            }
        }
        for hint in self.hints() {
            query.push(format!(
                "via={}:{}",
                percent::encode_component(hint.tag.as_token()),
                percent::encode_component(&hint.value)
            ));
        }
        if !query.is_empty() {
            out.push('?');
            out.push_str(&query.join("&"));
        }
        if let Some(anchor) = self.anchor() {
            out.push('#');
            out.push_str(
                &anchor
                    .field
                    .iter()
                    .map(|f| percent::encode_component(f).into_owned())
                    .collect::<Vec<_>>()
                    .join("/"),
            );
        }
        Ok(out)
    }

    /// Parse §3.1's string form.
    ///
    /// Returns [`RefError::NotAReferenceUri`] for anything that is not an
    /// `entity+ref://` string — **`REF-R20`'s outcome, and a classification
    /// rather than a fault.** A caller resolving a link body treats it as
    /// leaving the system.
    pub fn parse_uri(s: &str) -> Result<Self, RefError> {
        // `REF-R11`: the scheme is case-insensitive on parse.
        let rest = strip_scheme(s).ok_or(RefError::NotAReferenceUri)?;

        // Split off the fragment, then the query. Order matters: a `#` before a
        // `?` makes the `?` part of the fragment, per RFC 3986 §3.
        let (before_fragment, fragment) = match rest.split_once('#') {
            Some((a, b)) => (a, Some(b)),
            None => (rest, None),
        };
        let (authority_and_path, query) = match before_fragment.split_once('?') {
            Some((a, b)) => (a, Some(b)),
            None => (before_fragment, None),
        };

        let (peer, raw_path) = match authority_and_path.split_once('/') {
            Some((p, rest)) => (p, format!("/{rest}")),
            None => (authority_and_path, String::new()),
        };
        if peer.is_empty() {
            return Err(RefError::NoAuthority);
        }

        // A path of "" or "/" is *no path*: §3.1 says a pinned reference's path
        // is empty and its own example writes the `/`. A `live` reference whose
        // only path is the peer root is not an address anyone links to.
        let has_path = !raw_path.is_empty() && raw_path != "/";

        let mut hash_param: Option<&str> = None;
        let mut seen_param: Option<&str> = None;
        let mut via: Vec<Hint> = Vec::new();
        if let Some(q) = query {
            for pair in q.split('&') {
                if pair.is_empty() {
                    continue;
                }
                let (name, value) = match pair.split_once('=') {
                    Some((n, v)) => (n, v),
                    None => return Err(RefError::Malformed("query parameter has no value")),
                };
                match name {
                    // `REF-R16`: absent and present-but-empty are different, and
                    // the second is malformed.
                    "hash" if value.is_empty() => {
                        return Err(RefError::EmptyParameter { name: "hash" })
                    }
                    "seen" if value.is_empty() => {
                        return Err(RefError::EmptyParameter { name: "seen" })
                    }
                    "via" if value.is_empty() => {
                        return Err(RefError::EmptyParameter { name: "via" })
                    }
                    "hash" => hash_param = Some(value),
                    "seen" => seen_param = Some(value),
                    "via" => {
                        let (tag, val) = value
                            .split_once(':')
                            .ok_or(RefError::Malformed("via hint has no tag separator"))?;
                        let tag = percent::decode(tag).map_err(|_| RefError::BadEscape)?;
                        let val = percent::decode(val).map_err(|_| RefError::BadEscape)?;
                        via.push(Hint {
                            tag: HintTag::from_token(&tag),
                            value: val.into_owned(),
                        });
                    }
                    // An unknown query parameter is ignored, the same MUST-ignore
                    // discipline the substrate applies to unknown fields.
                    _ => {}
                }
            }
        }

        let at = match fragment {
            Some(f) if !f.is_empty() => {
                let mut names = Vec::new();
                for seg in f.split('/') {
                    let decoded = percent::decode(seg).map_err(|_| RefError::BadEscape)?;
                    names.push(decoded.into_owned());
                }
                (!names.is_empty()).then_some(Anchor { field: names })
            }
            _ => None,
        };

        match (hash_param, has_path) {
            // `REF-R9` / `REF-V4`.
            (Some(_), true) => Err(RefError::BothIdentityTerms),
            // `REF-R10` / `REF-V5`.
            (None, false) => Err(RefError::NoIdentityTerm),
            (Some(h), false) => {
                let hash = hash_from_param(h, "hash")?;
                Ok(EntityRef::Pinned { peer: peer.to_string(), hash, at, via })
            }
            (None, true) => {
                let mut segments = Vec::new();
                for seg in raw_path.split('/') {
                    let decoded = percent::decode(seg).map_err(|_| RefError::BadEscape)?;
                    segments.push(decoded.into_owned());
                }
                let path = segments.join("/");
                // `REF-R15` — the refusal is applied here, to an ABSOLUTE-form
                // path, and nowhere near §3.4's relative resolution.
                refuse_dot_segments(&path)?;
                let seen = match seen_param {
                    Some(s) => Some(hash_from_param(s, "seen")?),
                    None => None,
                };
                Ok(EntityRef::Live { peer: peer.to_string(), path, seen, at, via })
            }
        }
    }
}

/// `REF-R11`: accept the scheme case-insensitively, without allocating on the
/// overwhelmingly common lowercase input.
fn strip_scheme(s: &str) -> Option<&str> {
    let n = SCHEME.len();
    if s.len() < n {
        return None;
    }
    s.get(..n)
        .filter(|head| head.eq_ignore_ascii_case(SCHEME))
        .map(|_| &s[n..])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real self-describing hash — never a hand-written 32 bytes, because a
    /// fixed width is the lock-in §2.1 spends a paragraph removing.
    fn h(seed: &str) -> Hash {
        Hash::compute("test/note", seed.as_bytes())
    }

    /// Mixed case on purpose: every assertion that uses it is also a
    /// `REF-V3` witness.
    const PEER: &str = "QmAbCdEfGhIjKlMnOpQrStUvWxYz";

    // -- the atom ---------------------------------------------------------

    #[test]
    fn a_pinned_atom_round_trips_through_cbor() {
        let r = EntityRef::pin(PEER, h("a"));
        assert_eq!(EntityRef::from_value(&r.to_value()).unwrap(), r);
    }

    #[test]
    fn a_live_atom_carrying_every_optional_term_round_trips_through_cbor() {
        let r = EntityRef::live(PEER, "/sites/labs/pages/post1")
            .with_seen(h("b"))
            .with_at(Anchor { field: vec!["body".into(), "title".into()] })
            .with_via(vec![
                Hint::new(HintTag::Origin, "https://example.test"),
                Hint::new(HintTag::Peer, "QmOther"),
            ]);
        assert_eq!(EntityRef::from_value(&r.to_value()).unwrap(), r);
    }

    /// `REF-R1` — there is no untagged atom.
    #[test]
    fn an_atom_with_no_tag_is_refused() {
        let v = Value::Map(vec![
            (text("peer"), text(PEER)),
            (text("hash"), ecf_bytes(h("a").to_bytes())),
        ]);
        assert_eq!(
            EntityRef::from_value(&v),
            Err(RefError::Malformed("reference has no tag"))
        );
    }

    /// `REF-R2` / `REF-V4`,`V5` — both arms, and they must not merge with
    /// *malformed*: a tag that disagrees with its identity term is the one
    /// defect two shapes exist to make visible.
    #[test]
    fn a_tag_that_disagrees_with_its_identity_term_is_refused_by_name() {
        let pin_no_hash = Value::Map(vec![(text("tag"), text(TAG_PIN)), (text("peer"), text(PEER))]);
        assert_eq!(
            EntityRef::from_value(&pin_no_hash),
            Err(RefError::TagIdentityMismatch { tag: TAG_PIN, missing: "hash" })
        );

        let live_no_path =
            Value::Map(vec![(text("tag"), text(TAG_LIVE)), (text("peer"), text(PEER))]);
        assert_eq!(
            EntityRef::from_value(&live_no_path),
            Err(RefError::TagIdentityMismatch { tag: TAG_LIVE, missing: "path" })
        );
    }

    /// The carve-out. **Not `Malformed`** — a well-formed body its own shape
    /// excludes is a schema violation, and reporting it as a corrupt byte
    /// routes it to the wrong person.
    #[test]
    fn a_shape_carrying_the_other_shapes_identity_term_is_a_schema_violation() {
        let pin_with_path = Value::Map(vec![
            (text("tag"), text(TAG_PIN)),
            (text("peer"), text(PEER)),
            (text("hash"), ecf_bytes(h("a").to_bytes())),
            (text("path"), text("/sites/labs")),
        ]);
        assert_eq!(
            EntityRef::from_value(&pin_with_path),
            Err(RefError::ExcludedField { tag: TAG_PIN, field: "path" })
        );

        let live_with_hash = Value::Map(vec![
            (text("tag"), text(TAG_LIVE)),
            (text("peer"), text(PEER)),
            (text("path"), text("/sites/labs")),
            (text("hash"), ecf_bytes(h("a").to_bytes())),
        ]);
        assert_eq!(
            EntityRef::from_value(&live_with_hash),
            Err(RefError::ExcludedField { tag: TAG_LIVE, field: "hash" })
        );
    }

    /// The carve-out is **narrow**: V7 §2.6's MUST-ignore still governs every
    /// field that is not the discriminator's own slot. Without this arm the
    /// refusal above would be an open-type violation wearing a rule's name.
    #[test]
    fn an_ordinary_unknown_field_is_still_ignored() {
        let v = Value::Map(vec![
            (text("tag"), text(TAG_PIN)),
            (text("peer"), text(PEER)),
            (text("hash"), ecf_bytes(h("a").to_bytes())),
            (text("something_a_later_version_added"), text("x")),
        ]);
        assert_eq!(EntityRef::from_value(&v).unwrap(), EntityRef::pin(PEER, h("a")));
    }

    /// `REF-R3` / `REF-V10` — its own word, because the divergence it prevents
    /// has no error at either end.
    #[test]
    fn a_peer_id_encoded_as_bytes_is_refused_and_says_why() {
        let v = Value::Map(vec![
            (text("tag"), text(TAG_PIN)),
            (text("peer"), ecf_bytes(vec![1, 2, 3])),
            (text("hash"), ecf_bytes(h("a").to_bytes())),
        ]);
        assert_eq!(EntityRef::from_value(&v), Err(RefError::PeerIdNotText));
    }

    /// **OURS, not theirs.** §2.2 says a third intent is a third tag, so an
    /// unrecognised one is an arm we have not built.
    #[test]
    fn an_unknown_tag_is_an_arm_we_have_not_built_and_not_a_corrupt_body() {
        let v = Value::Map(vec![
            (text("tag"), text("snapshot")),
            (text("peer"), text(PEER)),
            (text("hash"), ecf_bytes(h("a").to_bytes())),
        ]);
        assert_eq!(
            EntityRef::from_value(&v),
            Err(RefError::UnknownTag { tag: "snapshot".into() })
        );
    }

    /// `REF-R4` — the hash is self-describing and **not fixed-width**. A
    /// SHA-384 reference must survive a build whose default is SHA-256, or the
    /// atom has re-locked the cage hash agility removed.
    #[test]
    fn a_hash_of_a_non_default_width_survives_both_forms() {
        let wide = Hash::new(1, vec![7u8; 48]); // ecfv1-sha384
        assert_eq!(wide.to_bytes().len(), 49, "self-describing: code + digest");
        let r = EntityRef::pin(PEER, wide);
        assert_eq!(EntityRef::from_value(&r.to_value()).unwrap(), r);
        assert_eq!(EntityRef::parse_uri(&r.to_uri().unwrap()).unwrap(), r);
    }

    // -- hints ------------------------------------------------------------

    /// `REF-R7` — an unknown hint kind is carried, never a refusal.
    /// `REF-R5`'s structural half — and it is carried *and* excluded from what
    /// a resolver acts on, which is the pair that keeps a hint droppable.
    #[test]
    fn an_unknown_hint_kind_is_carried_and_never_acted_on() {
        let r = EntityRef::pin(PEER, h("a")).with_via(vec![
            Hint::new(HintTag::Other("carrier-pigeon".into()), "somewhere"),
            Hint::new(HintTag::Origin, "https://example.test"),
        ]);
        let decoded = EntityRef::from_value(&r.to_value()).unwrap();
        assert_eq!(decoded, r, "the unknown kind survives a round trip");
        assert_eq!(decoded.hints().len(), 2);
        let usable = decoded.hints_to_try();
        assert_eq!(usable.len(), 1, "a resolver acts only on kinds it knows");
        assert_eq!(usable[0].tag, HintTag::Origin);
    }

    /// The string form's half of the same rule.
    #[test]
    fn an_unknown_hint_kind_round_trips_through_the_string_form() {
        let r = EntityRef::pin(PEER, h("a"))
            .with_via(vec![Hint::new(HintTag::Other("carrier-pigeon".into()), "some where")]);
        let uri = r.to_uri().unwrap();
        assert!(uri.contains("via=carrier-pigeon:some%20where"), "{uri}");
        assert_eq!(EntityRef::parse_uri(&uri).unwrap(), r);
    }

    // -- absent versus empty ---------------------------------------------

    /// The string form has no spelling for present-and-empty, so **the encoder
    /// does not emit one either** — the wire cannot express what the string
    /// cannot. Asserted rather than described, because it is the one place this
    /// module deliberately narrows what §2.1's CDDL admits.
    ///
    /// **It asserts the KEY IS ABSENT, not that two atoms encode alike.** The
    /// first cut compared `with_empties.to_value()` against `bare.to_value()`,
    /// and an encoder that emitted `via: []` for *both* satisfied it — the
    /// neuter written to falsify this test came back green, and the cause was
    /// the third one: the neuter landed, and the assertion did not measure what
    /// its own name claimed.
    #[test]
    fn an_empty_via_and_an_empty_anchor_are_the_same_as_absent() {
        let bare = EntityRef::pin(PEER, h("a"));
        let with_empties = EntityRef::pin(PEER, h("a"))
            .with_via(vec![])
            .with_at(Anchor { field: vec![] });
        assert_eq!(with_empties, bare, "an empty anchor collapses to absent");

        let encoded = with_empties.to_value();
        let keys: Vec<String> = encoded
            .as_map()
            .expect("a reference encodes as a map")
            .iter()
            .filter_map(|(k, _)| k.as_text().map(str::to_string))
            .collect();
        assert!(!keys.contains(&"via".to_string()), "an empty via is not emitted: {keys:?}");
        assert!(!keys.contains(&"at".to_string()), "an empty anchor is not emitted: {keys:?}");
        assert_eq!(encoded, bare.to_value());
        assert_eq!(with_empties.to_uri().unwrap(), bare.to_uri().unwrap());

        // …and a body that arrived carrying them decodes to the same atom.
        let arrived = Value::Map(vec![
            (text("tag"), text(TAG_PIN)),
            (text("peer"), text(PEER)),
            (text("hash"), ecf_bytes(h("a").to_bytes())),
            (text("via"), Value::Array(vec![])),
            (
                text("at"),
                Value::Map(vec![(text("field"), Value::Array(vec![]))]),
            ),
        ]);
        assert_eq!(EntityRef::from_value(&arrived).unwrap(), bare);
    }

    // -- the string form --------------------------------------------------

    /// `REF-V1`.
    #[test]
    fn a_pinned_reference_round_trips_atom_to_string_to_atom() {
        let r = EntityRef::pin(PEER, h("a"));
        let uri = r.to_uri().unwrap();
        assert_eq!(uri, format!("{SCHEME}{PEER}/?hash={}", h("a").to_hex()));
        assert_eq!(EntityRef::parse_uri(&uri).unwrap(), r);
        assert_eq!(EntityRef::parse_uri(&uri).unwrap().to_uri().unwrap(), uri);
    }

    /// `REF-V2` — the query component carries three different kinds of term and
    /// is where they diverge.
    #[test]
    fn a_live_reference_with_seen_via_and_at_round_trips() {
        let r = EntityRef::live(PEER, "/sites/labs/pages/post1")
            .with_seen(h("b"))
            .with_at(Anchor { field: vec!["body".into()] })
            .with_via(vec![Hint::new(HintTag::Mirror, "https://mirror.test/x")]);
        let uri = r.to_uri().unwrap();
        assert_eq!(EntityRef::parse_uri(&uri).unwrap(), r);
        assert_eq!(EntityRef::parse_uri(&uri).unwrap().to_uri().unwrap(), uri);
    }

    /// **`REF-V3`, the highest-value vector here.** Every general URL library
    /// lowercases the authority by default; a peer id that survives such a
    /// parser names a *different peer*, and the failure is a clean 404 at a
    /// well-formed address — which reads as *not found* rather than as a bug.
    #[test]
    fn a_mixed_case_peer_id_survives_parse_and_reserialize_unchanged() {
        let r = EntityRef::pin(PEER, h("a"));
        let uri = r.to_uri().unwrap();
        assert!(uri.contains(PEER), "emitted verbatim: {uri}");
        let parsed = EntityRef::parse_uri(&uri).unwrap();
        assert_eq!(parsed.peer(), PEER, "the authority is CASE-SENSITIVE");
        assert_eq!(parsed.to_uri().unwrap(), uri);
    }

    /// `REF-R11` — case-insensitive on parse, lowercase on emit.
    #[test]
    fn the_scheme_is_case_insensitive_on_parse_and_lowercase_on_emit() {
        let upper = format!("ENTITY+REF://{PEER}/?hash={}", h("a").to_hex());
        let parsed = EntityRef::parse_uri(&upper).unwrap();
        assert_eq!(parsed, EntityRef::pin(PEER, h("a")));
        assert!(parsed.to_uri().unwrap().starts_with(SCHEME));
    }

    /// `REF-V4` and `REF-V5` — the discriminator is a rule, not a convention.
    #[test]
    fn a_string_with_both_identity_terms_or_neither_is_refused() {
        let both = format!("{SCHEME}{PEER}/sites/labs?hash={}", h("a").to_hex());
        assert_eq!(EntityRef::parse_uri(&both), Err(RefError::BothIdentityTerms));

        let neither = format!("{SCHEME}{PEER}/");
        assert_eq!(EntityRef::parse_uri(&neither), Err(RefError::NoIdentityTerm));
        assert_eq!(
            EntityRef::parse_uri(&format!("{SCHEME}{PEER}")),
            Err(RefError::NoIdentityTerm)
        );
    }

    /// `REF-V6` — the most common real content case: a page slug with a space
    /// or a `#`.
    #[test]
    fn a_path_segment_with_reserved_characters_round_trips_percent_encoded() {
        let r = EntityRef::live(PEER, "/sites/labs/pages/a note #2");
        let uri = r.to_uri().unwrap();
        assert!(uri.contains("/a%20note%20%232"), "{uri}");
        assert!(!uri.contains("#"), "a `#` inside a segment is not a fragment: {uri}");
        assert_eq!(EntityRef::parse_uri(&uri).unwrap(), r);
    }

    /// The delimiter is never encoded — the other half of `REF-R14`, and the
    /// arm that catches an encoder that escapes the whole path as one blob.
    #[test]
    fn the_path_delimiter_is_not_encoded() {
        let uri = EntityRef::live(PEER, "/a/b/c").to_uri().unwrap();
        assert_eq!(uri, format!("{SCHEME}{PEER}/a/b/c"));
    }

    /// `REF-V8`.
    #[test]
    fn a_dot_segment_in_an_absolute_form_path_is_refused() {
        assert_eq!(
            EntityRef::parse_uri(&format!("{SCHEME}{PEER}/sites/../labs")),
            Err(RefError::DotSegment)
        );
        assert_eq!(
            EntityRef::parse_uri(&format!("{SCHEME}{PEER}/sites/./labs")),
            Err(RefError::DotSegment)
        );
        // …and we cannot emit one either.
        assert_eq!(
            EntityRef::live(PEER, "/sites/../labs").to_uri(),
            Err(RefError::DotSegment)
        );
    }

    /// **`REF-V9` — the arm that catches over-application of `REF-V8`.**
    /// §3.4's relative form is directory-relative, so `..` is meaningful and
    /// expected there. The refusal lives on the absolute-form parser and
    /// nowhere near [`crate::content_site::location::classify_link`], which is
    /// this repo's single home for relative resolution — so an ordinary correct
    /// link is untouched. Asserted here so a later author cannot "tidy" the
    /// refusal upward into the classifier.
    #[test]
    fn a_relative_link_containing_dot_dot_is_not_this_modules_business() {
        use crate::content_site::location::{classify_link, LinkTarget, Location};
        let current = Location {
            peer_id: Some(PEER.into()),
            site_id: "lab".into(),
            page: "research/model/grounding".into(),
        };
        assert_eq!(
            classify_link("../notes/x.md", &current),
            LinkTarget::InSite { page: "research/notes/x".into() },
            "a relative `..` resolves; it is not an absolute-form path"
        );
        // And the same string is not a reference URI at all — `REF-R20`.
        assert_eq!(
            EntityRef::parse_uri("../notes/x.md"),
            Err(RefError::NotAReferenceUri)
        );
    }

    /// `REF-R16` — absent and present-but-empty are different facts.
    #[test]
    fn a_parameter_present_with_an_empty_value_is_refused() {
        assert_eq!(
            EntityRef::parse_uri(&format!("{SCHEME}{PEER}/?hash=")),
            Err(RefError::EmptyParameter { name: "hash" })
        );
        assert_eq!(
            EntityRef::parse_uri(&format!("{SCHEME}{PEER}/sites/labs?seen=")),
            Err(RefError::EmptyParameter { name: "seen" })
        );
        assert_eq!(
            EntityRef::parse_uri(&format!("{SCHEME}{PEER}/sites/labs?via=")),
            Err(RefError::EmptyParameter { name: "via" })
        );
    }

    /// **The authority is the term that makes a reference routable at all**, so
    /// its absence is its own refusal rather than a generic parse failure.
    #[test]
    fn a_string_with_no_authority_is_refused() {
        assert_eq!(
            EntityRef::parse_uri(&format!("{SCHEME}/sites/labs")),
            Err(RefError::NoAuthority)
        );
        // `entity+ref:` without `//` is not the scheme at all.
        assert_eq!(
            EntityRef::parse_uri("entity+ref:sites/labs"),
            Err(RefError::NotAReferenceUri)
        );
    }

    /// `REF-R20` — an unparseable reference **leaves the system**; it is never
    /// re-anchored.
    #[test]
    fn a_string_that_is_not_a_reference_uri_is_classified_not_guessed() {
        for s in ["https://example.test/x", "entity://PEER/sites/x", "site:labs/intro", ""] {
            assert_eq!(
                EntityRef::parse_uri(s),
                Err(RefError::NotAReferenceUri),
                "{s:?} must be classified as leaving the system"
            );
        }
    }

    #[test]
    fn a_hash_parameter_that_is_not_a_hash_is_refused() {
        for bad in ["zz", "abc", "00"] {
            assert!(
                matches!(
                    EntityRef::parse_uri(&format!("{SCHEME}{PEER}/?hash={bad}")),
                    Err(RefError::BadHash { param: "hash" })
                ),
                "{bad:?} must not parse as a content hash"
            );
        }
    }

    #[test]
    fn a_malformed_percent_escape_is_refused_rather_than_tolerated() {
        assert_eq!(
            EntityRef::parse_uri(&format!("{SCHEME}{PEER}/sites/%zz")),
            Err(RefError::BadEscape)
        );
    }

    /// A `#` before a `?` makes the `?` part of the fragment (RFC 3986 §3).
    /// Splitting in the wrong order silently moves a query term into an anchor
    /// field name.
    #[test]
    fn the_fragment_is_split_before_the_query() {
        let r = EntityRef::live(PEER, "/sites/labs")
            .with_at(Anchor { field: vec!["a?b".into()] });
        let uri = r.to_uri().unwrap();
        assert!(uri.ends_with("#a%3Fb"), "{uri}");
        assert_eq!(EntityRef::parse_uri(&uri).unwrap(), r);
    }

    /// An anchor field name containing the `/` join delimiter is data, not
    /// structure — the same rule as a path segment.
    #[test]
    fn an_anchor_field_name_containing_a_slash_round_trips() {
        let r = EntityRef::pin(PEER, h("a"))
            .with_at(Anchor { field: vec!["a/b".into(), "c".into()] });
        let uri = r.to_uri().unwrap();
        assert!(uri.ends_with("#a%2Fb/c"), "{uri}");
        assert_eq!(EntityRef::parse_uri(&uri).unwrap(), r);
    }

    /// `EntityRef::live` normalizes so our own atoms round-trip exactly. Stated
    /// as a test because it is a narrowing of what §2.1's `tree-path` admits —
    /// see the module doc.
    #[test]
    fn a_live_path_is_normalized_to_a_leading_slash() {
        let a = EntityRef::live(PEER, "sites/labs");
        let b = EntityRef::live(PEER, "/sites/labs");
        assert_eq!(a, b);
        assert_eq!(EntityRef::parse_uri(&a.to_uri().unwrap()).unwrap(), a);
    }

    /// The query order is canonical, because byte-identical re-serialization
    /// needs one and §3 states none. If arch names a different order this test
    /// is where it changes.
    #[test]
    fn the_query_order_is_canonical_identity_term_then_hints() {
        let r = EntityRef::live(PEER, "/x")
            .with_seen(h("b"))
            .with_via(vec![Hint::new(HintTag::Origin, "o"), Hint::new(HintTag::Peer, "p")]);
        let uri = r.to_uri().unwrap();
        let query = uri.split_once('?').unwrap().1;
        let names: Vec<&str> = query.split('&').map(|p| p.split_once('=').unwrap().0).collect();
        assert_eq!(names, vec!["seen", "via", "via"]);
        // Hints keep their order — §2.3: descending confidence.
        assert_eq!(EntityRef::parse_uri(&uri).unwrap(), r);
    }

    /// **The peer root is not an address, and admitting it left the wire able
    /// to express something the string form cannot.**
    ///
    /// The module doc states the rule — *"we collapse both to absent in the
    /// encoder as well as the decoder, so the wire cannot express what the
    /// string cannot"* — and `path: "/"` was the instance it did not cover:
    /// `from_value` refused only `""`, so a `live` atom naming the peer root
    /// decoded fine, `to_uri` rendered it `entity+ref://{peer}/`, and
    /// `parse_uri` then correctly read that back as *no identity term at all*.
    /// §3.2 makes the round trip a **[MUST]** in both directions, so the atom
    /// that cannot survive it is the one that must not exist.
    ///
    /// Asserted from **both ends**, because each has a different wrong
    /// implementation: a decoder that admits it (a foreign entry hands us one)
    /// and an emitter that renders it (our own `live(peer, "")` did).
    #[test]
    fn the_peer_root_is_not_a_live_address() {
        let root = Value::Map(vec![
            (text("tag"), text(TAG_LIVE)),
            (text("peer"), text(PEER)),
            (text("path"), text("/")),
        ]);
        assert_eq!(
            EntityRef::from_value(&root),
            Err(RefError::TagIdentityMismatch { tag: TAG_LIVE, missing: "path" }),
            "a live atom naming the peer root must not decode"
        );

        // The emitter half — constructed directly, since the decoder above can
        // no longer produce one.
        let emitted = EntityRef::Live {
            peer: PEER.to_string(),
            path: "/".into(),
            seen: None,
            at: None,
            via: Vec::new(),
        };
        assert_eq!(
            emitted.to_uri(),
            Err(RefError::TagIdentityMismatch { tag: TAG_LIVE, missing: "path" }),
            "and must not be rendered into a string that cannot be read back"
        );
    }

    /// §5's floor: *"an atom with no `via` and no `at` is the whole floor. A
    /// peer that resolves only pinned references, only within its own
    /// namespace, and ignores every hint, is a valid participant — not a
    /// degraded one."*
    #[test]
    fn the_floor_is_a_bare_pin() {
        let r = EntityRef::pin(PEER, h("a"));
        assert!(r.anchor().is_none());
        assert!(r.hints().is_empty());
        assert_eq!(EntityRef::from_value(&r.to_value()).unwrap(), r);
        assert_eq!(EntityRef::parse_uri(&r.to_uri().unwrap()).unwrap(), r);
    }
}
