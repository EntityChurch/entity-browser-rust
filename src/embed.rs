//! `APP-CONVENTION-EMBED` §3 — the `Embed` **input** entity and its inline
//! form, `embed-node`.
//!
//! ## Which "embed" this is
//!
//! There are two in this crate and they are different things, deliberately:
//!
//! - **This module** is the convention's §3 input surface — the `(type, data)`
//!   pair, the tagged [`EmbedPayload`] union, `fallback`, `renditions`. It is
//!   the shared vocabulary, and `APP-CONVENTION-FEED` §2.3's `body` **is** one
//!   of these, carried inline.
//! - [`crate::content_site::embed`] is the SITE convention's **inline-directive
//!   grammar** (`::embed[fallback]{ref=…}` in a markdown body). §3 says that
//!   grammar *"belongs to the consuming convention … NOT to EMBED"*, so its
//!   home is correct — see the ⚠ below for the MUST that joins them and that
//!   we do not satisfy.
//!
//! ## `embed-node` is the INPUT surface and never `EmbedOutput`
//!
//! §3.1's warning is the one a consuming convention gets wrong: **an entry
//! stores what was authored, and the handler runs at the reader.** Typing a
//! stored field as §4's `EmbedOutput` would fix the rendition choice at
//! authoring time for every reader forever, leave §5's `{media_type → handler}`
//! dispatch nothing to run, and — because §4's vocabulary is **closed by
//! design** — publish a wire format that can never carry a content kind that
//! vocabulary did not anticipate. The input surface is open on purpose.
//!
//! **A node and an entity differ in ADDRESSING only**, and the dispatch key is
//! carried rather than dropped: §3 puts the media type in the *type tag* rather
//! than in `data` so there is one source of truth, and an inline node keeps
//! that property where a bare `embed-data` would lose it. *"A handler that
//! accepts one accepts the other, and an implementation MUST NOT make
//! behaviour depend on which form the embed arrived in."*
//!
//! ## The payload union has THREE arms and this is its one home
//!
//! [`EmbedPayload`] was already implemented here — once, in the wrong place, as
//! `content_site::format::AssetPayload`: named for assets, codec-private, and
//! carrying a union that belongs to this convention. That file's own CDDL note
//! said so (*"NOT a second payload union; the embed one, reused"*) while the
//! code restated it. **This is a move, not a rewrite** — the encoding is
//! unchanged for both arms `app/site-asset` admits, and the proof is
//! `EXPECTED-INGEST.json` byte-identical plus `make crossimpl-site` AGREED.
//!
//! **The `child` arm is real here and refused by the importing field.** `A-30`
//! ruled that `app/site-asset` does not admit it (SITE v0.5.1 §4, `[MUST NOT]`)
//! — so the shared union carries three arms and the site layer says *"invalid
//! for its type"*, which is the shape a carve-out is supposed to take. It only
//! became buildable now: `child-payload.ref` is an [`EntityRef`], and until the
//! reference atom had a home this arm could not be written at all.
//!
//! ## Two rules that fight §2.6, and each needs its own refusal
//!
//! V7 §2.6 makes unknown fields MUST-ignore. §3 carves two exceptions out of
//! it, and *"where your type carves an exception out of a rule you otherwise
//! satisfy, the exception needs its own refusal and its own named check"*
//! (`GUIDE-APPLICATION-DEVELOPMENT` §3, the binding table — a rule this seat
//! supplied the third instance for, one convention over):
//!
//! 1. **`params` keys are STRINGS ONLY (no int keys).** An integer key is not
//!    an unknown field to skip; it is a `params` bag that cannot be read the
//!    way the convention says to read it. [`EmbedError::NonStringParamKey`].
//! 2. **A v0.2 consumer MUST REFUSE TO RENDER an embed carrying a non-empty
//!    `requires` or `sandbox`** — passive-only at the format layer, *"prevents
//!    a partial-honor impl silently rendering with declared-but-unenforced
//!    caps."* Skipping them as unknown fields is precisely the partial honour
//!    that rule excludes. [`EmbedNode::render_verdict`].
//!
//! §7 is `[OPEN per G1]`, so those two fields are **carried opaquely and
//! re-emitted** rather than modelled — modelling an open section would be
//! building against a draft, and dropping them would make a re-encode rewrite
//! somebody else's declaration.
//!
//! ## ⚠ What we do NOT satisfy, stated rather than discovered later
//!
//! §3's embedding-modes note carries a MUST **that names this repo in its own
//! justification**: *"an inline directive MUST lower to a `child` payload; it
//! is sugar, not a parallel format (workbench-go/entity-browser-rust
//! round-trip pin: 'edit in entity-browser-rust, view in workbench' requires
//! the directive and the child entity be the same thing)."*
//!
//! **Ours does not lower to anything.** `content_site::embed`'s directive
//! carries a site-relative asset path resolved against `app/site-asset`, we
//! mint no `Embed` entity at all, and there is therefore no `child` payload for
//! the directive to *be*. That is a parallel format for the same job, which is
//! the thing the MUST excludes — and it is the sharp form of what we routed as
//! `A-24` (every `ref` we ship is a third form `F-1` does not classify).
//! **`vocab-lint` cannot see this**: a *missing* emission is not a divergent
//! tag, so nothing goes red anywhere. Routed, not fixed here — the repair is a
//! design question about whether SITE's asset model becomes `Embed` entities,
//! and it is not phase 2a's to answer.

#![allow(dead_code)] // the FEED types that carry a node land in phase 2a's next slice

use std::collections::BTreeMap;

use entity_ecf::{bytes as ecf_bytes, text, to_ecf, Value};
use entity_entity::Entity;
use entity_hash::Hash;

use crate::entity_ref::{EntityRef, RefError};

/// `app/embed/{media_type}` — §3's type-tag prefix, which **is** the dispatch
/// key. There is no `data.media_type`; it would be *"a redundant second source
/// of truth"*, and that sentence is also why `app/site-asset` may not carry a
/// `child` payload (it *does* have a `data.media_type`, so the two could
/// disagree).
pub const EMBED_TYPE_PREFIX: &str = "app/embed/";

/// `inline-payload = { tag: "inline", bytes: bstr .size (1..16384) }`.
///
/// **A RANGE, not a ceiling** — the lower bound is 1, so a zero-byte body is
/// outside it in the same way an oversized one is, and both take the `pointer`
/// arm. §3.1: *"the inline bound applies unchanged … a property of the payload
/// and not of the addressing"*, which is why it lives with the union rather
/// than with either carrier.
pub const INLINE_PAYLOAD_MAX: usize = 16384;

// ---------------------------------------------------------------------------
// The payload union
// ---------------------------------------------------------------------------

/// §3's `embed-payload` — **TAGGED; a decoder MUST reject an untagged or
/// ambiguous payload** (the silent-divergence risk `G-PIN-2` closes).
///
/// Three arms, and only one of them may cross a peer boundary. `inline` carries
/// bytes; `pointer` names a blob in the **resolving peer's own** content store
/// — *"the implied-authority form of the atom, exactly as REFERENCE §3.4's
/// `site:` is"* — and adding a required `peer` to it would restate a term that
/// is already known and invite it to be wrong. `child` is the exception: it
/// names something on **another** peer, so it is the one that carries the
/// authority term, and that is why it is an [`EntityRef`] where the other two
/// are not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbedPayload {
    /// In-tree bytes, within [`INLINE_PAYLOAD_MAX`].
    Inline(Vec<u8>),
    /// A `system/content` blob in the resolving peer's own store. Same-peer by
    /// design.
    Pointer(Hash),
    /// Entity-native transclusion of a sibling `Embed`. **The only payload that
    /// may cross a peer boundary**, hence the reference.
    Child(EntityRef),
}

/// Why a payload was refused. **Each names whose defect it is** — the
/// discipline `A-30` cost us, applied on the way in rather than after.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayloadError {
    /// **Theirs, malformed.** No `tag` at all — §3's MUST, and the one refusal
    /// that is about a *missing discriminator* rather than about which arm
    /// arrived.
    Untagged,
    /// **OURS.** A tagged arm this build does not implement, i.e. a genuinely
    /// newer producer against an older reader. Naming our own gap is the
    /// correct attribution here, and it is *not* the same statement as
    /// [`Untagged`](Self::Untagged).
    UnknownTag { tag: String },
    /// **Theirs, malformed.** The right tag, and the arm's own required term is
    /// missing or ill-typed.
    Malformed { tag: &'static str, what: &'static str },
    /// **Theirs.** A `child` payload whose reference is refused. Carries the
    /// reference's own outcome, so a caller can still tell *"an intent we have
    /// not built"* from *"a broken atom"* one layer down.
    BadRef { source: RefError },
}

impl std::fmt::Display for PayloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PayloadError::Untagged => write!(f, "embed payload carries no tag"),
            PayloadError::UnknownTag { tag } => {
                write!(f, "embed payload tag {tag:?} is an arm this build does not implement")
            }
            PayloadError::Malformed { tag, what } => {
                write!(f, "{tag} payload: {what}")
            }
            PayloadError::BadRef { source } => write!(f, "child payload reference: {source}"),
        }
    }
}

impl EmbedPayload {
    /// Whether authored bytes of this length may take the `inline` arm — §3's
    /// `bstr .size (1..16384)`, **read as a range**: above the ceiling and
    /// below the floor are both outside it, and both owe a pointer.
    ///
    /// **This is the AUTHORING side of the bound and deliberately not the
    /// decoding side**, which is a distinction the site convention paid for
    /// twice. Authoring is turning bytes into a payload for the first time, and
    /// there the range is a `[MUST NOT]` we control. **Transcribing is not**:
    /// re-encoding a foreign asset we already decoded — the durable cache's
    /// write-through — must not start refusing bytes it can render, because a
    /// cache that drops what it cannot re-verify turns an outage into a missing
    /// figure (D24). See
    /// `asset_store::a_legacy_oversized_asset_heals_into_a_shape_that_is_declared_and_a_size_that_is_not`.
    ///
    /// One expression, because there were about to be two: `stage` carried the
    /// range inline for `app/site-asset`, and every later carrier — a FEED
    /// entry body first — needs the same one.
    pub fn inline_is_conformant(len: usize) -> bool {
        (1..=INLINE_PAYLOAD_MAX).contains(&len)
    }

    /// The wire tag of the arm.
    pub fn tag(&self) -> &'static str {
        match self {
            EmbedPayload::Inline(_) => "inline",
            EmbedPayload::Pointer(_) => "pointer",
            EmbedPayload::Child(_) => "child",
        }
    }

    /// Encode to the tagged CBOR map.
    ///
    /// **This is the encoding `app/site-asset` has shipped since the pointer
    /// arm landed** — moving the union here must not move a byte, and the
    /// site's `EXPECTED-INGEST.json` is the instrument that says so.
    pub fn to_value(&self) -> Value {
        match self {
            EmbedPayload::Inline(bytes) => Value::Map(vec![
                (text("tag"), text("inline")),
                (text("bytes"), ecf_bytes(bytes.clone())),
            ]),
            EmbedPayload::Pointer(hash) => Value::Map(vec![
                (text("tag"), text("pointer")),
                (text("hash"), ecf_bytes(hash.to_bytes())),
            ]),
            EmbedPayload::Child(r) => Value::Map(vec![
                (text("tag"), text("child")),
                (text("ref"), r.to_value()),
            ]),
        }
    }

    /// Decode from the tagged CBOR map.
    ///
    /// **An untagged map that happens to carry `bytes` is refused**, not read —
    /// that is the pre-declaration shape this repo once emitted and exactly
    /// what §3 requires a decoder to reject. The distinction between that and
    /// an unknown tag is kept, because one is a malformed payload and the other
    /// is our own missing arm.
    pub fn from_value(v: &Value) -> Result<Self, PayloadError> {
        let map = v.as_map().ok_or(PayloadError::Untagged)?;
        let field = |name: &str| map.iter().find(|(k, _)| k.as_text() == Some(name)).map(|(_, v)| v);

        let tag = field("tag")
            .and_then(|v| v.as_text())
            .ok_or(PayloadError::Untagged)?;

        match tag {
            "inline" => {
                let bytes = field("bytes").and_then(|v| v.as_bytes()).ok_or(
                    PayloadError::Malformed { tag: "inline", what: "no bytes" },
                )?;
                Ok(EmbedPayload::Inline(bytes.clone()))
            }
            "pointer" => {
                // A malformed hash is a refusal, never a defaulted one: a
                // pointer we cannot address is not a pointer to nothing, it is
                // bytes we must not claim to hold.
                let raw = field("hash").and_then(|v| v.as_bytes()).ok_or(
                    PayloadError::Malformed { tag: "pointer", what: "no hash" },
                )?;
                let hash = Hash::from_bytes(raw).map_err(|_| PayloadError::Malformed {
                    tag: "pointer",
                    what: "hash is not a content hash",
                })?;
                Ok(EmbedPayload::Pointer(hash))
            }
            "child" => {
                let r = field("ref").ok_or(PayloadError::Malformed {
                    tag: "child",
                    what: "no ref",
                })?;
                EntityRef::from_value(r)
                    .map(EmbedPayload::Child)
                    .map_err(|source| PayloadError::BadRef { source })
            }
            other => Err(PayloadError::UnknownTag { tag: other.to_string() }),
        }
    }
}

// ---------------------------------------------------------------------------
// embed-data / embed-node
// ---------------------------------------------------------------------------

/// §3's `rendition` — a responsive/format variant, lazily dereferenced.
///
/// **Selection happens at the reader** (§5.3), which is the same reason a
/// stored field is the input surface and not `EmbedOutput`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendition {
    pub pointer: Hash,
    pub media_type: String,
    pub capability_tags: Vec<String>,
}

/// §3's `embed-data`.
///
/// **No `Eq`, and that is the open `params` bag being honest:** its values are
/// arbitrary ECF values, and a CBOR float has no total equality. `PartialEq` is
/// what the wire admits.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbedData {
    pub payload: EmbedPayload,
    /// **MANDATORY and non-empty** — authored text degradation (§6's ladder,
    /// §8's anti-graveyard rule). An embed nobody can render is a hole in a
    /// document; the fallback is what keeps it a sentence.
    pub fallback: String,
    /// An open attribute bag. **Keys are strings only** — see the module doc.
    pub params: BTreeMap<String, Value>,
    pub renditions: Vec<Rendition>,
    /// §7, `[OPEN per G1]` — carried opaquely, never modelled. A **non-empty**
    /// value makes the embed unrenderable in v0.2; see
    /// [`EmbedNode::render_verdict`].
    pub requires: Option<Value>,
    /// §7, `[OPEN per G1]` — as `requires`.
    pub sandbox: Option<Value>,
}

impl EmbedData {
    /// The floor: a payload and a fallback, which is every text and image post.
    pub fn new(payload: EmbedPayload, fallback: impl Into<String>) -> Self {
        Self {
            payload,
            fallback: fallback.into(),
            params: BTreeMap::new(),
            renditions: Vec::new(),
            requires: None,
            sandbox: None,
        }
    }
}

/// §3.1's `embed-node` — **the same `(type, data)` pair, inline and not
/// separately addressed.**
///
/// Carries the **media type**, not the whole tag, because the tag *is*
/// `app/embed/` + the media type and storing both would be the second source
/// of truth §3 removed. [`Self::type_name`] rebuilds it.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbedNode {
    pub media_type: String,
    pub data: EmbedData,
}

/// What a v0.2 consumer may do with a node it decoded.
///
/// Two outcomes rather than a `bool`, because *"we will not render this"* has
/// to be able to say why — the one thing a partial-honour implementation never
/// does is announce that it is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderVerdict {
    Renderable,
    /// §3's passive-only rule: a non-empty `requires` or `sandbox` declares an
    /// **active** embed, and v0.2 has no enforcement to honour it with.
    RefuseActive { declaration: &'static str },
}

/// Why a node was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbedError {
    /// The type tag is not `app/embed/{media_type}`. **Not an embed at all** —
    /// the same first question `decode_share` asks, and for the same reason: a
    /// window-id-shaped slot or a mirrored foreign subtree can hand you
    /// anything.
    NotAnEmbed { entity_type: String },
    /// An `app/embed/` tag with nothing after it. The dispatch key is the whole
    /// point of the tag, so an empty one is not a media type.
    EmptyMediaType,
    /// Unreadable body, or a required field missing or ill-typed.
    Malformed(&'static str),
    /// No `fallback`. §3: mandatory.
    MissingFallback,
    /// A `fallback` that is present and empty. §3: non-empty. **Kept apart from
    /// [`MissingFallback`](Self::MissingFallback)** — one is a producer that
    /// does not know about the degradation ladder, the other is one that knows
    /// and had nothing to say, and only the second is worth telling an author
    /// about.
    EmptyFallback,
    /// A `params` key that is not a text string. §3's carve-out out of §2.6 —
    /// see the module doc.
    NonStringParamKey,
    /// The payload's own refusal, carried through.
    Payload(PayloadError),
}

impl std::fmt::Display for EmbedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EmbedError::NotAnEmbed { entity_type } => write!(f, "not an embed: {entity_type}"),
            EmbedError::EmptyMediaType => write!(f, "embed type carries no media type"),
            EmbedError::Malformed(what) => write!(f, "malformed embed: {what}"),
            EmbedError::MissingFallback => write!(f, "embed has no fallback"),
            EmbedError::EmptyFallback => write!(f, "embed fallback is empty"),
            EmbedError::NonStringParamKey => write!(f, "embed params carries a non-string key"),
            EmbedError::Payload(e) => write!(f, "{e}"),
        }
    }
}

fn rendition_value(r: &Rendition) -> Value {
    Value::Map(vec![
        (text("pointer"), ecf_bytes(r.pointer.to_bytes())),
        (text("media_type"), text(r.media_type.clone())),
        (
            text("capability_tags"),
            Value::Array(r.capability_tags.iter().map(|t| text(t.clone())).collect()),
        ),
    ])
}

fn decode_rendition(v: &Value) -> Result<Rendition, EmbedError> {
    let map = v.as_map().ok_or(EmbedError::Malformed("rendition is not a map"))?;
    let field = |name: &str| map.iter().find(|(k, _)| k.as_text() == Some(name)).map(|(_, v)| v);
    let raw = field("pointer")
        .and_then(|v| v.as_bytes())
        .ok_or(EmbedError::Malformed("rendition has no pointer"))?;
    let pointer =
        Hash::from_bytes(raw).map_err(|_| EmbedError::Malformed("rendition pointer is not a hash"))?;
    let media_type = field("media_type")
        .and_then(|v| v.as_text())
        .ok_or(EmbedError::Malformed("rendition has no media_type"))?
        .to_string();
    let capability_tags = match field("capability_tags") {
        Some(v) => {
            let list = v
                .as_array()
                .ok_or(EmbedError::Malformed("rendition capability_tags is not an array"))?;
            let mut tags = Vec::with_capacity(list.len());
            for t in list {
                tags.push(
                    t.as_text()
                        .ok_or(EmbedError::Malformed("rendition capability tag is not text"))?
                        .to_string(),
                );
            }
            tags
        }
        None => Vec::new(),
    };
    Ok(Rendition { pointer, media_type, capability_tags })
}

/// A `Value` that is present and carries nothing — the difference between
/// *declared* and *declared empty*, which §3's passive-only rule turns on.
fn is_empty_declaration(v: &Value) -> bool {
    match v {
        Value::Array(a) => a.is_empty(),
        Value::Map(m) => m.is_empty(),
        Value::Null => true,
        _ => false,
    }
}

impl EmbedNode {
    pub fn new(media_type: impl Into<String>, data: EmbedData) -> Self {
        Self { media_type: media_type.into(), data }
    }

    /// `app/embed/{media_type}` — the dispatch key, rebuilt rather than stored.
    pub fn type_name(&self) -> String {
        format!("{EMBED_TYPE_PREFIX}{}", self.media_type)
    }

    /// §3's passive-only rule, as an outcome rather than a silent skip.
    pub fn render_verdict(&self) -> RenderVerdict {
        for (name, slot) in [("requires", &self.data.requires), ("sandbox", &self.data.sandbox)] {
            if let Some(v) = slot {
                if !is_empty_declaration(v) {
                    return RenderVerdict::RefuseActive { declaration: name };
                }
            }
        }
        RenderVerdict::Renderable
    }

    /// The `embed-data` map — the `data` half of the node.
    pub fn data_value(&self) -> Value {
        let d = &self.data;
        let mut fields = vec![
            (text("payload"), d.payload.to_value()),
            (text("fallback"), text(d.fallback.clone())),
        ];
        if !d.params.is_empty() {
            fields.push((
                text("params"),
                Value::Map(d.params.iter().map(|(k, v)| (text(k.clone()), v.clone())).collect()),
            ));
        }
        if !d.renditions.is_empty() {
            fields.push((
                text("renditions"),
                Value::Array(d.renditions.iter().map(rendition_value).collect()),
            ));
        }
        // Carried, never authored by us — see the module doc on §7 being open.
        if let Some(v) = &d.requires {
            fields.push((text("requires"), v.clone()));
        }
        if let Some(v) = &d.sandbox {
            fields.push((text("sandbox"), v.clone()));
        }
        Value::Map(fields)
    }

    /// §3.1's inline form: `{ type, data }`, the same pair an addressed `Embed`
    /// carries.
    pub fn to_value(&self) -> Value {
        Value::Map(vec![
            (text("type"), text(self.type_name())),
            (text("data"), self.data_value()),
        ])
    }

    /// The addressed form — an `Embed` **entity**. *"A node and an entity
    /// differ in addressing only"*, so this is the same `data`, hashed under
    /// the same tag.
    pub fn to_entity(&self) -> Result<Entity, String> {
        Entity::new(&self.type_name(), to_ecf(&self.data_value()))
            .map_err(|e| format!("embed entity: {e}"))
    }

    /// Decode the inline form.
    pub fn from_value(v: &Value) -> Result<Self, EmbedError> {
        let map = v.as_map().ok_or(EmbedError::Malformed("embed node is not a map"))?;
        let field = |name: &str| map.iter().find(|(k, _)| k.as_text() == Some(name)).map(|(_, v)| v);
        let type_name = field("type")
            .and_then(|v| v.as_text())
            .ok_or(EmbedError::Malformed("embed node has no type"))?;
        let data = field("data").ok_or(EmbedError::Malformed("embed node has no data"))?;
        Self::from_parts(type_name, data)
    }

    /// Decode the addressed form. **Same code path**, because §3.1 says a
    /// handler that accepts one accepts the other and an implementation MUST
    /// NOT make behaviour depend on which form it arrived in — so the two entry
    /// points differ only in where they read the pair from.
    pub fn from_entity(entity: &Entity) -> Result<Self, EmbedError> {
        let data: Value = ciborium::from_reader(entity.data.as_slice())
            .map_err(|_| EmbedError::Malformed("embed body is not CBOR"))?;
        Self::from_parts(&entity.entity_type, &data)
    }

    fn from_parts(type_name: &str, data: &Value) -> Result<Self, EmbedError> {
        let media_type = type_name.strip_prefix(EMBED_TYPE_PREFIX).ok_or_else(|| {
            EmbedError::NotAnEmbed { entity_type: type_name.to_string() }
        })?;
        if media_type.is_empty() {
            return Err(EmbedError::EmptyMediaType);
        }
        let map = data.as_map().ok_or(EmbedError::Malformed("embed data is not a map"))?;
        let field = |name: &str| map.iter().find(|(k, _)| k.as_text() == Some(name)).map(|(_, v)| v);

        let payload = EmbedPayload::from_value(
            field("payload").ok_or(EmbedError::Malformed("embed has no payload"))?,
        )
        .map_err(EmbedError::Payload)?;

        let fallback = match field("fallback") {
            None => return Err(EmbedError::MissingFallback),
            Some(v) => v
                .as_text()
                .ok_or(EmbedError::Malformed("embed fallback is not text"))?,
        };
        if fallback.is_empty() {
            return Err(EmbedError::EmptyFallback);
        }

        let mut params = BTreeMap::new();
        if let Some(v) = field("params") {
            let entries = v
                .as_map()
                .ok_or(EmbedError::Malformed("embed params is not a map"))?;
            for (k, val) in entries {
                // §3's carve-out out of §2.6 — an int key is not an unknown
                // field to skip, it is a bag that cannot be read as specified.
                let key = k.as_text().ok_or(EmbedError::NonStringParamKey)?;
                params.insert(key.to_string(), val.clone());
            }
        }

        let mut renditions = Vec::new();
        if let Some(v) = field("renditions") {
            let list = v
                .as_array()
                .ok_or(EmbedError::Malformed("embed renditions is not an array"))?;
            for item in list {
                renditions.push(decode_rendition(item)?);
            }
        }

        Ok(EmbedNode {
            media_type: media_type.to_string(),
            data: EmbedData {
                payload,
                fallback: fallback.to_string(),
                params,
                renditions,
                requires: field("requires").cloned(),
                sandbox: field("sandbox").cloned(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity_ref::EntityRef;

    fn h(seed: &str) -> Hash {
        Hash::compute("test/note", seed.as_bytes())
    }

    const PEER: &str = "QmAbCdEfGhIjKlMnOpQrStUvWxYz";

    fn text_node() -> EmbedNode {
        EmbedNode::new(
            "text/plain",
            EmbedData::new(EmbedPayload::Inline(b"hello".to_vec()), "hello"),
        )
    }

    // -- the payload union ------------------------------------------------

    /// All three arms, including the one that could not be written until the
    /// reference atom had a home.
    #[test]
    fn every_payload_arm_round_trips() {
        for payload in [
            EmbedPayload::Inline(b"<svg/>".to_vec()),
            EmbedPayload::Pointer(h("blob")),
            EmbedPayload::Child(EntityRef::pin(PEER, h("sibling"))),
        ] {
            assert_eq!(
                EmbedPayload::from_value(&payload.to_value()).unwrap(),
                payload,
                "{} did not round trip",
                payload.tag()
            );
        }
    }

    /// §3's MUST, and the arm it must not merge with. **An untagged map that
    /// carries `bytes` is the pre-declaration shape this repo once emitted**,
    /// so a decoder that read it would re-admit exactly the divergence
    /// `G-PIN-2` closes.
    #[test]
    fn an_untagged_payload_is_refused_and_is_not_an_unknown_tag() {
        let untagged = Value::Map(vec![(text("bytes"), ecf_bytes(b"x".to_vec()))]);
        assert_eq!(EmbedPayload::from_value(&untagged), Err(PayloadError::Untagged));

        let unknown = Value::Map(vec![
            (text("tag"), text("stream")),
            (text("url"), text("https://x.test")),
        ]);
        assert_eq!(
            EmbedPayload::from_value(&unknown),
            Err(PayloadError::UnknownTag { tag: "stream".into() }),
            "an arm we have not built is OURS; an untagged payload is theirs"
        );
    }

    /// A pointer we cannot address is not a pointer to nothing.
    #[test]
    fn a_pointer_with_an_unreadable_hash_is_refused() {
        let v = Value::Map(vec![
            (text("tag"), text("pointer")),
            (text("hash"), ecf_bytes(vec![0xff, 0xff])),
        ]);
        assert!(matches!(
            EmbedPayload::from_value(&v),
            Err(PayloadError::Malformed { tag: "pointer", .. })
        ));
    }

    /// The child arm's reference is decoded, not waved through — and its
    /// refusal is carried so a caller can still tell *"an intent we have not
    /// built"* from *"a broken atom"*.
    #[test]
    fn a_child_payload_carries_its_references_own_refusal() {
        let v = Value::Map(vec![
            (text("tag"), text("child")),
            (
                text("ref"),
                Value::Map(vec![(text("tag"), text("pin")), (text("peer"), text(PEER))]),
            ),
        ]);
        assert_eq!(
            EmbedPayload::from_value(&v),
            Err(PayloadError::BadRef {
                source: RefError::TagIdentityMismatch { tag: "pin", missing: "hash" }
            })
        );
    }

    // -- the node ---------------------------------------------------------

    #[test]
    fn a_node_round_trips_through_its_inline_form() {
        let n = text_node();
        assert_eq!(EmbedNode::from_value(&n.to_value()).unwrap(), n);
        assert_eq!(n.type_name(), "app/embed/text/plain");
    }

    /// §3.1: *"a handler that accepts one accepts the other, and an
    /// implementation MUST NOT make behaviour depend on which form the embed
    /// arrived in."* Asserted as an equality between the two decoders, because
    /// the way that MUST gets broken is two code paths, not one wrong branch.
    #[test]
    fn the_addressed_and_inline_forms_decode_to_the_same_node() {
        let n = text_node();
        let from_inline = EmbedNode::from_value(&n.to_value()).unwrap();
        let from_entity = EmbedNode::from_entity(&n.to_entity().unwrap()).unwrap();
        assert_eq!(from_inline, from_entity);
        assert_eq!(from_entity, n);
    }

    /// A node and an entity differ in **addressing** only, so the entity's body
    /// is the node's `data` and nothing else.
    #[test]
    fn the_entity_body_is_the_nodes_data_verbatim() {
        let n = text_node();
        assert_eq!(n.to_entity().unwrap().data, to_ecf(&n.data_value()));
    }

    #[test]
    fn something_that_is_not_an_embed_is_refused_by_its_type() {
        let v = Value::Map(vec![
            (text("type"), text("app/site-asset")),
            (text("data"), Value::Map(vec![])),
        ]);
        assert_eq!(
            EmbedNode::from_value(&v),
            Err(EmbedError::NotAnEmbed { entity_type: "app/site-asset".into() })
        );
        // The prefix with nothing after it is not a media type either.
        let empty = Value::Map(vec![
            (text("type"), text(EMBED_TYPE_PREFIX)),
            (text("data"), Value::Map(vec![])),
        ]);
        assert_eq!(EmbedNode::from_value(&empty), Err(EmbedError::EmptyMediaType));
    }

    /// §3: `fallback` is mandatory **and** non-empty, and the two failures stay
    /// apart — one producer does not know about the degradation ladder, the
    /// other knew and had nothing to say.
    #[test]
    fn a_missing_fallback_and_an_empty_one_are_different_refusals() {
        let body = |fallback: Option<&str>| {
            let mut fields = vec![(
                text("payload"),
                EmbedPayload::Inline(b"x".to_vec()).to_value(),
            )];
            if let Some(f) = fallback {
                fields.push((text("fallback"), text(f)));
            }
            Value::Map(vec![
                (text("type"), text("app/embed/text/plain")),
                (text("data"), Value::Map(fields)),
            ])
        };
        assert_eq!(EmbedNode::from_value(&body(None)), Err(EmbedError::MissingFallback));
        assert_eq!(EmbedNode::from_value(&body(Some(""))), Err(EmbedError::EmptyFallback));
        assert!(EmbedNode::from_value(&body(Some("alt text"))).is_ok());
    }

    /// §3's first carve-out out of §2.6's MUST-ignore. An int key is not an
    /// unknown field to skip.
    #[test]
    fn a_params_key_that_is_not_a_string_is_refused_rather_than_skipped() {
        let v = Value::Map(vec![
            (text("type"), text("app/embed/text/plain")),
            (
                text("data"),
                Value::Map(vec![
                    (text("payload"), EmbedPayload::Inline(b"x".to_vec()).to_value()),
                    (text("fallback"), text("alt")),
                    (
                        text("params"),
                        Value::Map(vec![(Value::Integer(1.into()), text("v"))]),
                    ),
                ]),
            ),
        ]);
        assert_eq!(EmbedNode::from_value(&v), Err(EmbedError::NonStringParamKey));
    }

    /// §3's second carve-out: **a v0.2 consumer MUST REFUSE TO RENDER** an
    /// embed declaring caps it cannot enforce. Skipping the field as unknown is
    /// the partial honour the rule excludes.
    ///
    /// The *empty* arm is what stops this over-firing — §3 says **non-empty**,
    /// and a declared-but-empty `requires` is a producer saying *"none"*.
    #[test]
    fn a_non_empty_requires_or_sandbox_refuses_to_render_and_an_empty_one_does_not() {
        let mut active = text_node();
        active.data.requires = Some(Value::Array(vec![text("compute")]));
        assert_eq!(
            active.render_verdict(),
            RenderVerdict::RefuseActive { declaration: "requires" }
        );

        let mut boxed = text_node();
        boxed.data.sandbox = Some(Value::Map(vec![(text("deny_external_handlers"), Value::Bool(true))]));
        assert_eq!(
            boxed.render_verdict(),
            RenderVerdict::RefuseActive { declaration: "sandbox" }
        );

        let mut declared_empty = text_node();
        declared_empty.data.requires = Some(Value::Array(vec![]));
        declared_empty.data.sandbox = Some(Value::Map(vec![]));
        assert_eq!(declared_empty.render_verdict(), RenderVerdict::Renderable);

        assert_eq!(text_node().render_verdict(), RenderVerdict::Renderable);
    }

    /// §7 is `[OPEN per G1]`, so those two fields are carried opaquely — and a
    /// re-encode must not quietly drop somebody else's declaration, which would
    /// turn a refusal into a render at the next reader.
    #[test]
    fn an_open_section_declaration_survives_a_decode_and_re_encode() {
        let mut n = text_node();
        n.data.requires = Some(Value::Array(vec![text("compute")]));
        n.data.sandbox = Some(Value::Map(vec![(text("read_only_within"), text("/x"))]));
        let round_tripped = EmbedNode::from_value(&n.to_value()).unwrap();
        assert_eq!(round_tripped, n);
        assert_eq!(
            round_tripped.render_verdict(),
            RenderVerdict::RefuseActive { declaration: "requires" }
        );
    }

    #[test]
    fn renditions_round_trip_including_an_empty_tag_list() {
        let mut n = text_node();
        n.data.renditions = vec![
            Rendition {
                pointer: h("small"),
                media_type: "image/webp".into(),
                capability_tags: vec!["webp".into()],
            },
            Rendition {
                pointer: h("large"),
                media_type: "image/png".into(),
                capability_tags: vec![],
            },
        ];
        assert_eq!(EmbedNode::from_value(&n.to_value()).unwrap(), n);
    }

    #[test]
    fn params_round_trip_with_string_keys() {
        let mut n = text_node();
        n.data.params.insert("width".into(), Value::Integer(640.into()));
        n.data.params.insert("align".into(), text("center"));
        assert_eq!(EmbedNode::from_value(&n.to_value()).unwrap(), n);
    }

    /// The §3.1 example from the convention, spelled out: *"a text post is an
    /// inline payload, an image post is a pointer payload, and both are one
    /// field of one entity."*
    #[test]
    fn a_text_post_and_an_image_post_are_one_shape() {
        let post = EmbedNode::new(
            "text/plain",
            EmbedData::new(EmbedPayload::Inline("hello".as_bytes().to_vec()), "hello"),
        );
        let image = EmbedNode::new(
            "image/png",
            EmbedData::new(EmbedPayload::Pointer(h("photo")), "a photo of a cat"),
        );
        assert_eq!(EmbedNode::from_value(&post.to_value()).unwrap(), post);
        assert_eq!(EmbedNode::from_value(&image.to_value()).unwrap(), image);
        assert_eq!(post.type_name(), "app/embed/text/plain");
        assert_eq!(image.type_name(), "app/embed/image/png");
    }
}
