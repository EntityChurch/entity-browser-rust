//! `APP-CONVENTION-FEED` v0.1 — the entry, the index and the subscription
//! record.
//!
//! Four of the convention's six types, which is stage 1's scope: `app/feed/`
//! `{entry, index-head, index-page, follow}`. **`collection` (§5) and `mirror`
//! (§6) are deliberately not here** — the mirror is phase 4 and needs the
//! byte-fidelity republication path rather than a codec (its rule 1 is the one
//! `tests/mirror_byte_fidelity.rs` already gates), and the collection has no
//! consumer yet. Saying which four is not the same as saying "the FEED types
//! are done".
//!
//! ## Three properties everything else is derived from
//!
//! §1: **the author is the namespace**, **the unit of addressing is the unit of
//! verification**, and **an entry that travels verifies alone**. The first two
//! are what this module implements; the third is why [`FeedEntry`] keeps an
//! `author` field where [`crate::share::Share`] dropped its `from`.
//!
//! **That difference looks like an inconsistency between two conventions and is
//! not.** A share is read *in the publisher's tree*, so §4 of SHARE makes the
//! namespace the fact and a `from` field a stranger's self-declared claim about
//! their own identity — we deleted it. **An entry travels** (§6's mirror), and a
//! mirrored entry read out of a gatherer's tree has no namespace to consult, so
//! `author` is the term that survives the journey. `FEED-R1` is what keeps it
//! honest: an entry found under one peer's namespace claiming a different
//! author is **invalid**, and [`FeedEntry::from_entity`] takes the namespace as
//! an argument and refuses. *Carry a fact the structure supplies only where the
//! structure stops supplying it.*
//!
//! ## What a body is, and the blocker that was not one
//!
//! `body` is an `embed-node` **carried inline** — [`crate::embed`], the INPUT
//! surface. **No separate `Embed` entity is required for an entry**: a text post
//! is an inline payload, an image post is a pointer payload, and both are one
//! field of one entity. Only the `child` arm names a separate entity, and it is
//! optional. This was read as a blocker on EMBED's sequencing for two weeks and
//! never was one — arch `d1583f3`, *"the entry body was never blocked:
//! embed-node named the wrong surface."* Do not re-derive it.
//!
//! ## Each site declares which reference atom it takes, and the pinned ones do
//! not widen
//!
//! §2.2.1 is a table, and it is the reason [`EntityRef::is_pinned`] exists:
//!
//! | site | accepts | why |
//! |---|---|---|
//! | `reply.root`, `reply.parent` | **pinned only** (`FEED-R6`) | a reply must not become as trustworthy as whatever currently answers a location, and a parent must not be editable underneath its replies |
//! | `prev` | a bare `content-hash` | an append-only commitment to a *specific* predecessor is meaningless against a moving target |
//! | `context`, `attachments` | either | *"part of a topic"* is often a maintained index; an attached *living* document is the case the live shape exists for |
//! | `index-page.entries` | **pinned only** | typed `reference`, §4.2 |
//!
//! **A live reference in a pinned site is refused, not silently accepted** —
//! [`FeedError::LiveReferenceWherePinRequired`], and it is a carve-out out of
//! §2.6's MUST-ignore in the same family as EMBED's two.
//!
//! ## `reply` carries `root` as well as `parent`, and the second field is
//! load-bearing
//!
//! With `parent` alone, assembling a conversation is a hop-by-hop walk and **one
//! unreachable author truncates everything below them**. `root` lets any holder
//! of any entry name the whole conversation in one step. **A conversation needs
//! no genesis entity: the root entry *is* the conversation.**
//!
//! `context` is **not** `reply`. `reply` says *this answers that*; `context`
//! says *this belongs with that*. Collapsing them makes *"replied to"*
//! unrenderable.
//!
//! ## `created_at` is not an ordering authority, and this module does nothing
//! about it on purpose
//!
//! §2.3.2 / `FEED-R8`: a reader **MUST NOT** rely on it for correctness and
//! **MUST NOT reject an entry for an implausible timestamp**. So there is no
//! validation here, and
//! [`an_implausible_created_at_is_not_a_reason_to_reject_an_entry`] asserts the
//! absence — because *"we do not check X"* is exactly the kind of claim a later
//! author repairs into a bug.
//!
//! [`an_implausible_created_at_is_not_a_reason_to_reject_an_entry`]: self::tests
//!
//! ## ⚠ §2.4 and §4.4 declare two different cursors, and the declared one
//! cannot satisfy `FEED-R14`
//!
//! §2.4's CDDL: `? cursor: content-hash ; last index page this reader applied
//! (§4.4)`. §4.4: *"**The cursor is `{page, applied}`** — the page number the
//! reader reached, and the newest entry hash it took from that page … **This is
//! why the cursor carries a number and not only a hash**: an author may remove
//! the very entry a reader was holding as its position, and a cursor that
//! cannot survive that is a cursor that breaks on edit."*
//!
//! **A single `content-hash` is exactly the "only a hash" form §4.4 says is
//! insufficient**, so `FEED-R14` — *resume from `page` when `applied` no longer
//! resolves* — has no field to read `page` out of.
//!
//! **We implement the declared field** (`? cursor: content-hash`) and route the
//! contradiction, for the reason `G-PIN-4` taught one convention over: whatever
//! publishes first is the baseline, and inventing a `{page, applied}` map would
//! make us the baseline for a shape the spec does not declare. The cost is
//! bounded and stated — §2.4 says a follow record is **the reader's private
//! data** and *"nothing in this convention publishes it"* — so where the page
//! number lives for our own resumption is a phase-3 decision about local state,
//! not a wire question we may settle alone.
//!
//! ## The detached signature is `FEED-R2` and is NOT built here
//!
//! §1.1: an entry MUST be individually signed as a **separate**
//! `system/signature` entity at `/{author}/system/signature/{hex(entry_hash)}`.
//! The path builder is the kernel's ([`entity_hash::invariant_signature_path`])
//! and the emitter exists ([`crate::content_site::registry_publish`]'s
//! `sign_detached`, at the same invariant pointer for registry bindings), so
//! `FEED-R2` is a loop rather than a design — **and it is phase 2b's loop, not
//! this module's.** A format module that signed nothing while claiming `FEED-R2`
//! would be the shape this repo calls *recording a gap is what makes it look
//! handled.*

#![allow(dead_code)] // the reader loop and the publisher land in 2b/2c

use entity_ecf::{bytes as ecf_bytes, text, to_ecf, uinteger, Value};
use entity_entity::Entity;
use entity_hash::Hash;

use crate::embed::{EmbedError, EmbedNode};
use crate::entity_ref::{EntityRef, RefError};

/// `app/feed/entry` — **one thing someone published**, the atom of the
/// convention. Assembled by its author.
///
/// A blog post, a video post, a photo post, a comment and a forum post are all
/// this one type: *"a new product is a new body type or a new renderer. It is a
/// new entity type only if a conformant consumer must **behave** differently."*
pub const FEED_ENTRY_TYPE: &str = "app/feed/entry";

/// `app/feed/index-head` — the stream's entry point: which page numbers are in
/// use.
pub const FEED_INDEX_HEAD_TYPE: &str = "app/feed/index-head";

/// `app/feed/index-page` — one **key-addressed** page of the author's stream,
/// newest-first within the page.
pub const FEED_INDEX_PAGE_TYPE: &str = "app/feed/index-page";

/// `app/feed/mirror` — **what one reader gathered, published so the next reader
/// does not have to gather it again** (§6).
///
/// The gatherer signs *this record*; the entries it names are **republished
/// unmodified**, each travelling with its author's own detached signature, so a
/// mirror carries authorship it did not mint and cannot forge. §6.1 rule 2:
/// *a mirror can omit but never substitute.*
pub const FEED_MIRROR_TYPE: &str = "app/feed/mirror";

/// `app/feed/follow` — a reader's durable subscription to a peer's feed.
///
/// **A distinct type from `app/share/follow`, and the discriminator is the
/// SUBJECT.** `app/share/follow` follows a **grant** — a titled record whose
/// audience the publisher authorized, so *the publisher knows the follower
/// exists*. This follows a **namespace** — public, pull-only, no grant, no
/// permission, and *the publisher does not know the follower exists*. Different
/// mechanisms with different authorization models, which is why §2.4 declined
/// to unify them and why we must not either.
pub const FEED_FOLLOW_TYPE: &str = "app/feed/follow";

/// §4.2's head, **peer-relative** — the one literal both path forms are built
/// from.
///
/// Two things ride on the leading slash. It is what makes this a *path* and not
/// a *type tag*, to a reader and to the tier's vocabulary analyzer alike — the
/// bare spelling `app/feed/index` is shaped exactly like a tag and
/// `tools/vocab-lint.sh` reported it as one (see [`index_head_key`]). And it is
/// what lets [`index_head_path`] and [`index_head_key`] be **one** expression of
/// §4.2 rather than two that can drift.
const INDEX_HEAD_REL: &str = "/app/feed/index";

/// The head's pinned path — `/{peer}/app/feed/index` (§4.2).
///
/// **Pinned, because a reader holding no reference has to start somewhere.**
/// Unlike SHARE, where the convention specifies a type vocabulary and no tree
/// path, FEED names both paths by hand — an index nobody can find is not an
/// entry point.
pub fn index_head_path(peer_id: &str) -> String {
    format!("/{peer_id}{INDEX_HEAD_REL}")
}

/// A page's pinned path — `/{peer}/app/feed/index/{page}`, the page number in
/// decimal (§4.2).
///
/// **The key IS the page number**, which is what makes §4.3's rule 1 work: a
/// hash back-chain would make page *N*'s identity depend on page *N−1*, so
/// editing one old page forces rewriting every page after it. With key
/// addressing, rewriting page 12 changes page 12's binding and nothing else.
pub fn index_page_path(peer_id: &str, page: u64) -> String {
    format!("{}/{page}", index_head_path(peer_id))
}

/// The head's **peer-relative key** — what a consumer resolving against a signed
/// tree asks for, since that API takes a key rather than an absolute path.
///
/// ⚠ **Derived rather than spelled, and the reason is a live analyzer
/// limitation rather than taste.** `spec vocab` classifies a source literal by
/// shape, and skips a would-be tag only when it ends in `/` — the rule added
/// after its first run reported `app/share/records/` (a tree prefix) as
/// undeclared vocabulary. `app/feed/index` is a *complete* path, so it carries
/// no trailing slash, and it walked straight through that rule: `make lint` went
/// red on **`implemented-undeclared app/feed/index`**, a type tag no spec
/// declares and that we do not emit.
///
/// **That is the expensive direction** — `_DECL_PARAM`'s own note calls
/// accusing a conformant seat of inventing vocabulary the costly false
/// positive — and it is the second instance of the same path/tag confusion, so
/// it is routed to arch rather than only worked around here. What this function
/// buys is that our own gate is not red about a path; **it does not fix the
/// analyzer**, and the next bare `app/feed/…` key literal anywhere will trip it
/// again.
pub fn index_head_key() -> &'static str {
    INDEX_HEAD_REL.trim_start_matches('/')
}

/// A page's peer-relative key — §4.2's `app/feed/index/{page}`, decimal.
///
/// Derived from [`index_head_key`] rather than spelled, so §4.2's *"pages live
/// under the head"* is one expression. The reason the literal above carries a
/// leading slash applies here transitively.
pub fn index_page_key(page: u64) -> String {
    format!("{}/{page}", index_head_key())
}

/// The tree prefix an entry is bound under. ⚠ **OURS, not the convention's.**
///
/// §2: ***"the cross-impl contract is the type tag, not the path"*** — so
/// aggregation is a type-filtered query and where an entry *lives* is a local
/// choice. A cross-impl consumer must not depend on this; it exists so the
/// publisher can bind, and so §4.3 rule 6's fallback has something to
/// enumerate. Spelled with the leading slash for [`index_head_key`]'s reason.
const ENTRY_PREFIX_REL: &str = "/app/feed/entries/";

/// The tree prefix an entry is bound under. Ours; see [`ENTRY_PREFIX_REL`].
pub fn entry_prefix() -> &'static str {
    ENTRY_PREFIX_REL.trim_start_matches('/')
}

/// Where an entry is bound — **keyed by its own content hash.**
///
/// §2.2.1 makes every reference to an entry a **pin**, so an entry's identity
/// *is* its hash, and a mutable key would let a publisher move different bytes
/// under the address a `via: path` hint names. A key that is the identity
/// cannot drift from it.
pub fn entry_key(entry_hash: &Hash) -> String {
    format!("{}{}", entry_prefix(), entry_hash.to_hex())
}

/// An entry's detached-signature key — V7 §3.5's invariant pointer, tail only.
///
/// The **path** form is [`FeedEntry::signature_path`], which calls the kernel's
/// own builder. This is the peer-relative key a foreign-tree read takes, and it
/// is derived from that same builder rather than restated, so the two cannot
/// drift — C15, on a path this module has two spellings of by necessity.
pub fn signature_key(author: &str, entry_hash: &Hash) -> String {
    let path = entity_hash::invariant_signature_path(author, entry_hash);
    // `/{peer}/system/signature/{hex}` → `system/signature/{hex}`.
    path.trim_start_matches('/')
        .strip_prefix(author)
        .unwrap_or(&path)
        .trim_start_matches('/')
        .to_string()
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// Why a feed entity was refused. **Each names whose defect it is.**
#[derive(Debug, Clone, PartialEq)]
pub enum FeedError {
    /// Not one of this convention's types. The first question, for the reason
    /// `decode_share` asks it first: a mirrored foreign subtree can hand you
    /// anything.
    NotAFeedEntity { entity_type: String },
    /// Unreadable body, or a required field missing or ill-typed.
    Malformed(&'static str),
    /// **`FEED-R1`, the forgery gate.** An entry found under one peer's
    /// namespace claiming a different author is invalid and a conformant reader
    /// MUST reject it. Its own outcome, because *"this entry is corrupt"* and
    /// *"this entry claims to be by somebody else"* are not the same report and
    /// only one of them is interesting.
    AuthorIsNotTheNamespace { author: String, namespace: String },
    /// **`FEED-R6`** — a live reference in a site §2.2.1 types as `reference`.
    /// A carve-out out of §2.6's MUST-ignore: the atom is well-formed and the
    /// site does not admit it, so it is a schema violation and not a corrupt
    /// byte.
    LiveReferenceWherePinRequired { field: &'static str },
    /// A reference that would not decode, carrying its own outcome so a caller
    /// can still tell an intent we have not built from a broken atom.
    Reference { field: &'static str, source: RefError },
    /// The body's own refusal, carried through.
    Body(EmbedError),
    /// **§4.2's `page MUST equal its key`.** The page number is the address, so
    /// a body that disagrees with the key it was read from is a page that would
    /// answer to two numbers — and `FEED-R11` (never renumber) is exactly the
    /// property that would break.
    PageNumberDisagreesWithKey { declared: u64, key: u64 },
}

impl std::fmt::Display for FeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FeedError::NotAFeedEntity { entity_type } => {
                write!(f, "not a feed entity: {entity_type}")
            }
            FeedError::Malformed(what) => write!(f, "malformed feed entity: {what}"),
            FeedError::AuthorIsNotTheNamespace { author, namespace } => write!(
                f,
                "entry claims author {author} but was found under {namespace}"
            ),
            FeedError::LiveReferenceWherePinRequired { field } => {
                write!(f, "{field} requires a pinned reference and carries a live one")
            }
            FeedError::Reference { field, source } => write!(f, "{field}: {source}"),
            FeedError::Body(e) => write!(f, "body: {e}"),
            FeedError::PageNumberDisagreesWithKey { declared, key } => {
                write!(f, "index page declares page {declared} at key {key}")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Shared decode helpers
// ---------------------------------------------------------------------------

fn field<'a>(map: &'a [(Value, Value)], name: &str) -> Option<&'a Value> {
    map.iter().find(|(k, _)| k.as_text() == Some(name)).map(|(_, v)| v)
}

fn body_map(entity: &Entity, want: &str) -> Result<Vec<(Value, Value)>, FeedError> {
    if entity.entity_type != want {
        return Err(FeedError::NotAFeedEntity {
            entity_type: entity.entity_type.clone(),
        });
    }
    let value: Value = ciborium::from_reader(entity.data.as_slice())
        .map_err(|_| FeedError::Malformed("body is not CBOR"))?;
    value
        .as_map()
        .cloned()
        .ok_or(FeedError::Malformed("body is not a map"))
}

fn uint(map: &[(Value, Value)], name: &'static str) -> Option<u64> {
    field(map, name)
        .and_then(|v| v.as_integer())
        .and_then(|i| u64::try_from(i128::from(i)).ok())
}

fn required_uint(map: &[(Value, Value)], name: &'static str) -> Result<u64, FeedError> {
    uint(map, name).ok_or(FeedError::Malformed(name))
}

/// An optional `uint` where **absent and unreadable must not collapse.**
///
/// `uint(..).unwrap_or(default)` is the shape that reads a present-but-ill-typed
/// field as the default, and it is only safe where the default is not itself a
/// meaningful value. `IndexHead::oldest`'s is: `0` means *"nothing has been
/// dropped"*, so a floor we could not read would claim the whole archive is
/// still published. Absent stays a real default; unreadable is the field's own
/// refusal.
fn optional_uint(map: &[(Value, Value)], name: &'static str) -> Result<Option<u64>, FeedError> {
    match field(map, name) {
        None => Ok(None),
        Some(v) => v
            .as_integer()
            .and_then(|i| u64::try_from(i128::from(i)).ok())
            .map(Some)
            .ok_or(FeedError::Malformed(name)),
    }
}

fn required_text(map: &[(Value, Value)], name: &'static str) -> Result<String, FeedError> {
    field(map, name)
        .and_then(|v| v.as_text())
        .map(str::to_string)
        .ok_or(FeedError::Malformed(name))
}

fn hash_field(map: &[(Value, Value)], name: &'static str) -> Result<Option<Hash>, FeedError> {
    match field(map, name) {
        None => Ok(None),
        Some(v) => {
            let raw = v
                .as_bytes()
                .ok_or(FeedError::Malformed("content hash is not a byte string"))?;
            Hash::from_bytes(raw)
                .map(Some)
                .map_err(|_| FeedError::Malformed("content hash is not a hash"))
        }
    }
}

/// Decode a reference and enforce the site's declared atom (§2.2.1).
fn reference(v: &Value, field_name: &'static str, pinned_only: bool) -> Result<EntityRef, FeedError> {
    let r = EntityRef::from_value(v).map_err(|source| FeedError::Reference {
        field: field_name,
        source,
    })?;
    if pinned_only && !r.is_pinned() {
        return Err(FeedError::LiveReferenceWherePinRequired { field: field_name });
    }
    Ok(r)
}

fn reference_list(
    v: &Value,
    field_name: &'static str,
    pinned_only: bool,
) -> Result<Vec<EntityRef>, FeedError> {
    let list = v
        .as_array()
        .ok_or(FeedError::Malformed("reference list is not an array"))?;
    let mut out = Vec::with_capacity(list.len());
    for item in list {
        out.push(reference(item, field_name, pinned_only)?);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// app/feed/entry
// ---------------------------------------------------------------------------

/// §2.3's `reply` — **present means this entry is a reply.**
///
/// Both terms are pinned (`FEED-R6`). `root` is not redundant with `parent`:
/// see the module doc.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub root: EntityRef,
    pub parent: EntityRef,
}

/// §2.3's `app/feed/entry`.
#[derive(Debug, Clone, PartialEq)]
pub struct FeedEntry {
    /// MUST equal the authoring namespace (`FEED-R1`).
    pub author: String,
    /// The author's own clock, in ms. **A display heuristic, never an ordering
    /// authority** (§2.3.2).
    pub created_at: u64,
    /// What was authored. The handler runs at the reader.
    pub body: EmbedNode,
    /// Present ⇒ this is a reply.
    pub reply: Option<Reply>,
    /// What this is **part of** — never who it is for (§9.4 declines addressing
    /// altogether).
    pub context: Option<EntityRef>,
    /// **Opt-in** append-only commitment (§2.3.1). NOT navigation — navigation
    /// is by key (§4), never by chain.
    pub prev: Option<Hash>,
    /// Referenced, never inlined (`FEED-R5`) — which this type satisfies
    /// structurally, since the field cannot hold bytes.
    pub attachments: Vec<EntityRef>,
}

impl FeedEntry {
    /// The floor: an author, a clock and a body.
    pub fn new(author: impl Into<String>, created_at: u64, body: EmbedNode) -> Self {
        Self {
            author: author.into(),
            created_at,
            body,
            reply: None,
            context: None,
            prev: None,
            attachments: Vec::new(),
        }
    }

    /// The `data` map. Optional terms are emitted only when present — an absent
    /// `reply` and an empty one are different facts, and only the first is
    /// representable.
    pub fn data_value(&self) -> Value {
        let mut fields = vec![
            (text("author"), text(self.author.clone())),
            (text("created_at"), uinteger(self.created_at)),
            (text("body"), self.body.to_value()),
        ];
        if let Some(reply) = &self.reply {
            fields.push((
                text("reply"),
                Value::Map(vec![
                    (text("root"), reply.root.to_value()),
                    (text("parent"), reply.parent.to_value()),
                ]),
            ));
        }
        if let Some(context) = &self.context {
            fields.push((text("context"), context.to_value()));
        }
        if let Some(prev) = &self.prev {
            fields.push((text("prev"), ecf_bytes(prev.to_bytes())));
        }
        if !self.attachments.is_empty() {
            fields.push((
                text("attachments"),
                Value::Array(self.attachments.iter().map(EntityRef::to_value).collect()),
            ));
        }
        Value::Map(fields)
    }

    pub fn to_entity(&self) -> Result<Entity, String> {
        Entity::new(FEED_ENTRY_TYPE, to_ecf(&self.data_value()))
            .map_err(|e| format!("feed entry: {e}"))
    }

    /// Decode an entry read under `namespace`.
    ///
    /// **`namespace` is not optional and is not a convenience.** `FEED-R1` — the
    /// forgery gate — is the whole reason it is a parameter: an entry found
    /// under one peer's namespace claiming a different author is invalid, and a
    /// decoder that never learns where it read the entity from cannot check it.
    pub fn from_entity(entity: &Entity, namespace: &str) -> Result<Self, FeedError> {
        let map = body_map(entity, FEED_ENTRY_TYPE)?;
        let author = required_text(&map, "author")?;
        if author != namespace {
            return Err(FeedError::AuthorIsNotTheNamespace {
                author,
                namespace: namespace.to_string(),
            });
        }
        // No validation of the VALUE — `FEED-R8` forbids rejecting an entry for
        // an implausible timestamp, so the only requirement is that the field
        // is there and is a number.
        let created_at = required_uint(&map, "created_at")?;
        let body = EmbedNode::from_value(
            field(&map, "body").ok_or(FeedError::Malformed("body"))?,
        )
        .map_err(FeedError::Body)?;

        let reply = match field(&map, "reply") {
            None => None,
            Some(v) => {
                let inner = v.as_map().ok_or(FeedError::Malformed("reply is not a map"))?;
                let root = reference(
                    field(inner, "root").ok_or(FeedError::Malformed("reply.root"))?,
                    "reply.root",
                    true,
                )?;
                let parent = reference(
                    field(inner, "parent").ok_or(FeedError::Malformed("reply.parent"))?,
                    "reply.parent",
                    true,
                )?;
                Some(Reply { root, parent })
            }
        };
        let context = match field(&map, "context") {
            None => None,
            Some(v) => Some(reference(v, "context", false)?),
        };
        let prev = hash_field(&map, "prev")?;
        let attachments = match field(&map, "attachments") {
            None => Vec::new(),
            Some(v) => reference_list(v, "attachments", false)?,
        };

        Ok(FeedEntry { author, created_at, body, reply, context, prev, attachments })
    }

    /// Where this entry's detached signature belongs — `FEED-R2`'s path, built
    /// by the **kernel's** helper rather than restated here (V7 §3.5's
    /// invariant pointer). Minting it is phase 2b's; see the module doc.
    pub fn signature_path(&self, entry_hash: &Hash) -> String {
        entity_hash::invariant_signature_path(&self.author, entry_hash)
    }
}

// ---------------------------------------------------------------------------
// app/feed/index-head and app/feed/index-page
// ---------------------------------------------------------------------------

/// §4.2's `app/feed/index-head` — which page numbers are in use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexHead {
    /// The highest page number in use.
    pub current: u64,
    /// The lowest page number still published. **Defaults to 0**, per §4.2.
    pub oldest: u64,
    pub updated_at: u64,
}

impl IndexHead {
    pub fn new(current: u64, updated_at: u64) -> Self {
        Self { current, oldest: 0, updated_at }
    }

    pub fn data_value(&self) -> Value {
        let mut fields = vec![(text("current"), uinteger(self.current))];
        // `? oldest` defaults to 0, so a head that has dropped nothing emits no
        // key — declared-default and absent are the same fact here and the
        // CDDL says which one is canonical.
        if self.oldest != 0 {
            fields.push((text("oldest"), uinteger(self.oldest)));
        }
        fields.push((text("updated_at"), uinteger(self.updated_at)));
        Value::Map(fields)
    }

    pub fn to_entity(&self) -> Result<Entity, String> {
        Entity::new(FEED_INDEX_HEAD_TYPE, to_ecf(&self.data_value()))
            .map_err(|e| format!("feed index head: {e}"))
    }

    pub fn from_entity(entity: &Entity) -> Result<Self, FeedError> {
        let map = body_map(entity, FEED_INDEX_HEAD_TYPE)?;
        Ok(IndexHead {
            current: required_uint(&map, "current")?,
            // Absent is a real zero (§4.2's declared default); present-and-
            // unreadable is not — see `optional_uint`.
            oldest: optional_uint(&map, "oldest")?.unwrap_or(0),
            updated_at: required_uint(&map, "updated_at")?,
        })
    }
}

/// §4.2's `app/feed/index-page` — one key-addressed page, **newest first within
/// the page**.
///
/// §4.5: *"`entries` within an index page is newest-first, as ordered by the
/// publisher. The order is **authored**, not derived."* So this type preserves
/// the order it was given and never sorts.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexPage {
    /// This page's own number — **MUST equal its key**.
    pub page: u64,
    /// Pinned references, newest first. **A producer MUST NOT emit an unbounded
    /// page and a reader MUST NOT assume any page size** (`FEED-R12`) — §4.3
    /// rule 5 declines to name a number on purpose, because the right value
    /// depends on entry size and cadence, which vary by orders of magnitude
    /// between publishers. *"What is normative is the shape, not the
    /// arithmetic."* So this type imposes no bound and no reader here reads one.
    pub entries: Vec<EntityRef>,
    pub updated_at: u64,
}

impl IndexPage {
    pub fn new(page: u64, entries: Vec<EntityRef>, updated_at: u64) -> Self {
        Self { page, entries, updated_at }
    }

    pub fn data_value(&self) -> Value {
        Value::Map(vec![
            (text("page"), uinteger(self.page)),
            (
                text("entries"),
                Value::Array(self.entries.iter().map(EntityRef::to_value).collect()),
            ),
            (text("updated_at"), uinteger(self.updated_at)),
        ])
    }

    pub fn to_entity(&self) -> Result<Entity, String> {
        Entity::new(FEED_INDEX_PAGE_TYPE, to_ecf(&self.data_value()))
            .map_err(|e| format!("feed index page: {e}"))
    }

    /// Decode a page read at key `key_page`.
    ///
    /// **The key is a parameter for the same reason `namespace` is on an
    /// entry:** §4.2 makes `page` MUST equal its key, and a decoder that never
    /// learns which key it read from cannot check it. A page answering to two
    /// numbers is `FEED-R11`'s renumbering hazard arriving through the body
    /// instead of through the publisher.
    pub fn from_entity(entity: &Entity, key_page: u64) -> Result<Self, FeedError> {
        let map = body_map(entity, FEED_INDEX_PAGE_TYPE)?;
        let page = required_uint(&map, "page")?;
        if page != key_page {
            return Err(FeedError::PageNumberDisagreesWithKey { declared: page, key: key_page });
        }
        let entries = match field(&map, "entries") {
            None => return Err(FeedError::Malformed("entries")),
            Some(v) => reference_list(v, "entries", true)?,
        };
        Ok(IndexPage { page, entries, updated_at: required_uint(&map, "updated_at")? })
    }
}

// ---------------------------------------------------------------------------
// app/feed/mirror
// ---------------------------------------------------------------------------

/// The tree prefix a mirror is bound under — **§6.0.1, and it is the
/// convention's now rather than ours.**
///
/// It was ours when this module shipped, for the reason the old note gave: §6
/// pinned no path, §2's cross-impl contract is the type tag, and finding a mirror
/// was therefore a type-filtered query a static origin cannot serve (`A-38`).
/// **Arch ruled the prefix and adopted this spelling unchanged**, on the argument
/// one step further along than we took it: the thing a reader needs is to
/// *enumerate what a peer has gathered, from that peer's signed root, without
/// asking them*, which a type query cannot do — so *"a gathered view nobody can
/// find is not a source leg"*, the same sentence §4.2 is written from.
const MIRROR_PREFIX_REL: &str = "/app/feed/mirrors/";

/// The tree prefix a mirror is bound under (§6.0.1). See [`MIRROR_PREFIX_REL`].
pub fn mirror_prefix() -> &'static str {
    MIRROR_PREFIX_REL.trim_start_matches('/')
}

/// **What a mirror is OF** — §6.0's two kinds, as a type rather than as a raw
/// reference.
///
/// §6's `subject` is an `any-reference` and the two atoms mean two different
/// things, so *which kind it is* decides what the mirror means and how short
/// reads:
///
/// | kind | names | writers | *short* means |
/// |---|---|---|---|
/// | [`Thread`](Self::Thread) | one entity many parties contribute to | many | a contributor you did not reach |
/// | [`Timeline`](Self::Timeline) | a prefix one peer owns | exactly one | a gap in the prefix |
///
/// ## ⭐ Why this is a type and not just an `EntityRef`
///
/// **`FEED-R26`: a `subject` MUST NOT be a pin to a value that moves when the
/// subject changes** — and the value that violates it is the one a developer
/// reaches for first, an author's **index head**. Its hash changes every time
/// they post (so it is a *witness*, not an identity) and a reader must already
/// have reached the author to know it (so the address is underivable, which is
/// the hop a mirror exists to save).
///
/// A raw `EntityRef` parameter makes that mistake the path of least resistance:
/// the gatherer is holding the head when it decides. [`Self::timeline`] **takes
/// no hash and no path**, so the violating value is not expressible through the
/// constructor a timeline gather uses. *The guard is the argument list.*
///
/// ## The address survives the author posting, which is the measurable form
///
/// `FEED-R26`'s consequence is a property a test can hold: derive the key, let
/// the author post again, derive it once more — **same key.** A head-pinned
/// subject moves. That is what the rule is protecting and it is why the
/// enforcement here is a constructor rather than a validator: nothing in the
/// bytes of a mirror record says whether its pinned subject moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MirrorSubject {
    /// §6.0's **pinned** kind — a thread root. Many writers, no single
    /// authority.
    Thread {
        /// Who published the root. Carried for routing; **not in the key** —
        /// §6.0.1 derives a pinned key from the hash alone, and a content
        /// address needs no peer to be unique.
        peer: String,
        root: Hash,
    },
    /// §6.0's **live** kind — an author's own feed, the prefix they alone own.
    Timeline {
        peer: String,
        /// Peer-relative with a leading `/`, as [`EntityRef::live`] normalizes.
        path: String,
    },
}

impl MirrorSubject {
    /// A thread mirror of the entity at `root`, published by `peer`.
    pub fn thread(peer: impl Into<String>, root: Hash) -> Self {
        Self::Thread { peer: peer.into(), root }
    }

    /// A timeline mirror of `author`'s own feed.
    ///
    /// ⚠ **Takes no path, and that is a derivation rather than a convenience.**
    /// §6.0 calls the live subject *"a prefix one peer owns"* and names none, so
    /// a caller-supplied path would put a **local** choice inside a key two
    /// implementations have to agree on. Our own entry prefix is explicitly ours
    /// and not the convention's (`A-38`, [`entry_prefix`]) — a key derived from
    /// it could never be computed by another seat.
    ///
    /// So the address is [`index_head_path`]: the **only** feed path §4.2 pins by
    /// hand, named there for the reason §6.0.1 cites for pinning this prefix.
    /// Note this is the repair §6.0's own warning implies rather than a
    /// contradiction of it — that note forbids pinning the head's **hash**, and a
    /// live reference to the head's **path** has neither defect it lists: the
    /// path does not move when the author posts, and a reader needs no prior hop
    /// to write it down.
    ///
    /// ⚠ **Routed, not settled** — §6.0 does not say which path this is, and
    /// `FEED-12` compares keys across two seats. See [`path_coordinate`].
    pub fn timeline(author: impl Into<String>) -> Self {
        let peer = author.into();
        let path = index_head_path(&peer);
        // `index_head_path` is absolute; a subject's own `path` is peer-relative.
        let relative = path.strip_prefix(&format!("/{peer}")).unwrap_or(INDEX_HEAD_REL).to_string();
        Self::Timeline { peer, path: relative }
    }

    /// Read a subject off a decoded mirror record.
    ///
    /// ⭐ **Every field of both atoms is named, with the hints bound to `_`** —
    /// `FEED-R27` forbids an optional hint entering the derivation, and the way
    /// to enforce that is to make *adding a field to the atom* a compile error
    /// (`error[E0027]`) at the one site that decides what a coordinate is made
    /// of. A `..` here is the version that silently starts ignoring a field
    /// somebody later decided was identifying.
    pub fn from_reference(r: &EntityRef) -> Self {
        match r {
            EntityRef::Pinned { peer, hash, at: _, via: _ } => {
                Self::Thread { peer: peer.clone(), root: *hash }
            }
            EntityRef::Live { peer, path, seen: _, at: _, via: _ } => {
                Self::Timeline { peer: peer.clone(), path: path.clone() }
            }
        }
    }

    /// The reference a mirror record carries.
    pub fn reference(&self) -> EntityRef {
        match self {
            Self::Thread { peer, root } => EntityRef::pin(peer.clone(), *root),
            Self::Timeline { peer, path } => EntityRef::live(peer.clone(), path.clone()),
        }
    }

    /// Who to ask about the subject itself. Not part of a thread's key.
    pub fn peer(&self) -> &str {
        match self {
            Self::Thread { peer, .. } | Self::Timeline { peer, .. } => peer,
        }
    }

    /// §6.0.1's coordinate — **the value the key is the hex of.**
    pub fn coordinate(&self) -> Hash {
        match self {
            Self::Thread { root, .. } => *root,
            Self::Timeline { peer, path } => path_coordinate(peer, path),
        }
    }

    /// Where a gatherer binds its mirror of this subject (`FEED-R25`).
    pub fn key(&self) -> String {
        format!("{}{}", mirror_prefix(), self.coordinate().to_hex())
    }
}

/// §6.0.1's live coordinate — `content_hash` of the **absolute** path,
/// `/{peer}/{path}`, canonical UTF-8.
///
/// ## ⚠ This is the one invention in the ruling, and the clause is one argument
/// short
///
/// §6.0.1 says `hex(content_hash(absolute-path))`. **`content_hash` in this
/// corpus is a function of TWO arguments** — V7 §1.4's hash over the ECF
/// encoding of `{data, type}`, which is what [`Hash::compute`] takes — and a
/// path is not an entity, so there is no type to supply. Two readings survive,
/// they produce different bytes, and **the key is precisely the thing `FEED-12`
/// compares**, so a seat cannot pass that vector by reading the text.
///
/// | reading | input | what it invents |
/// |---|---|---|
/// | **ours** — the digest of the path | `sha256(utf8(absolute))` | that `content_hash` here means a bare digest |
/// | ECF-framed | `Hash::compute(T, utf8(absolute))` | a value for `T`, which the clause does not name |
///
/// **We take the first because its input is fully determined by the clause's own
/// words.** The second needs a type tag that would itself have to be transmitted
/// and agreed — and anything tag-shaped invented here is also a vocabulary
/// finding (`spec vocab` classifies source literals by shape). The output is
/// wrapped in the corpus's SHA-256 hash format so that **both kinds of key are
/// the same shape**, 66 hex characters `00`-prefixed, which is what makes
/// [`MirrorSubject::key`] one expression instead of two.
///
/// **Routed as an ask, and one function to change if arch rules the other way.**
/// `the_live_coordinate_is_pinned_to_a_literal` holds the bytes so a silent
/// drift on either side is visible.
pub fn path_coordinate(peer: &str, relative_path: &str) -> Hash {
    let absolute = if relative_path.starts_with('/') {
        format!("/{peer}{relative_path}")
    } else {
        format!("/{peer}/{relative_path}")
    };
    let digest: [u8; 32] = <sha2::Sha256 as sha2::Digest>::digest(absolute.as_bytes()).into();
    Hash::new(entity_hash::HASH_ALGORITHM_SHA256, digest)
}

// There is deliberately no `mirror_key(&Hash)` shorthand beside
// `MirrorSubject::key`. It existed while a subject could only be a pin, and once
// the subject widened it became a second expression of §6.0.1's derivation with
// **no caller** — C15's drift shape, and the dangerous half of it: the one
// nobody reads is the one that stops agreeing.

/// §6's `app/feed/mirror` — one reader's gathered view of one subject.
///
/// ## What `subject` may be — **widened, and the argument for widening is
/// closure's**
///
/// v0.1's `subject: reference` was pinned-only, so a subject could name one
/// entity and nothing else. We measured that the case the closure trace turns on
/// had no type: a gatherer following three *authors* republishes their
/// **timelines**, and a timeline is a growing prefix, not an entity. **Ruled
/// (`A-57`): §6.0's `subject` is an `any-reference`**, and [`MirrorSubject`]
/// carries the distinction.
///
/// ⭐ **Widened rather than given a second type, and that is not a size
/// judgement.** The record carries no field that differs between the two kinds
/// and a reader does the same thing with both, so a second type would **double
/// the consuming code path at exactly the seam `DX-R2` requires to be single** —
/// i.e. it would have been the first thing the new closure `MUST` forbids,
/// proposed in the same week that `MUST` landed.
///
/// What the two kinds buy is an **authority** distinction rather than a
/// presentation one: a timeline has exactly one writer and a thread has many, so
/// *short* means different things and the rollback floor applies to one and not
/// the other.
#[derive(Debug, Clone, PartialEq)]
pub struct FeedMirror {
    /// §6.0 — the subject this view is of. **Either atom**; see
    /// [`MirrorSubject`] for what each kind means and for `FEED-R26`.
    pub subject: EntityRef,
    /// Pinned references to what this mirror holds, **republished unmodified**.
    /// Order is the gatherer's; nothing in §6 makes it authoritative.
    ///
    /// ⭐ **Always pinned, even though `subject` is not — `FEED-R28`.** §6.0 says
    /// why in one line: a mirror carries exact bytes (§6.1 rule 1), so an entry
    /// named by a live reference would be a mirror of *whatever is there now*,
    /// which is not a mirror. The two fields taking different atoms is the shape
    /// of the type, not an inconsistency in it.
    pub entries: Vec<EntityRef>,
    pub gathered_at: u64,
    /// The key that assembled it. **Never an authorship claim** — §6.1 rule 3:
    /// attribution follows `entry.author`, and a renderer naming the gatherer is
    /// non-conformant.
    pub gathered_by: String,
}

impl FeedMirror {
    pub fn new(
        subject: EntityRef,
        entries: Vec<EntityRef>,
        gathered_at: u64,
        gathered_by: impl Into<String>,
    ) -> Self {
        Self { subject, entries, gathered_at, gathered_by: gathered_by.into() }
    }

    pub fn data_value(&self) -> Value {
        Value::Map(vec![
            (text("subject"), self.subject.to_value()),
            (
                text("entries"),
                Value::Array(self.entries.iter().map(EntityRef::to_value).collect()),
            ),
            (text("gathered_at"), uinteger(self.gathered_at)),
            (text("gathered_by"), text(self.gathered_by.clone())),
        ])
    }

    pub fn to_entity(&self) -> Result<Entity, String> {
        Entity::new(FEED_MIRROR_TYPE, to_ecf(&self.data_value()))
            .map_err(|e| format!("feed mirror: {e}"))
    }

    /// Decode a mirror read out of `namespace`'s tree.
    ///
    /// **`gathered_by` is cross-checked against the namespace**, for `FEED-R1`'s
    /// reason one type over: the record is the gatherer's own signed statement,
    /// and a body naming somebody else as the assembler would be a claim the
    /// structure already contradicts. Same move as `Share`'s dropped `from`
    /// field — where the structure supplies the fact, a field restating it is an
    /// untrusted second source — except that here §6's CDDL requires the field,
    /// so it is checked rather than ignored.
    ///
    /// ⚠ **There is no `complete` field and §6.1 rule 2 says there never will
    /// be.** A short mirror is not a defective one.
    pub fn from_entity(entity: &Entity, namespace: &str) -> Result<Self, FeedError> {
        let map = body_map(entity, FEED_MIRROR_TYPE)?;
        let subject = match field(&map, "subject") {
            None => return Err(FeedError::Malformed("subject")),
            // **`any-reference` since v0.2** — a live subject is a timeline
            // mirror, which is the kind that makes a mirror usable as a source
            // leg. `entries` two lines down stays pinned and that asymmetry is
            // the design, not an oversight.
            Some(v) => reference(v, "subject", false)?,
        };
        let entries = match field(&map, "entries") {
            None => return Err(FeedError::Malformed("entries")),
            Some(v) => reference_list(v, "entries", true)?,
        };
        let gathered_by = required_text(&map, "gathered_by")?;
        if gathered_by != namespace {
            return Err(FeedError::AuthorIsNotTheNamespace {
                author: gathered_by,
                namespace: namespace.to_string(),
            });
        }
        Ok(FeedMirror {
            subject,
            entries,
            gathered_at: required_uint(&map, "gathered_at")?,
            gathered_by,
        })
    }
}

// ---------------------------------------------------------------------------
// app/feed/follow
// ---------------------------------------------------------------------------

/// §2.4's `app/feed/follow` — a reader's durable subscription.
///
/// **This is the reader's private data.** Nothing in the convention publishes
/// it and nothing requires a publisher to learn who follows them; publishing a
/// follow list is a separate, voluntary act and is not specified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Follow {
    /// Whose feed this follows — **a NAMESPACE, not a record**. That is the
    /// discriminator against `app/share/follow`; see [`FEED_FOLLOW_TYPE`].
    pub subject: String,
    /// **A petname: local, chosen by the reader, never authoritative.** The
    /// answer to *"I cannot read a public key"* that requires no naming
    /// authority at all, because the name lives in the reader's own tree and is
    /// never transmitted as a claim about anyone.
    pub label: Option<String>,
    /// The identifier as typed or scanned, **for provenance display only**.
    pub via: Option<String>,
    /// When this follow was created, ms.
    pub since: u64,
    /// The declared cursor — see the module doc's ⚠ on why this cannot satisfy
    /// `FEED-R14` and why we emit it anyway.
    pub cursor: Option<Hash>,
}

impl Follow {
    pub fn new(subject: impl Into<String>, since: u64) -> Self {
        Self { subject: subject.into(), label: None, via: None, since, cursor: None }
    }

    pub fn data_value(&self) -> Value {
        let mut fields = vec![(text("subject"), text(self.subject.clone()))];
        if let Some(label) = &self.label {
            fields.push((text("label"), text(label.clone())));
        }
        if let Some(via) = &self.via {
            fields.push((text("via"), text(via.clone())));
        }
        fields.push((text("since"), uinteger(self.since)));
        if let Some(cursor) = &self.cursor {
            fields.push((text("cursor"), ecf_bytes(cursor.to_bytes())));
        }
        Value::Map(fields)
    }

    pub fn to_entity(&self) -> Result<Entity, String> {
        Entity::new(FEED_FOLLOW_TYPE, to_ecf(&self.data_value()))
            .map_err(|e| format!("feed follow: {e}"))
    }

    pub fn from_entity(entity: &Entity) -> Result<Self, FeedError> {
        let map = body_map(entity, FEED_FOLLOW_TYPE)?;
        Ok(Follow {
            subject: required_text(&map, "subject")?,
            label: field(&map, "label").and_then(|v| v.as_text()).map(str::to_string),
            via: field(&map, "via").and_then(|v| v.as_text()).map(str::to_string),
            since: required_uint(&map, "since")?,
            cursor: hash_field(&map, "cursor")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::{EmbedData, EmbedPayload};

    const ME: &str = "QmAuthorPeerIdInMixedCase";
    const THEM: &str = "QmOtherPeerIdEntirely";

    fn h(seed: &str) -> Hash {
        Hash::compute("test/note", seed.as_bytes())
    }

    /// **`signature_key` is derived from the kernel's own path builder, and a
    /// derivation is a claim that needs a gate.**
    ///
    /// `FeedEntry::signature_path` calls `invariant_signature_path`; the
    /// peer-relative *key* form is that same path with its peer segment removed,
    /// because a foreign-tree read takes a key and not a path. Restating the
    /// literal in two places is C15's defect on a path this module needs both
    /// forms of — so the key is computed from the path, and this asserts the two
    /// still agree. A kernel that changed the invariant-pointer layout would red
    /// here rather than produce a key nothing answers to.
    #[test]
    fn the_signature_key_is_the_kernels_own_path_with_the_peer_segment_removed() {
        let hash = h("an entry");
        let path = entity_hash::invariant_signature_path(ME, &hash);
        let key = signature_key(ME, &hash);

        assert_eq!(path, format!("/{ME}/{key}"), "the key is the path's tail");
        assert!(!key.starts_with('/'), "a key is peer-relative and unrooted");
        assert!(key.starts_with("system/signature/"), "V7 §3.5's invariant pointer");
        assert!(key.ends_with(&hash.to_hex()), "and it names the entry it covers");
        // A different signer's key differs only by the segment we removed, so
        // stripping the wrong thing would collapse two authors onto one key.
        assert_eq!(signature_key(THEM, &hash), key, "the TAIL is author-independent");
    }

    /// The two index key forms, and the one that is ours rather than the
    /// convention's. Pinned by literal — a test spelled in the constants would
    /// follow any rename and could never catch one.
    #[test]
    fn the_index_and_entry_keys_are_the_paths_we_publish_at() {
        assert_eq!(index_head_key(), "app/feed/index");
        assert_eq!(index_page_key(0), "app/feed/index/0");
        assert_eq!(index_page_key(12), "app/feed/index/12");
        // Ours, not the convention's — §2 leaves the entry path local.
        assert_eq!(entry_prefix(), "app/feed/entries/");
        let hash = h("an entry");
        assert_eq!(entry_key(&hash), format!("app/feed/entries/{}", hash.to_hex()));
    }

    fn text_body(s: &str) -> EmbedNode {
        EmbedNode::new(
            "text/plain",
            EmbedData::new(EmbedPayload::Inline(s.as_bytes().to_vec()), s),
        )
    }

    fn entry() -> FeedEntry {
        FeedEntry::new(ME, 1_757_000_000_000, text_body("hello"))
    }

    // -- the entry --------------------------------------------------------

    /// `FEED-1`'s shape: the floor entry, no reply, no context, no prev.
    #[test]
    fn a_bare_entry_round_trips() {
        let e = entry();
        assert_eq!(FeedEntry::from_entity(&e.to_entity().unwrap(), ME).unwrap(), e);
    }

    /// `FEED-2` — the reference atom and the threading fields together.
    #[test]
    fn an_entry_carrying_every_optional_term_round_trips() {
        let mut e = entry();
        e.reply = Some(Reply {
            root: EntityRef::pin(THEM, h("root")),
            parent: EntityRef::pin(THEM, h("parent")),
        });
        e.context = Some(EntityRef::live(THEM, "/app/feed/topics/rust").with_seen(h("topic")));
        e.prev = Some(h("previous"));
        e.attachments = vec![
            EntityRef::pin(ME, h("photo")),
            EntityRef::live(THEM, "/app/feed/living-doc"),
        ];
        assert_eq!(FeedEntry::from_entity(&e.to_entity().unwrap(), ME).unwrap(), e);
    }

    /// **`FEED-3`, the forgery gate.** An entry found under one namespace
    /// claiming another author is invalid — and it is its own outcome, not
    /// "malformed", because *"corrupt"* and *"claims to be by somebody else"*
    /// are different reports and only one of them is interesting.
    #[test]
    fn an_entry_whose_author_is_not_its_namespace_is_rejected() {
        let e = entry();
        assert_eq!(
            FeedEntry::from_entity(&e.to_entity().unwrap(), THEM),
            Err(FeedError::AuthorIsNotTheNamespace {
                author: ME.into(),
                namespace: THEM.into()
            })
        );
    }

    /// **`FEED-R6`** — §2.2.1's table is a rule, not a preference: a reply must
    /// not become as trustworthy as whatever currently answers a location, and
    /// a parent must not be editable underneath its replies.
    #[test]
    fn a_live_reference_in_a_pinned_site_is_refused() {
        let build = |root: EntityRef, parent: EntityRef| {
            let mut e = entry();
            e.reply = Some(Reply { root, parent });
            e.to_entity().unwrap()
        };
        let live = || EntityRef::live(THEM, "/app/feed/entries/x");
        let pin = || EntityRef::pin(THEM, h("p"));

        assert_eq!(
            FeedEntry::from_entity(&build(live(), pin()), ME),
            Err(FeedError::LiveReferenceWherePinRequired { field: "reply.root" })
        );
        assert_eq!(
            FeedEntry::from_entity(&build(pin(), live()), ME),
            Err(FeedError::LiveReferenceWherePinRequired { field: "reply.parent" })
        );
        assert!(FeedEntry::from_entity(&build(pin(), pin()), ME).is_ok());
    }

    /// The other half of §2.2.1's table, and the arm that catches
    /// over-application of the rule above: `context` and `attachments` take
    /// **either** atom, because *"part of a topic"* is often a maintained index
    /// and an attached *living* document is the case the live shape exists for.
    #[test]
    fn context_and_attachments_take_either_atom() {
        let mut e = entry();
        e.context = Some(EntityRef::live(THEM, "/app/feed/topics/rust"));
        e.attachments = vec![EntityRef::live(THEM, "/doc")];
        assert_eq!(FeedEntry::from_entity(&e.to_entity().unwrap(), ME).unwrap(), e);
    }

    /// **`FEED-R8` asserted as an ABSENCE.** A reader MUST NOT reject an entry
    /// for an implausible timestamp — so this pins that we do not check, which
    /// is exactly the kind of claim a later author "repairs" into a bug.
    #[test]
    fn an_implausible_created_at_is_not_a_reason_to_reject_an_entry() {
        for stamp in [0, 1, u64::MAX] {
            let e = FeedEntry::new(ME, stamp, text_body("x"));
            let decoded = FeedEntry::from_entity(&e.to_entity().unwrap(), ME).unwrap();
            assert_eq!(decoded.created_at, stamp);
        }
    }

    /// The body is an `embed-node` and needs no separate `Embed` entity — the
    /// blocker that was never one. A text post and an image post are one shape.
    #[test]
    fn a_text_post_and_an_image_post_are_the_same_entry_type() {
        let text_post = entry();
        let mut image_post = entry();
        image_post.body = EmbedNode::new(
            "image/png",
            EmbedData::new(EmbedPayload::Pointer(h("photo")), "a photo of a cat"),
        );
        assert_eq!(text_post.to_entity().unwrap().entity_type, FEED_ENTRY_TYPE);
        assert_eq!(image_post.to_entity().unwrap().entity_type, FEED_ENTRY_TYPE);
        assert_eq!(
            FeedEntry::from_entity(&image_post.to_entity().unwrap(), ME).unwrap(),
            image_post
        );
    }

    /// A body whose payload is refused fails the entry, carrying the embed's
    /// own outcome rather than flattening it.
    #[test]
    fn a_body_that_is_not_a_valid_embed_fails_the_entry_and_says_why() {
        let e = Entity::new(
            FEED_ENTRY_TYPE,
            to_ecf(&Value::Map(vec![
                (text("author"), text(ME)),
                (text("created_at"), uinteger(1)),
                (
                    text("body"),
                    Value::Map(vec![
                        (text("type"), text("app/embed/text/plain")),
                        (
                            text("data"),
                            Value::Map(vec![(
                                text("payload"),
                                EmbedPayload::Inline(b"x".to_vec()).to_value(),
                            )]),
                        ),
                    ]),
                ),
            ])),
        )
        .unwrap();
        assert_eq!(
            FeedEntry::from_entity(&e, ME),
            Err(FeedError::Body(EmbedError::MissingFallback))
        );
    }

    #[test]
    fn something_that_is_not_a_feed_entity_is_refused_by_its_type() {
        let e = Entity::new("app/site-page", to_ecf(&Value::Map(vec![]))).unwrap();
        assert_eq!(
            FeedEntry::from_entity(&e, ME),
            Err(FeedError::NotAFeedEntity { entity_type: "app/site-page".into() })
        );
        assert_eq!(
            IndexHead::from_entity(&e),
            Err(FeedError::NotAFeedEntity { entity_type: "app/site-page".into() })
        );
    }

    /// `FEED-R2`'s path is the kernel's invariant pointer, not a fourth
    /// expression of it.
    #[test]
    fn the_signature_path_is_the_kernels_invariant_pointer() {
        let e = entry();
        let hash = h("the entry");
        assert_eq!(
            e.signature_path(&hash),
            format!("/{ME}/system/signature/{}", hash.to_hex())
        );
    }

    // -- the index --------------------------------------------------------

    #[test]
    fn the_index_paths_are_the_two_the_convention_pins() {
        assert_eq!(index_head_path(ME), format!("/{ME}/app/feed/index"));
        assert_eq!(index_page_path(ME, 12), format!("/{ME}/app/feed/index/12"));
    }

    /// `? oldest` defaults to 0, and a head that has dropped nothing emits no
    /// key — so *declared zero* and *absent* are one fact and there is one
    /// encoding of it.
    #[test]
    fn a_head_round_trips_and_an_absent_oldest_is_zero() {
        let head = IndexHead::new(12, 1_757_000_000_000);
        let decoded = IndexHead::from_entity(&head.to_entity().unwrap()).unwrap();
        assert_eq!(decoded, head);
        assert_eq!(decoded.oldest, 0);

        let mut trimmed = head.clone();
        trimmed.oldest = 3;
        assert_eq!(
            IndexHead::from_entity(&trimmed.to_entity().unwrap()).unwrap(),
            trimmed
        );
    }

    /// **`oldest` is the one field here whose default is also a meaningful
    /// value, so absent and unreadable had collapsed into each other** — the
    /// `min_rollback_index` defect, in a different subsystem, with the same
    /// shape: the field carrying the navigational safety property got the only
    /// silent default in the decoder.
    ///
    /// Absent is a real zero (§4.2: *"default 0"*, and a publisher who has
    /// dropped nothing emits no key), so that arm must stay. **Present and
    /// unreadable is not zero** — it is a page floor we could not read, and
    /// reading it as 0 sends a reader walking back to page 0 through pages the
    /// publisher already dropped, silently, with the head claiming otherwise.
    #[test]
    fn an_oldest_that_is_present_but_unreadable_is_malformed_not_zero() {
        let head = |oldest: Value| {
            Entity::new(
                FEED_INDEX_HEAD_TYPE,
                to_ecf(&Value::Map(vec![
                    (text("current"), uinteger(12)),
                    (text("oldest"), oldest),
                    (text("updated_at"), uinteger(1)),
                ])),
            )
            .unwrap()
        };
        for bad in [text("5"), Value::Bool(true), Value::Array(vec![])] {
            assert_eq!(
                IndexHead::from_entity(&head(bad.clone())),
                Err(FeedError::Malformed("oldest")),
                "a present-but-unreadable oldest must not read as 0: {bad:?}"
            );
        }
        // The arm that must survive the fix: absent really is zero.
        let absent = Entity::new(
            FEED_INDEX_HEAD_TYPE,
            to_ecf(&Value::Map(vec![
                (text("current"), uinteger(12)),
                (text("updated_at"), uinteger(1)),
            ])),
        )
        .unwrap();
        assert_eq!(IndexHead::from_entity(&absent).unwrap().oldest, 0);
    }

    /// §4.5's one ordering contract: `entries` is **authored** order, not
    /// derived. A page that sorted would be a different page.
    #[test]
    fn a_page_preserves_the_authored_order_of_its_entries() {
        let entries = vec![
            EntityRef::pin(ME, h("newest")),
            EntityRef::pin(ME, h("older")),
            EntityRef::pin(ME, h("oldest")),
        ];
        let page = IndexPage::new(7, entries.clone(), 1);
        let decoded = IndexPage::from_entity(&page.to_entity().unwrap(), 7).unwrap();
        assert_eq!(decoded.entries, entries, "the publisher's sequence, verbatim");
    }

    /// §4.2's `page MUST equal its key`. A page answering to two numbers is
    /// `FEED-R11`'s renumbering hazard arriving through the body.
    #[test]
    fn a_page_whose_body_disagrees_with_its_key_is_refused() {
        let page = IndexPage::new(7, vec![EntityRef::pin(ME, h("e"))], 1);
        let entity = page.to_entity().unwrap();
        assert!(IndexPage::from_entity(&entity, 7).is_ok());
        assert_eq!(
            IndexPage::from_entity(&entity, 8),
            Err(FeedError::PageNumberDisagreesWithKey { declared: 7, key: 8 })
        );
    }

    /// `entries` is typed `reference`, so it is pinned-only — the same site
    /// rule as `reply`, one type over.
    #[test]
    fn a_page_entry_must_be_pinned() {
        let page = IndexPage::new(1, vec![EntityRef::live(ME, "/app/feed/entries/x")], 1);
        assert_eq!(
            IndexPage::from_entity(&page.to_entity().unwrap(), 1),
            Err(FeedError::LiveReferenceWherePinRequired { field: "entries" })
        );
    }

    /// **`FEED-R12` asserted as an absence, like `FEED-R8`.** §4.3 rule 5
    /// declines to name a page size on purpose; a reader that assumed one would
    /// be the defect. An empty page is also a legal page — *"a page that loses
    /// an entry is a page with fewer entries."*
    #[test]
    fn no_page_size_is_assumed_in_either_direction() {
        for count in [0usize, 1, 500] {
            let entries: Vec<EntityRef> =
                (0..count).map(|i| EntityRef::pin(ME, h(&format!("e{i}")))).collect();
            let page = IndexPage::new(0, entries.clone(), 1);
            let decoded = IndexPage::from_entity(&page.to_entity().unwrap(), 0).unwrap();
            assert_eq!(decoded.entries.len(), count);
        }
    }

    // -- the follow record ------------------------------------------------

    #[test]
    fn a_follow_round_trips_with_and_without_its_optional_terms() {
        let bare = Follow::new(THEM, 1_757_000_000_000);
        assert_eq!(Follow::from_entity(&bare.to_entity().unwrap()).unwrap(), bare);

        let mut full = bare.clone();
        full.label = Some("Ada".into());
        full.via = Some("entity+ref://QmOtherPeerIdEntirely/".into());
        full.cursor = Some(h("page 12"));
        assert_eq!(Follow::from_entity(&full.to_entity().unwrap()).unwrap(), full);
    }

    /// **The declared `cursor` cannot satisfy `FEED-R14`, and this test is
    /// where that is recorded rather than in prose alone.** §2.4 types it as a
    /// bare `content-hash`; §4.4 says the cursor is `{page, applied}` and *"this
    /// is why the cursor carries a number and not only a hash"*. We emit the
    /// declared field. If arch rules the map, this test is the one that
    /// changes — and until then nothing here can resume from a page number,
    /// because no field carries one.
    #[test]
    fn the_declared_cursor_is_a_bare_hash_and_carries_no_page_number() {
        let mut f = Follow::new(THEM, 1);
        f.cursor = Some(h("page 12"));
        let value = f.data_value();
        let cursor = value
            .as_map()
            .unwrap()
            .iter()
            .find(|(k, _)| k.as_text() == Some("cursor"))
            .map(|(_, v)| v.clone())
            .expect("cursor is emitted");
        assert!(
            cursor.as_bytes().is_some(),
            "§2.4 declares a bare content-hash, not a map: {cursor:?}"
        );
    }

    /// The subject is a **namespace**, and that is the discriminator against
    /// `app/share/follow` — a different mechanism with a different
    /// authorization model, which §2.4 declined to unify.
    #[test]
    fn a_feed_follow_is_not_a_share_follow() {
        let f = Follow::new(THEM, 1);
        assert_eq!(f.to_entity().unwrap().entity_type, "app/feed/follow");
        assert_ne!(f.to_entity().unwrap().entity_type, "app/share/follow");
    }

    // -----------------------------------------------------------------------
    // §6.0 / §6.0.1 — the subject, and the key derived from it
    // -----------------------------------------------------------------------

    /// ⛔ **`FEED-R27`, and it is the one this whole derivation exists for.**
    ///
    /// `at` and `via` are optional **hints** and `seen` is an expectation, so two
    /// gatherers naming one subject will not carry the same ones. If any of them
    /// entered the key, each would publish a correct, verifiable view at an
    /// address the other does not compute — **and nothing would error**, at
    /// either end, ever. *A derivation that includes an optional field is not a
    /// derivation.*
    ///
    /// This is the assertion `FEED-12` makes across two implementations; here it
    /// is made across two atoms.
    #[test]
    fn a_mirrors_key_ignores_every_optional_hint() {
        let root = Hash::compute("app/feed/entry", b"a thread root");
        let other = Hash::compute("app/feed/entry", b"something else");
        let hints = vec![crate::entity_ref::Hint::new(
            crate::entity_ref::HintTag::Origin,
            "https://mirror.example",
        )];
        let anchor = crate::entity_ref::Anchor { field: vec!["body".into()] };

        let bare = MirrorSubject::thread(ME, root);
        let hinted = MirrorSubject::from_reference(
            &EntityRef::pin(ME, root).with_via(hints.clone()).with_at(anchor.clone()),
        );
        assert_eq!(bare.key(), hinted.key(), "a hint entered a pinned key");

        let live_bare = MirrorSubject::timeline(ME);
        let live_hinted = MirrorSubject::from_reference(
            &live_bare
                .reference()
                .with_via(hints)
                .with_at(anchor)
                // `seen` is the sharpest of the three: it is the most tempting to
                // treat as identifying and §2.2 calls it an expectation.
                .with_seen(other),
        );
        assert_eq!(live_bare.key(), live_hinted.key(), "a hint entered a live key");

        // And the two kinds do not collide: a thread and a timeline of one peer
        // are different views and must not share a slot.
        assert_ne!(bare.key(), live_bare.key());
    }

    /// The live coordinate, **pinned to a literal computed outside this code.**
    ///
    /// §6.0.1 names `content_hash` with one argument where the corpus defines it
    /// with two (see [`path_coordinate`]), so our reading is a choice — and the
    /// key is exactly what `FEED-12` compares across seats. A test spelled in
    /// terms of `path_coordinate` would follow the choice silently; this one
    /// carries the bytes, so a drift on either side is visible as a diff.
    ///
    /// The expected value is `"00" || sha256(utf8("/{peer}/app/feed/index"))`,
    /// computed with `hashlib`, not with the function under test.
    #[test]
    fn the_live_coordinate_is_pinned_to_a_literal() {
        const ALICE: &str = "2AliceExamplePeerIdForKeyVectors";
        let subject = MirrorSubject::timeline(ALICE);
        assert_eq!(
            subject.coordinate().to_hex(),
            "00c11cf34c7c0dcf8c67494648b34e6690173fd07fb229b2f6d9b5ce2e23bbef4f",
            "the live-key derivation moved — this is a WIRE event, not a test fix"
        );
        assert_eq!(
            subject.key(),
            format!(
                "app/feed/mirrors/{}",
                "00c11cf34c7c0dcf8c67494648b34e6690173fd07fb229b2f6d9b5ce2e23bbef4f"
            ),
            "and the prefix is §6.0.1's"
        );
    }

    /// **`FEED-R26`'s codec half: a timeline subject is a LIVE reference, so
    /// `MirrorSubject::timeline` cannot express the shape the rule forbids.**
    ///
    /// ⚠ **Stated plainly, because a green test here is worth less than it
    /// looks:** the enforcement is the constructor's *argument list* — it takes no
    /// hash — so the violation is a compile error rather than a test failure, and
    /// this asserts only that the atom that comes out is the live one.
    /// **The consequence is measured where it bites**, over a real mirror read
    /// across two of the author's publishes:
    /// `feed_mirror::tests::a_timeline_mirror_is_still_found_after_the_author_posts_again`.
    #[test]
    fn a_timeline_subject_is_a_live_reference_and_not_a_pin() {
        let subject = MirrorSubject::timeline(ME);
        assert!(!subject.reference().is_pinned(), "a timeline subject is a live reference");
        // The address is a function of the peer id **alone** — which is what makes
        // it derivable by a reader who has never fetched anything from this
        // author, the hop §6.0 says a mirror exists to save.
        assert_eq!(subject.key(), MirrorSubject::timeline(ME).key());
        assert_ne!(subject.key(), MirrorSubject::timeline(THEM).key());
    }

    /// §6.0 since v0.2: the **subject** takes either atom and **`entries`**
    /// takes only a pin (`FEED-R28`).
    ///
    /// The asymmetry is the design. A mirror carries exact bytes, so an entry
    /// named live would be a mirror of whatever is there now — which is not a
    /// mirror — while a subject named live is the only way to say *this view is
    /// of that author's feed*.
    #[test]
    fn a_mirror_subject_takes_either_atom_and_its_entries_take_only_a_pin() {
        let entry = Hash::compute("app/feed/entry", b"one post");
        let timeline = MirrorSubject::timeline(ME);

        let record =
            FeedMirror::new(timeline.reference(), vec![EntityRef::pin(ME, entry)], 7, THEM);
        let entity = record.to_entity().unwrap();
        let back = FeedMirror::from_entity(&entity, THEM).expect("a live subject decodes");
        assert_eq!(back, record, "the timeline mirror did not round-trip");
        assert_eq!(
            MirrorSubject::from_reference(&back.subject),
            timeline,
            "the subject came back as a different subject"
        );

        // …and the entries do not widen with it.
        let live_entry = FeedMirror::new(
            timeline.reference(),
            vec![EntityRef::live(ME, "/app/feed/entries/whatever-is-there-now")],
            7,
            THEM,
        );
        assert!(
            matches!(
                FeedMirror::from_entity(&live_entry.to_entity().unwrap(), THEM),
                Err(FeedError::LiveReferenceWherePinRequired { field: "entries" })
            ),
            "a live entry reference was accepted into a mirror"
        );
    }
}
