//! `APP-CONVENTION-FEED` §4.3 and `FEED-R4` — **reading somebody else's feed.**
//!
//! [`crate::feed`] is the codec and [`crate::feed_publish`] is the emitter. This
//! is the consumer: walk an author's index, fetch the entries it names, and
//! decide — for each one, separately — whether anyone may be named for it.
//!
//! ## Why this is its own module, and why it is ASYNC
//!
//! It shipped inside `feed_publish` as phase 2b, gated `cfg(not(wasm32))`
//! because the *publisher* writes a directory and the browser has none. The
//! reader has no such dependency, and that gate was the only thing keeping a
//! defect out of sight: **[`FeedSource`] was synchronous, and every real source
//! of a foreign entity in this repo is `async`** — `SignedSession::resolve`,
//! `http_poll::fetch_*`, `foreign_cache::ensure_current`, every one of them.
//!
//! That is not an inconvenience, it is **structurally unusable in the browser**.
//! The 2b gate bridged it with a spin-loop `block_on` over a noop waker, which
//! is fine for `std::future::ready` and, against a `JsFuture`, spins the main
//! thread forever waiting on an event loop it is itself blocking. So a
//! synchronous reader could never have been wired to anything; the seam built to
//! make the reader *testable* had made it *unshippable*, and only a module that
//! nothing could import was hiding it.
//!
//! ***A trait shaped by its test double rather than by its production
//! implementation is a seam that fits nothing.*** Ask what the real caller
//! looks like before you name the trait — here the answer was one grep.
//!
//! The shape is [`crate::content_site::http_poll::BinSource`]'s, deliberately
//! and not by coincidence: a boxed `!Send` future, because the production
//! implementation of this trait will most often *wrap* that one, and a second
//! spelling of *"an async source in this crate"* is the drift C15 names. It
//! carries no `Send` bound for the same reason `BinSource` does not — a wasm
//! `JsFuture` is not `Send` and never will be.
//!
//! ## `FEED-R4` is a type, not a flag
//!
//! *"An entry whose signature is absent MUST be presented as unattributed, never
//! attributed to anyone."* So [`read_feed`] yields [`ReadEntry`], which pairs the
//! entry with an [`Attribution`] — there is no way to obtain the entry without
//! the verdict, and no `bool` for a renderer to get the wrong way round.
//! [`Unattributed`] has **six** reasons and they stay apart for AP40's reason:
//! *nobody signed this*, *we cannot check*, *it is signed by someone else* and
//! *the signature is forged* route to four different people.
//!
//! ## Verification needs no key distribution and no second fetch
//!
//! For a canonical Ed25519 peer id the **peer id embeds the public key**
//! (identity-multihash), so [`attribute`] derives it from `author` and never
//! resolves the identity entity. The `signer` field is then a **cross-check**
//! rather than an input: we rebuild the author's `system/peer` entity and
//! require the signature to name its hash.
//!
//! **That cross-check is a CONFORMANCE and REPORTING check, not a security
//! boundary, and the neuter is what established which.** Removing it does not
//! let a stranger's signature through — `verify_for_key_type` against the
//! author's key already refuses it — it just relabels the outcome
//! `BadSignature`. So what it buys is §1.1's `signer = author` being enforced at
//! all, and *"somebody else signed this"* staying apart from *"these bytes are
//! wrong"*. ***A check whose neuter only moves the error label is a reporting
//! check, and describing it as a defence is the overclaim a security review then
//! has to unpick.***
//!
//! **A peer id that does not carry its key is its own outcome**
//! ([`Unattributed::KeyNotInPeerId`]), never a verification failure: Ed448 and
//! the legacy SHA-256 form need an out-of-band key, and *"we could not check"*
//! must never render as *"this is not theirs"*.

// The Feed window follows a feed since 2026-09-10 (`views::feed` via
// `feed_fetch`); what is still unused is the half of this module's surface the
// window does not call — `read_by_enumeration`'s reporting arms and several
// `Unattributed` constructors that only a gate builds.
#![allow(dead_code)]

use std::future::Future;
use std::pin::Pin;

use entity_entity::Entity;
use entity_hash::Hash;

use crate::entity_ref::EntityRef;
use crate::feed::{entry_key, entry_prefix, index_head_key, index_page_key, signature_key, FeedEntry, FeedError, IndexHead, IndexPage};

// ---------------------------------------------------------------------------
// FEED-R4 — attribution
// ---------------------------------------------------------------------------

/// Whether an entry is attributable to its author, and if not, **why not**.
///
/// There is no `bool` and no `Option<()>` here on purpose: `FEED-R4` is a MUST
/// about what a reader **presents**, and a renderer handed a flag gets it the
/// wrong way round exactly once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attribution {
    /// `FEED-R2` satisfied: a `system/signature` at the invariant pointer, over
    /// this entry's hash, by a key the author's peer id derives to.
    Signed,
    /// Present it as **by nobody**. Never as by the author.
    Unattributed(Unattributed),
}

impl Attribution {
    /// Whether a renderer may name the author.
    ///
    /// **Spelled positively**, so a seventh [`Unattributed`] reason added later
    /// cannot silently start rendering as attributed — the
    /// `AppServerView::is_serving` defect, one subsystem over.
    pub fn may_name_the_author(&self) -> bool {
        matches!(self, Attribution::Signed)
    }
}

/// Why an entry could not be attributed. **Each names whose defect it is, and
/// two of them are not defects at all.**
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unattributed {
    /// **`FEED-R4`'s named case.** No signature entity at the invariant pointer.
    /// Not an error: an entry may legitimately arrive without one, and the rule
    /// is about what we then say, not about refusing it.
    NoSignature,
    /// **Ours, and not a judgement about the entry.** The author's peer id does
    /// not carry its own key (Ed448, or the legacy SHA-256 form), so we have
    /// nothing to verify against and would need an out-of-band key. *"We could
    /// not check"* must never render as *"this is not theirs"*.
    KeyNotInPeerId { peer_id: String },
    /// Theirs, malformed — the entity at the signature key is not a decodable
    /// `system/signature`.
    SignatureUnreadable { detail: String },
    /// Theirs — a well-formed signature over **a different entity**. The shape a
    /// signature copied from another entry takes.
    WrongTarget { target: Hash, entry: Hash },
    /// Theirs — the signature names an identity that is not the author's.
    /// Verification is not even attempted: a signature by somebody else is not a
    /// forgery to test, it is a different claim.
    SignerIsNotTheAuthor,
    /// Theirs — everything lines up and the bytes do not verify.
    BadSignature,
}

impl std::fmt::Display for Unattributed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unattributed::NoSignature => write!(f, "no signature was published for this entry"),
            Unattributed::KeyNotInPeerId { peer_id } => {
                write!(f, "{peer_id} does not carry its own key, so we cannot check")
            }
            Unattributed::SignatureUnreadable { detail } => {
                write!(f, "the signature does not decode: {detail}")
            }
            Unattributed::WrongTarget { target, entry } => write!(
                f,
                "the signature covers {} and this entry is {}",
                target.to_hex(),
                entry.to_hex()
            ),
            Unattributed::SignerIsNotTheAuthor => {
                write!(f, "the signature names a signer who is not the author")
            }
            Unattributed::BadSignature => write!(f, "the signature does not verify"),
        }
    }
}

/// **The whole of `FEED-R4`, as one pure function.**
///
/// Pure and native, for the reason every decision in this repo that matters is:
/// the only alternative is reaching it through a projection, a temp directory
/// and an async pump, and then every combination of *(peer id form × signature
/// present × target × signer × bytes)* costs a publish.
///
/// `signature` is `None` when nothing is published at the invariant pointer —
/// which is `FEED-R4`'s named case and **not** an error the caller should have
/// converted into one first.
pub fn attribute(author: &str, entry_hash: &Hash, signature: Option<&Entity>) -> Attribution {
    let Some(sig_entity) = signature else {
        return Attribution::Unattributed(Unattributed::NoSignature);
    };
    let sig = match entity_types::SignatureData::from_entity(sig_entity) {
        Ok(s) => s,
        Err(e) => {
            return Attribution::Unattributed(Unattributed::SignatureUnreadable {
                detail: format!("{e:?}"),
            })
        }
    };
    if sig.target != *entry_hash {
        return Attribution::Unattributed(Unattributed::WrongTarget {
            target: sig.target,
            entry: *entry_hash,
        });
    }

    // The author's key, out of the author's peer id. See the module doc — no
    // second fetch, and a peer id that does not carry one is its own answer
    // rather than a failure.
    let pid = entity_crypto::PeerId::from(author.to_string());
    let Some((pubkey, key_type_byte)) = pid.derive_public_key() else {
        return Attribution::Unattributed(Unattributed::KeyNotInPeerId {
            peer_id: author.to_string(),
        });
    };
    let Ok(key_type) = entity_crypto::KeyType::from_byte(key_type_byte) else {
        return Attribution::Unattributed(Unattributed::KeyNotInPeerId {
            peer_id: author.to_string(),
        });
    };

    // The cross-check, before the cryptography: does the signature name the
    // author's identity? A signature lifted from another entry by another author
    // can still verify against ITS OWN key, so the question "is this the author's
    // signature" is not answered by `verify` alone.
    let identity = entity_types::PeerData {
        public_key: pubkey.clone(),
        key_type: key_type_name(key_type).to_string(),
    };
    let expected_signer = match identity.to_entity() {
        Ok(e) => e.content_hash,
        // Our own encoder failing is not the publisher's defect, but there is no
        // outcome that says so and inventing one for an unreachable branch is
        // worse than folding it into "we could not check".
        Err(_) => {
            return Attribution::Unattributed(Unattributed::KeyNotInPeerId {
                peer_id: author.to_string(),
            })
        }
    };
    if sig.signer != expected_signer {
        return Attribution::Unattributed(Unattributed::SignerIsNotTheAuthor);
    }

    match entity_crypto::verify_for_key_type(
        key_type,
        &pubkey,
        &entry_hash.to_bytes(),
        &sig.signature,
    ) {
        Ok(()) => Attribution::Signed,
        Err(_) => Attribution::Unattributed(Unattributed::BadSignature),
    }
}

/// The `key_type` string a `system/peer` entity carries — **the kernel's
/// `KeyType::label()`**, which carries V7 §3.5's v7.66 pin in its own doc.
///
/// ⚠ **This function's doc used to say it "mirrors `RootProjector::algorithm`
/// rather than re-deriving", and it did not** — it was a second, independent
/// match over the same three arms, and the two disagreed on
/// `ExperimentalTest` (that one wrote `"ed25519"` through a wildcard). *A
/// comment claiming a factoring is not the factoring*, and the tell is the one
/// AGENTS.md gives: ask which module would have to change if the shared thing
/// changed. The answer was "both, independently", which is the definition of the
/// drift it claimed to avoid.
pub(crate) fn key_type_name(k: entity_crypto::KeyType) -> &'static str {
    k.label()
}

// ---------------------------------------------------------------------------
// The cursor — §4.4, and it is LOCAL STATE
// ---------------------------------------------------------------------------

/// A reader's position in one author's feed: **`{page, applied}`**, §4.4.
///
/// `page` is the index page the position is on and `applied` is the newest entry
/// the reader took from it. Reading is then §4.3 rule 4 — *fetch the head, read
/// down from `current` to your cursor, and stop* — which is `O(new)` instead of
/// `O(window)` on every poll. **An implementation with no cursor of any kind
/// re-reads its whole window forever, whether or not anything changed**, which
/// is what this reader did until now.
///
/// ## ⛔ It lives here, in the reader, and it has no `to_entity`
///
/// `FEED-R35` is a MUST NOT: *publish a reader's cursor position in any record
/// this convention defines.* §2.4 states the field's absence as normative and
/// §4.4 says the position *"is held wherever a reader keeps its own bookkeeping,
/// it is never a field on a published record, and a publisher never learns
/// it."* So this type is deliberately **not** in [`crate::feed`], which is the
/// wire codec: a position type sitting beside `Follow` is one somebody gives a
/// `to_entity` to. There is nothing to publish it with.
///
/// ## Why a page number and not just a hash
///
/// So it survives an edit. An author may remove the very entry a reader is
/// holding as its position; a bare hash then resolves to nothing and the reader
/// has no way back into the archive except from the top.
/// [`Resumed::FromPage`] is §4.4's `[MUST]` — *if `applied` no longer resolves,
/// resume from `page`* — and it costs one page of re-delivery rather than the
/// whole feed. We carried the v0.2 bare-hash field for two weeks and said in
/// writing that it could not satisfy `FEED-R14`; this is the shape that can.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    /// The index page [`applied`](Self::applied) is on. Stable: §4.3 rule 3
    /// forbids renumbering precisely because *"renumbering would invalidate
    /// every reader's cursor"*, so an entry never moves between pages.
    pub page: u64,
    /// The newest entry hash taken from that page.
    pub applied: Hash,
}

/// **How a walk related to the position it was given.** Seven facts, each
/// routing a caller somewhere different — which is the test for whether a split
/// is real (AP40).
///
/// | | what a caller does about it |
/// |---|---|
/// | [`FromNewest`](Self::FromNewest) | record the returned cursor; this was a first read |
/// | [`AtCursor`](Self::AtCursor) | the ordinary incremental poll; record and render |
/// | [`FromPage`](Self::FromPage) | **expect duplicates from that page** and dedupe — the position was edited away |
/// | [`Behind`](Self::Behind) | poll again, or raise the limit. The position is **unchanged** and there is more between here and it |
/// | [`ArchiveMovedOn`](Self::ArchiveMovedOn) | history was lost, not by us: the publisher dropped the page we were on |
/// | [`PositionAhead`](Self::PositionAhead) | a publisher anomaly — their index does not reach a page we already read |
/// | [`Unpositioned`](Self::Unpositioned) | this source has no index, so §4.4 does not apply to it at all |
///
/// Collapsing any pair costs a real answer. *Duplicates are coming* and *there
/// is a gap above your position* are opposite instructions; *the publisher
/// dropped your page* and *your page is ahead of theirs* are a normal §4.3
/// rule 3 event and a fault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resumed {
    /// No position was held. The window is the newest `limit` entries.
    FromNewest,
    /// The held `applied` was found on its page, so everything returned is
    /// strictly newer than it.
    AtCursor,
    /// **§4.4's `[MUST]`.** `applied` was not on its page — the author removed
    /// it — so the reader resumed from `page` and re-delivered that page whole.
    FromPage { page: u64 },
    /// The limit cut the walk short **before** it reached the held position, so
    /// there is a gap between the oldest entry returned and `held`. The cursor
    /// is not advanced: advancing over a gap skips it forever.
    Behind { held: u64 },
    /// The held page is older than `head.oldest`. §4.3 rule 3 lets a publisher
    /// drop old pages, so this is a normal event and not a fault — but the
    /// reader has lost the history between them and cannot get it back here.
    ArchiveMovedOn { held: u64, oldest: u64 },
    /// The held page is newer than `head.current`: the publisher's index no
    /// longer reaches a page we have already read. A conformant publisher does
    /// not produce this (rule 1 rewrites only the last page), so it is reported
    /// rather than quietly treated as a fresh read.
    PositionAhead { held: u64, current: u64 },
    /// The source served no index, so the window came from §4.3 rule 6's
    /// enumeration fallback. **There are no pages, so there is no
    /// `{page, applied}` to record** — a cursor held against this author's
    /// published leg does not transfer to their live one, and pretending it did
    /// would be a position over an order we reconstructed.
    Unpositioned,
}

/// A walk's result: the entries, how it related to the position it was given,
/// and the position to hold next.
///
/// ⚠ **`next` is `None` when the cursor MUST NOT be advanced**, not when
/// nothing happened — [`Resumed`] is what says which. The two cases are
/// [`Resumed::Behind`] (a gap above the held position) and
/// [`Resumed::Unpositioned`] (no pages to name one with). A caller that treats
/// `None` as *"keep the old one"* is correct in both.
#[derive(Debug, Clone, PartialEq)]
pub struct FeedWindow {
    pub entries: Vec<ReadEntry>,
    pub resumed: Resumed,
    pub next: Option<Cursor>,
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// One entry as a reader holds it: the bytes, their address, and **whether
/// anyone may be named for them**.
///
/// The three travel together because `FEED-R4` is a rule about presentation and
/// a struct that let you take the entry without the verdict would be the API
/// version of the defect it forbids.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadEntry {
    pub hash: Hash,
    pub entry: FeedEntry,
    pub attribution: Attribution,
    /// **What was obtained, byte-for-byte** — see [`Obtained`]. A reader that
    /// only renders ignores it; a reader that republishes must carry exactly
    /// this and nothing re-encoded from [`Self::entry`].
    pub obtained: Obtained,
}

/// The bytes as they arrived, and the evidence that travels with them.
///
/// ## Why a decoded entry is not enough to republish
///
/// `APP-CONVENTION-FEED` §6.1 rule 1 and the replication proposal's byte-
/// preservation `MUST` say the same thing from two tiers: **a republished entity
/// is bound byte-identically to the form it was obtained in, and MUST NOT be
/// re-encoded — including by decoding it through a type that does not fully
/// declare it.** [`ReadEntry::entry`] is exactly such a type: `FeedEntry` knows
/// §2.3's fields and V7 §2.6 obliges it to *ignore* the rest, so
/// `FeedEntry::from_entity(…).to_entity()` silently drops whatever a publisher
/// carried that we have not heard of.
///
/// ⛔ **And the loss is not a field, it is authorship.** A detached signature is
/// bound at `/{author}/system/signature/{hex(entry_hash)}`; move the hash by one
/// byte and the signature no longer names the entity, so `FEED-R4` obliges every
/// entry to render **unattributed**. *A complete, verifiable, correctly-walked
/// republication in which nobody wrote anything* — measured on another seat as
/// **3 of 3 hashes moved, with no error anywhere**, which is why this is a type
/// and not a convention.
///
/// The signature is carried for §6.1 rule 3's reason: *a mirror may carry an
/// author's signature and may never supply one.* A republisher that fetched the
/// entry and left the signature behind publishes bytes nobody can be named for.
#[derive(Debug, Clone, PartialEq)]
pub struct Obtained {
    /// The entry entity exactly as the source served it.
    pub entity: Entity,
    /// The author's detached `system/signature`, if the source had one. `None`
    /// is `FEED-R4`'s named case and **not** an error.
    pub signature: Option<Entity>,
}

/// Recompute an entity's address from its own bytes.
///
/// **The one place byte fidelity is decided, and it is at the boundary bytes
/// ARRIVE at** — a hash carried in an `Entity` struct is a *claim* about the
/// bytes beside it, and on any source that is not root-anchored nothing has
/// checked it. `SignedSession` verifies the published leg; a live peer's
/// `system/tree:get` and another peer's republished walk do not, and those are
/// exactly the two legs this convention adds.
///
/// Cheap on purpose: one re-encode of the body we already hold, compared once.
pub(crate) fn recomputed_hash(entity: &Entity) -> Result<Hash, String> {
    Entity::new(&entity.entity_type, entity.data.clone())
        .map(|rebuilt| rebuilt.content_hash)
        .map_err(|e| format!("{e:?}"))
}

/// Why a feed read stopped.
#[derive(Debug, Clone, PartialEq)]
pub enum FeedReadError {
    /// The head is absent or unreadable. **Fatal for us and not for the
    /// convention** — see [`read_feed`]'s note on §4.3 rule 6.
    NoIndex { detail: String },
    /// The head decoded and a page it names did not. Its own outcome: the
    /// publisher committed to a page number in a signed root and then did not
    /// serve it, which is a different report from *"this author has no index"*.
    PageMissing { page: u64, detail: String },
    /// **We could not look**, for an entry the index names. Kept apart from a
    /// short view (the entry is genuinely not served) because they are opposite
    /// facts: *the publisher unpublished this post* against *we could not reach
    /// the origin*. Shortening the feed on the second would be D24's rule
    /// pointed at a reader — an outage rendered as missing posts, which a person
    /// cannot tell from an author who deleted something.
    EntryUnreachable { entry: Hash, detail: String },
    /// **The index named one entry and the source served different bytes.**
    ///
    /// Its own outcome and **fatal**, because §6.1 rule 2 draws exactly this
    /// line: *a mirror can omit but never substitute.* An omission is an
    /// ordinary short view ([`read_one`] returns `Ok(None)` for it); a
    /// substitution is a source answering a pinned reference with something
    /// else, and continuing would render bytes under an address that does not
    /// name them — with the signature we fetched by that address attached.
    ///
    /// Unreachable on a root-anchored published leg, where `SignedSession`
    /// refuses first. It is the live leg and the mirror leg that need it.
    Substituted { named: Hash, served: Hash },
    /// A feed entity that does not decode.
    Malformed { key: String, source: FeedError },
}

impl std::fmt::Display for FeedReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FeedReadError::NoIndex { detail } => write!(f, "no readable feed index: {detail}"),
            FeedReadError::PageMissing { page, detail } => {
                write!(f, "the head names page {page} and it does not resolve: {detail}")
            }
            FeedReadError::EntryUnreachable { entry, detail } => write!(
                f,
                "the index names entry {} and it could not be fetched: {detail}",
                entry.to_hex()
            ),
            FeedReadError::Substituted { named, served } => write!(
                f,
                "the index names entry {} and the bytes served hash to {} — \
                 a source may omit an entry, never substitute one",
                named.to_hex(),
                served.to_hex()
            ),
            FeedReadError::Malformed { key, source } => write!(f, "{key}: {source}"),
        }
    }
}

/// What a reader needs from a tree.
///
/// **Async, and shaped like [`BinSource`](crate::content_site::http_poll::BinSource)
/// on purpose** — see the module doc. A boxed `!Send` future, and an owned key,
/// because the future must not borrow `self`: the production implementation
/// clones what it needs and hands back a `JsFuture`.
pub trait FeedSource {
    /// Resolve a peer-relative key. `Ok(None)` is a **genuine absence**, which
    /// `FEED-R4` and §4.3 rule 6 both treat as an ordinary fact; `Err` is a
    /// failure to look, which is a different report.
    fn get(&self, relative_key: String) -> Pin<Box<dyn Future<Output = Result<Option<Entity>, String>>>>;

    /// The immediate child key **names** bound under a peer-relative prefix, for
    /// §4.3 rule 6's fallback — *"a reader that cannot fetch \[the index] falls
    /// back to enumerating the prefix."*
    ///
    /// **`Ok(None)` means this source cannot enumerate at all**, which is a
    /// different fact from `Ok(Some(vec![]))` (*it enumerated, and there is
    /// nothing there*) and from `Err` (*it tried and could not look*). Only the
    /// first says *"the fallback is unavailable, so an absent index is
    /// terminal"*.
    ///
    /// **Defaulted, so no existing source changed.** A live peer overrides it —
    /// `system/tree:get` at a trailing-slashed prefix answers with a listing —
    /// and the published arm keeps today's behaviour byte-identical until
    /// somebody implements it. `SignedSession::enumerate` exists and is bounded,
    /// so the HTTP arm *can* adopt this; it is one method, not a redesign. The
    /// reason it has not is `A-38`: the entry prefix is **ours**, not the
    /// convention's (§2 — *"the cross-impl contract is the type tag, not the
    /// path"*), so enumerating it reads a publisher running this implementation
    /// and is not yet a cross-impl mechanism.
    fn list(
        &self,
        relative_prefix: String,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Vec<String>>, String>>>> {
        let _ = relative_prefix;
        Box::pin(std::future::ready(Ok(None)))
    }
}

/// Walk a feed: head → pages, newest page first → entries → attribution.
///
/// Stops at `limit` entries. Ordering is **as published** — newest-first within
/// a page, pages walked from `current` down — and nothing here sorts: §4.5 makes
/// the order authored, and `created_at` is explicitly not an ordering authority.
///
/// ## ⚠ For us the index is not an optimization, and §4.3 rule 6 says it is
///
/// Rule 6: *"a reader that cannot fetch it falls back to **enumerating the
/// prefix** — slower, same answer."* **The convention pins no prefix for
/// entries** (§2: *"the cross-impl contract is the type tag, not the path"*), so
/// the fallback is a type-filtered query over the universal tree — and our
/// foreign-tree consumer resolves a **key**, with no type query at all. So
/// `NoIndex` is terminal here where the convention says it should be a slow
/// path. Routed as `A-38` rather than worked around; a synthetic prefix walk
/// would make us the seat that decided where entries live.
pub async fn read_feed<S: FeedSource + ?Sized>(
    src: &S,
    author: &str,
    limit: usize,
) -> Result<Vec<ReadEntry>, FeedReadError> {
    Ok(read_feed_from(src, author, limit, None).await?.entries)
}

/// [`read_feed`], resuming from a held position — §4.3 rule 4's `O(new)` poll.
///
/// With `cursor: None` this is byte-for-byte the old walk, which is why
/// [`read_feed`] is a one-line delegation and no existing caller moved.
///
/// With a position held, the walk **reads down from `head.current` and stops at
/// it**, so an unchanged feed costs one head fetch, one page fetch and zero
/// entry fetches. The three ways that can fail to be simple — the position was
/// edited away, the publisher dropped the page it was on, the limit ran out
/// before we got there — are [`Resumed`]'s subject, and each one is a different
/// instruction to the caller.
///
/// ## ⚠ The gap rule, which is the only place this can lose data
///
/// If `limit` is reached before the walk arrives at the held position, the
/// entries returned are the newest ones and **there is a gap between the oldest
/// of them and the position**. Advancing the cursor to the newest would make
/// that gap permanent: the next poll reads down to the new position and stops
/// above the entries it skipped. So `next` is `None` and the caller keeps what
/// it has ([`Resumed::Behind`]).
///
/// **A FRESH truncated read is not the same case and does advance.** A reader
/// with no position never claimed the older entries, so the newest `limit` are
/// its position and the backlog below is a *backfill* question, not a *what is
/// new* one. Conflating the two would either lose entries or make a first read
/// unable to establish a cursor at all — and that second failure is the defect
/// this function exists to remove.
pub async fn read_feed_from<S: FeedSource + ?Sized>(
    src: &S,
    author: &str,
    limit: usize,
    cursor: Option<&Cursor>,
) -> Result<FeedWindow, FeedReadError> {
    let head_entity = match src.get(index_head_key().to_string()).await {
        Err(detail) => return Err(FeedReadError::NoIndex { detail }),
        Ok(Some(entity)) => entity,
        // **No head is not "no feed" — it is §4.3 rule 6.** And for a LIVE peer
        // it is the ordinary case, not the exotic one: the index is a publish
        // artifact (`plan_index` builds it on the way out, into the out-dir),
        // so an author's own tree holds entries and no index at all. A reader
        // that stopped here would tell you a peer with three posts has no feed.
        Ok(None) => {
            return Ok(FeedWindow {
                entries: read_by_enumeration(src, author, limit).await?,
                // No pages, so no `{page, applied}`. Reported rather than
                // silently dropping a cursor the caller handed us.
                resumed: Resumed::Unpositioned,
                next: None,
            })
        }
    };
    let head = IndexHead::from_entity(&head_entity).map_err(|source| FeedReadError::Malformed {
        key: index_head_key().into(),
        source,
    })?;

    // Where does the held position sit relative to what the publisher still
    // keeps? Only one of the three answers lets the walk stop early, and the
    // other two are facts worth reporting rather than a silent full read.
    let (stop_at, out_of_range) = match cursor {
        None => (None, None),
        Some(c) if c.page > head.current => (
            None,
            Some(Resumed::PositionAhead { held: c.page, current: head.current }),
        ),
        Some(c) if c.page < head.oldest => (
            None,
            Some(Resumed::ArchiveMovedOn { held: c.page, oldest: head.oldest }),
        ),
        Some(c) => (Some(c), None),
    };

    let mut out: Vec<ReadEntry> = Vec::new();
    // The NEWEST entry actually taken, with the page it came from — that is the
    // cursor's own definition, and it is `first` because the walk is
    // newest-first throughout.
    let mut newest_taken: Option<(u64, Hash)> = None;
    let mut reached_position = stop_at.is_none();
    let mut hit_applied = false;
    let mut truncated = false;
    let mut page = head.current;

    loop {
        let key = index_page_key(page);
        let entity = src
            .get(key.clone())
            .await
            .map_err(|detail| FeedReadError::PageMissing { page, detail })?
            .ok_or_else(|| FeedReadError::PageMissing {
                page,
                detail: "the page does not resolve".into(),
            })?;
        // The key is passed in, because §4.2 makes `page` MUST equal its key and
        // a decoder that never learns which key it read from cannot check it.
        let decoded = IndexPage::from_entity(&entity, page)
            .map_err(|source| FeedReadError::Malformed { key: key.clone(), source })?;

        for reference in &decoded.entries {
            // §2.2.1 types `entries` as `reference`, i.e. pinned-only, and
            // `IndexPage::from_entity` already refused anything else — so this
            // is a pin and its hash is the entry's identity.
            let EntityRef::Pinned { hash, .. } = reference else { continue };
            // **Before the limit check, deliberately.** Arriving at the position
            // means there is nothing left to take, so a budget that happens to
            // run out on the same step must not be reported as a gap.
            if let Some(c) = stop_at {
                if page == c.page && *hash == c.applied {
                    hit_applied = true;
                    break;
                }
            }
            if out.len() >= limit {
                truncated = true;
                break;
            }
            let Some(read) = read_one(src, author, hash).await? else { continue };
            if newest_taken.is_none() {
                newest_taken = Some((page, read.hash));
            }
            out.push(read);
        }

        if truncated {
            break;
        }
        if let Some(c) = stop_at {
            if page == c.page {
                reached_position = true;
                break;
            }
        }
        if page <= head.oldest {
            break;
        }
        page -= 1;
    }

    let resumed = match (out_of_range, stop_at) {
        (Some(r), _) => r,
        (None, None) => Resumed::FromNewest,
        (None, Some(c)) if !reached_position => Resumed::Behind { held: c.page },
        (None, Some(c)) => {
            if hit_applied {
                Resumed::AtCursor
            } else {
                // §4.4's MUST: `applied` was not on its page, so the whole page
                // came back and the caller may see entries it already has.
                Resumed::FromPage { page: c.page }
            }
        }
    };

    let next = match newest_taken {
        // The gap rule. See the doc comment — this is the one `None` that is a
        // refusal to advance rather than an absence.
        Some(_) if !reached_position => None,
        Some((page, applied)) => Some(Cursor { page, applied }),
        // Nothing taken: a still-valid position stands, and a lost or absent
        // one stays lost. `stop_at` is `None` in exactly the second case.
        None => stop_at.cloned(),
    };

    Ok(FeedWindow { entries: out, resumed, next })
}

/// One entry and its verdict. `Ok(None)` when the index names an entry the
/// publisher does not serve — **§4.3 rule 6's other direction**: the index is
/// not the authority, so a name it carries that resolves to nothing is a short
/// view, not a corrupt feed.
async fn read_one<S: FeedSource + ?Sized>(
    src: &S,
    author: &str,
    hash: &Hash,
) -> Result<Option<ReadEntry>, FeedReadError> {
    let key = entry_key(hash);
    // **Not `PageMissing { page: 0 }`.** That is what this said until the review,
    // and it rendered a failed *entry* fetch as *"the head names page 0 and it
    // does not resolve"* — a sentence about a different subject, pointing whoever
    // read it at the index. AP40's cost is not the collapsed value, it is the
    // wrong sentence that comes out of it.
    let Some(entity) = src
        .get(key.clone())
        .await
        .map_err(|detail| FeedReadError::EntryUnreachable { entry: *hash, detail })?
    else {
        return Ok(None);
    };
    // **The pin is checked against the BYTES, not against the struct's own
    // claim.** `Entity.content_hash` is a field a source filled in; on the two
    // legs this convention adds nothing has verified it, and every downstream
    // act — which signature we fetch, which address we republish under — keys
    // off this hash.
    let served = recomputed_hash(&entity).map_err(|_| FeedReadError::Malformed {
        key: key.clone(),
        source: FeedError::Malformed("the served entity does not re-address"),
    })?;
    if served != *hash {
        return Err(FeedReadError::Substituted { named: *hash, served });
    }
    finish_entry(src, author, *hash, &key, entity).await.map(Some)
}

/// Decode one fetched entry and attach its `FEED-R4` verdict.
///
/// **Split out of [`read_one`] so both arms share ONE expression of it.** The
/// index arm arrives here with a hash it looked the entity up by; the
/// enumeration arm arrives with an entity it already holds and takes the hash
/// from the bytes. A second copy of "decode, fetch the signature, attribute"
/// is the drift C15 exists to refuse — and it would be the copy that quietly
/// stopped attributing.
pub(crate) async fn finish_entry<S: FeedSource + ?Sized>(
    src: &S,
    author: &str,
    hash: Hash,
    key: &str,
    entity: Entity,
) -> Result<ReadEntry, FeedReadError> {
    let entry = FeedEntry::from_entity(&entity, author)
        .map_err(|source| FeedReadError::Malformed { key: key.to_string(), source })?;

    // A signature we could not FETCH and a signature that is not there are the
    // same fact to `FEED-R4` — both mean we hold no signature — so a lookup
    // failure lands on `NoSignature` rather than failing the read. What must not
    // happen is either of them rendering as attributed.
    let sig = src.get(signature_key(author, &hash)).await.ok().flatten();
    let attribution = attribute(author, &hash, sig.as_ref());
    Ok(ReadEntry {
        hash,
        entry,
        attribution,
        // Both halves kept **as served**. See [`Obtained`] — this is the only
        // form a republisher may bind, and the signature is the half a tidy
        // implementation drops because rendering never needs it.
        obtained: Obtained { entity, signature: sig },
    })
}

/// §4.3 rule 6 — *"a reader that cannot fetch \[the index] falls back to
/// **enumerating the prefix** — slower, same answer."*
///
/// Reached when the head is **genuinely absent** (`Ok(None)`), never when it
/// could not be fetched: a 403 or an outage is a failure to look, and walking
/// the prefix afterwards would turn *"we could not read your index"* into a
/// silently shorter feed.
///
/// ## Two bounds, and neither is hidden
///
/// **The prefix is ours.** [`entry_prefix`] is a local binding choice (§2: *"the
/// cross-impl contract is the type tag, not the path"*) and its own doc says it
/// exists so this fallback has something to enumerate. So this reads a
/// publisher running **this** implementation; `A-38` is the routed question of
/// what a cross-impl reader should do, and this does not answer it.
///
/// **The order is RECONSTRUCTED, not authored.** §4.5 makes the order authored
/// and the index is what carries it — a prefix listing is in content-hash order,
/// which is to say none. So this sorts by [`crate::feed_tree::sort_key`], the
/// same `(created_at, content_hash)` total order the author's own publisher
/// assigns pages with, and reverses it for §4.5's newest-first presentation.
/// That is the best reconstruction available and it is a reconstruction; the arm
/// is logged rather than the two being presented as interchangeable.
///
/// **A row that does not decode is SKIPPED, not fatal** — the opposite of the
/// index arm, deliberately. There, the index vouched for the entry and a name it
/// carries that will not decode is a corrupt feed. Here the prefix is just a
/// place in a stranger's tree, anything may be bound under it, and one bad row
/// must not hide the rest. That is `feed_tree::read_owned_feed`'s own rule for
/// the same prefix read from the inside.
async fn read_by_enumeration<S: FeedSource + ?Sized>(
    src: &S,
    author: &str,
    limit: usize,
) -> Result<Vec<ReadEntry>, FeedReadError> {
    let listed = src
        .list(entry_prefix().to_string())
        .await
        .map_err(|detail| FeedReadError::NoIndex { detail })?;
    let Some(names) = listed else {
        // *This source cannot enumerate*, which is the published arm today —
        // so an absent index stays terminal there and the message says which
        // of the two facts it is.
        return Err(FeedReadError::NoIndex {
            detail: "no head at the pinned key, and this source cannot enumerate the \
                     entry prefix to fall back on"
                .into(),
        });
    };

    let mut rows: Vec<ReadEntry> = Vec::new();
    for name in names {
        let key = format!("{}{name}", entry_prefix());
        let Some(entity) = src
            .get(key.clone())
            .await
            .map_err(|detail| FeedReadError::NoIndex { detail })?
        else {
            continue;
        };
        // **The hash comes from the BYTES, never from the key name** — and
        // since the review, never from `Entity.content_hash` either. §2.2.1
        // makes an entry's identity its own content hash, so *computing* it is
        // what keeps a mis-bound key, or a source that filled the field in by
        // hand, from sending us to look up somebody else's signature. There is
        // no pin to compare against on this arm, so the computed value simply
        // *is* the address.
        let Ok(hash) = recomputed_hash(&entity) else { continue };
        match finish_entry(src, author, hash, &key, entity).await {
            Ok(row) => rows.push(row),
            Err(FeedReadError::Malformed { .. }) => continue,
            Err(e) => return Err(e),
        }
    }

    rows.sort_by_key(|r| (r.entry.created_at, r.hash));
    rows.reverse(); // §4.5 — newest first, as the index arm presents them.
    rows.truncate(limit);
    tracing::info!(
        author = %author,
        entries = rows.len(),
        arm = "enumeration",
        "feed read without an index — order is reconstructed, not authored"
    );
    Ok(rows)
}

/// A minimal executor for the native side.
///
/// These futures are `!Send` by design (a wasm `JsFuture` is), so no runtime is
/// involved.
///
/// ⚠ **Native only, and the cfg is the guarantee rather than the doc comment.**
/// Spinning on a real `JsFuture` does not resolve it — it blocks the very event
/// loop that would — so this must never be reachable from a browser build, which
/// `not(target_arch = "wasm32")` makes structurally true instead of a rule
/// somebody has to remember.
///
/// It said `#[cfg(test)]` until `publish --gather` needed it: a CLI verb driving
/// an async `FeedSource` over synchronous file reads is exactly what this does,
/// and writing a second spinner beside it would be C15 on a loop that is
/// load-bearing for the gates as well.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    fn raw() -> RawWaker {
        fn noop(_: *const ()) {}
        fn clone(_: *const ()) -> RawWaker {
            raw()
        }
        RawWaker::new(std::ptr::null(), &RawWakerVTable::new(clone, noop, noop, noop))
    }
    let waker = unsafe { Waker::from_raw(raw()) };
    let mut cx = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    loop {
        if let Poll::Ready(v) = future.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

/// A whole tree in a map — the double every decision gate in this convention
/// runs against, shared with [`crate::feed_publish`]'s so there is one of them.
///
/// **It is async now**, which is the point of the split: a synchronous double is
/// satisfied by a synchronous trait, and that is what let a reader nothing could
/// wire look finished.
#[cfg(test)]
#[derive(Default, Clone)]
pub(crate) struct Tree(pub std::collections::BTreeMap<String, Entity>);

#[cfg(test)]
impl FeedSource for Tree {
    fn get(
        &self,
        relative_key: String,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Entity>, String>>>> {
        let found = self.0.get(&relative_key).cloned();
        Box::pin(std::future::ready(Ok(found)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The trait is usable by an implementation that cannot answer
    /// immediately** — which is every real one.
    ///
    /// A source whose future is not ready on first poll must still work. This is
    /// the property a synchronous `FeedSource` could not have, and it is the
    /// whole reason this module exists apart from `feed_publish`: the trait's
    /// old shape was decided by a test double that always had the answer in
    /// hand, and no real source ever does.
    #[test]
    fn a_source_that_answers_later_is_still_a_source() {
        struct Deferred(Tree, std::cell::Cell<u32>);
        impl FeedSource for Deferred {
            fn get(
                &self,
                relative_key: String,
            ) -> Pin<Box<dyn Future<Output = Result<Option<Entity>, String>>>> {
                let found = self.0 .0.get(&relative_key).cloned();
                    let mut polls = 0u32;
                // Pending twice, then ready — a stand-in for a network round
                // trip. A synchronous trait cannot express this at all.
                Box::pin(std::future::poll_fn(move |cx| {
                    polls += 1;
                    if polls < 3 {
                        cx.waker().wake_by_ref();
                        return std::task::Poll::Pending;
                    }
                    std::task::Poll::Ready(Ok(found.clone()))
                }))
            }
        }

        let (tree, author, _) = crate::feed_publish::tests::published_tree(3);
        let deferred = Deferred(tree, std::cell::Cell::new(0));
        let read = block_on(read_feed(&deferred, &author, 10)).expect("a slow source is a source");
        assert_eq!(read.len(), 3);
        assert!(read.iter().all(|r| r.attribution == Attribution::Signed));
    }

    /// A feed that is simply not there is `NoIndex`, and it is a different
    /// answer from an author who has published an empty one.
    #[test]
    fn an_author_with_no_feed_at_all_is_its_own_answer() {
        let nothing = Tree::default();
        assert!(matches!(
            block_on(read_feed(&nothing, "QmAuthor", 10)),
            Err(FeedReadError::NoIndex { .. })
        ));
    }

    // -- byte preservation, at the boundary bytes arrive at -----------------

    /// ⭐ **What a reader obtains is kept as served, so that it can be
    /// republished.**
    ///
    /// The whole of the closure precondition, checked where it is cheap: the
    /// bytes on [`Obtained::entity`] are the publisher's, not a re-encode of
    /// [`ReadEntry::entry`], and the author's signature came with them. A
    /// gatherer built on a reader that dropped either one publishes a feed
    /// nobody can be named for.
    #[test]
    fn what_a_reader_obtains_is_kept_as_served_and_its_signature_comes_with_it() {
        let (tree, author, report) = crate::feed_publish::tests::published_tree(3);
        let read = block_on(read_feed(&tree, &author, 10)).expect("the feed reads");
        assert_eq!(read.len(), 3);

        for row in &read {
            let served = tree.0.get(&entry_key(&row.hash)).expect("the tree served it");
            assert_eq!(
                row.obtained.entity.data, served.data,
                "the obtained bytes are not the served bytes"
            );
            assert_eq!(row.obtained.entity.content_hash, row.hash);
            assert!(
                row.obtained.signature.is_some(),
                "FEED-R2's signature was fetched to attribute with and then dropped — \
                 a republisher has nothing to carry"
            );
        }

        // And the round trip a naive gatherer would perform instead — this is
        // the control arm, asserted here so the defect has a name in our tree
        // and not only in the other seat's measurement.
        let re_encoded = read[0].entry.to_entity().unwrap();
        assert_eq!(
            re_encoded.content_hash, read[0].hash,
            "for a body we fully declare the re-encode happens to agree — which is \
             exactly why this is not a property to rely on"
        );
        assert!(report.entry_hashes.contains(&read[0].hash));
    }

    /// ⛔ **A source may omit an entry; it may never substitute one.**
    ///
    /// §6.1 rule 2 draws the line and the two legs this convention adds are
    /// where it bites: the published leg is root-anchored and `SignedSession`
    /// refuses first, but a live peer and a republishing peer each hand over an
    /// entity with nothing above them checking that it is the one the pin
    /// named. Rendering it would attach a signature fetched by the *named*
    /// hash to bytes that are not it.
    #[test]
    fn a_source_answering_a_pin_with_other_bytes_is_refused_rather_than_rendered() {
        let (mut tree, author, report) = crate::feed_publish::tests::published_tree(2);
        let named = report.entry_hashes[0];
        let other = tree
            .0
            .get(&entry_key(&report.entry_hashes[1]))
            .expect("the second entry is published")
            .clone();
        // The substitution: the second entry's bytes, served at the first's key.
        tree.0.insert(entry_key(&named), other);

        match block_on(read_feed(&tree, &author, 10)) {
            Err(FeedReadError::Substituted { named: n, served }) => {
                assert_eq!(n, named);
                assert_ne!(served, named);
            }
            other => panic!("a substituted entry was accepted: {other:?}"),
        }
    }

    /// ⭐ **`Entity.content_hash` is a claim, not a check — the address is
    /// computed.**
    ///
    /// The field is `pub`, and on every source that is not root-anchored it is
    /// simply what the far end put there. A reader that trusted it would fetch
    /// the signature at the *claimed* address, verify it against the *claimed*
    /// address, and attribute bytes that are somebody else's entirely.
    #[test]
    fn a_hash_a_source_filled_in_by_hand_does_not_decide_the_address() {
        let (mut tree, author, report) = crate::feed_publish::tests::published_tree(2);
        let named = report.entry_hashes[0];
        let mut forged = tree.0.get(&entry_key(&report.entry_hashes[1])).unwrap().clone();
        // Bytes of entry 2, wearing entry 1's address.
        forged.content_hash = named;
        tree.0.insert(entry_key(&named), forged);

        assert!(
            matches!(
                block_on(read_feed(&tree, &author, 10)),
                Err(FeedReadError::Substituted { .. })
            ),
            "the struct's own claim about its address was taken as the address"
        );
    }

    // -- the cursor ---------------------------------------------------------

    /// **A source with no index cannot be positioned against, and says so.**
    ///
    /// §4.3 rule 6's enumeration fallback is the live-peer path — the index is a
    /// publish artifact, so an author's own tree holds entries and no pages at
    /// all. There is no `{page, applied}` to record there, and a cursor held
    /// against that author's *published* leg does not transfer: the order on
    /// this leg is reconstructed by us, not authored by them.
    ///
    /// So the position is neither applied nor silently dropped. It is reported
    /// as inapplicable and the caller keeps what it had — which matters because
    /// `feed_route` walks both legs for one author, and a leg that quietly
    /// reset the position would make the other leg re-deliver its whole window
    /// on the next poll.
    #[test]
    fn a_source_with_no_index_reports_that_a_position_does_not_apply_to_it() {
        struct Enumerating(Tree);
        impl FeedSource for Enumerating {
            fn get(
                &self,
                relative_key: String,
            ) -> Pin<Box<dyn Future<Output = Result<Option<Entity>, String>>>> {
                self.0.get(relative_key)
            }
            fn list(
                &self,
                relative_prefix: String,
            ) -> Pin<Box<dyn Future<Output = Result<Option<Vec<String>>, String>>>> {
                let names: Vec<String> = self
                    .0
                     .0
                    .keys()
                    .filter_map(|k| k.strip_prefix(&relative_prefix).map(str::to_string))
                    .collect();
                Box::pin(std::future::ready(Ok(Some(names))))
            }
        }

        let (mut tree, author, report) = crate::feed_publish::tests::published_tree(3);
        tree.0.remove(index_head_key());
        let src = Enumerating(tree);

        // A position that was perfectly valid against this author's published
        // leg — which is the case the outcome is about.
        let held = Cursor { page: 0, applied: report.entry_hashes[0] };
        let w = block_on(read_feed_from(&src, &author, 10, Some(&held))).unwrap();
        assert_eq!(w.resumed, Resumed::Unpositioned);
        assert_eq!(w.entries.len(), 3, "the fallback still read the feed");
        assert_eq!(
            w.next, None,
            "and offered no position, because this source has no pages to name one with"
        );
    }

    /// **Seven ways a walk relates to the position it was given, and each has
    /// its own word.** The count is asserted so an eighth cannot quietly reuse
    /// one — the same reason `Unattributed`'s roster is pinned, and the same
    /// reason `Hydration` ships five labels rather than four.
    ///
    /// Two pairs are the ones a tidy implementation merges, and both would cost
    /// a real answer: [`Resumed::FromPage`] says *expect duplicates* while
    /// [`Resumed::Behind`] says *there is a gap above you* — opposite
    /// instructions; and [`Resumed::ArchiveMovedOn`] is a publisher exercising
    /// §4.3 rule 3 while [`Resumed::PositionAhead`] is one that cannot have.
    #[test]
    fn every_way_a_walk_relates_to_its_position_has_its_own_word() {
        let all = [
            Resumed::FromNewest,
            Resumed::AtCursor,
            Resumed::FromPage { page: 1 },
            Resumed::Behind { held: 1 },
            Resumed::ArchiveMovedOn { held: 0, oldest: 1 },
            Resumed::PositionAhead { held: 9, current: 1 },
            Resumed::Unpositioned,
        ];
        let words: std::collections::BTreeSet<String> =
            all.iter().map(|r| format!("{r:?}")).collect();
        assert_eq!(words.len(), all.len(), "two outcomes render alike: {words:?}");

        // Exhaustive, so an eighth variant is a compile error here rather than a
        // silent seventh-and-a-half.
        let _exhaustive = |r: Resumed| match r {
            Resumed::FromNewest
            | Resumed::AtCursor
            | Resumed::FromPage { .. }
            | Resumed::Behind { .. }
            | Resumed::ArchiveMovedOn { .. }
            | Resumed::PositionAhead { .. }
            | Resumed::Unpositioned => (),
        };
    }
}
