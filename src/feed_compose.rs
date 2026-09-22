//! The **composer** — the four verbs that author a feed into your own tree.
//!
//! Add an entry · remove an entry · add to a collection · remove from a
//! collection. That is the whole surface, and it is the operator's own scoping:
//! *"It's some basic UI. Add an entry, add something to a collection. Remove an
//! entry, remove something from a collection."*
//!
//! ## ⭐ Writing into your own tree IS publishing, and this module is the whole act
//!
//! There is no publish verb here, no origin, no signed root and no network,
//! **and that is not a reduced version of anything.** `APP-CONVENTION-FEED`'s
//! own model is that a peer holding entries serves them to anyone connected to
//! it, exactly as it serves everything else in its tree — so an entry written
//! here is readable by a live peer the moment the write lands.
//!
//! The **static** road — projecting a signed root and putting bytes at an origin
//! — is an *additional*, operational act that freezes a tree into a version
//! other peers can pull without the author being up. That road already exists
//! ([`crate::publish_axes`], [`crate::feed_publish`]) and it mints its own root
//! over whatever is in the tree when it runs. It is not this module's business
//! and this module must not grow a step that pretends it is.
//!
//! ⚠ **This was got backwards twice in one week, once by arch and once here**,
//! both times by treating the static emit as *"publishing"* and the tree write
//! as a precursor to it. The correction is recorded in arch's
//! `ROUTING-2026-09-15-d` §1 against themselves. The shape of the mistake is
//! worth naming because it is not obvious: a word with two meanings, where one
//! of them has a verb and a pipeline and a directory, and the other is just a
//! `put`. The one with machinery looks like the real one.
//!
//! ## What the verbs are, and what each one actually touches
//!
//! | verb | writes | unbinds |
//! |---|---|---|
//! | [`plan_add_entry`] | the entry, **and its `FEED-R2` detached signature** | — |
//! | [`plan_remove_entry`] | — | the entry **and its signature** |
//! | [`plan_add_to_collection`] | the collection | — |
//! | [`plan_remove_from_collection`] | the collection | — |
//!
//! ⛔ **The signature is the half that gets forgotten, in both directions, and
//! it is not under `app/feed/`.** `FEED-R2`'s detached signature lives at V7
//! §3.5's invariant pointer — `system/signature/{hex(entry_hash)}` — which is a
//! *kernel-owned* path this convention borrows. So an add that writes only the
//! entry publishes something no reader can attribute
//! ([`crate::feed_read::attribute`] answers `NoSignature`), and a remove that
//! unbinds only the entry leaves a signature bound to bytes that are gone —
//! which §7.3 forbids in as many words: *a tree from which an entry was removed
//! is byte-identical to a tree that never contained it. Not similar —
//! identical.* Both halves are gated.
//!
//! ## Everything here is PURE, and the plan is the return value
//!
//! No verb touches [`Peers`](crate::peers::Peers), the store, or a clock. Each
//! returns a [`ComposePlan`] the caller applies. Three reasons, and the third is
//! the one that decided it:
//!
//! 1. the browser's write path is arm-split and asynchronous, and none of the
//!    *decisions* here are;
//! 2. `make test` then gates the whole of it natively, on both arms, instead of
//!    through Selenium — the rule `session_config::decide_home` and
//!    `ladder_step` are already built on;
//! 3. **a verb that both decides and writes cannot be asked what it would do**,
//!    and *"what does removing this actually unbind"* is precisely the question
//!    §7.5's `FEED-R21` makes a UI obligation. A plan you can render is how the
//!    honest sentence gets written next to the button rather than in a help
//!    page.
//!
//! ## The clock is an argument
//!
//! `created_at` and `updated_at` are the author's own. They are parameters for
//! `feed_ingest`'s reason — a value read from the environment makes the same
//! authored input produce different entities on different machines — and
//! because `now_ms()` answers `0.0` natively, so a verb that read its own clock
//! would be reachable only through a browser.
//!
//! ## What this module is NOT
//!
//! - **not an index maintainer.** §4's index is a *projection* of the entry set
//!   ([`crate::feed_publish::plan_index`]), derived at publish time from the
//!   entries plus a page size. Writing it here as well would be two sources for
//!   one fact, and the one that can go stale is the derived one — C15, and
//!   [`crate::feed_tree`]'s module doc already rules it. A live reader reaches
//!   entries through §4.3 rule 6's prefix enumeration, which
//!   [`crate::feed_peer`] implements.
//! - **not a remover of anybody else's copy.** See [`RemovalMeaning`].

use entity_entity::Entity;
use entity_hash::Hash;

use crate::feed::{
    collection_key, entry_key, signature_key, CollectionId, FeedCollection, FeedEntry,
};

// ---------------------------------------------------------------------------
// The plan
// ---------------------------------------------------------------------------

/// What a verb would do to the tree: bindings to write, bindings to unbind.
///
/// Keys are **peer-relative** — `app/feed/entries/{hex}`, not
/// `/{peer}/app/feed/entries/{hex}` — which is the form every other key builder
/// in [`crate::feed`] produces and the form a foreign-tree read takes. The
/// caller qualifies them with the peer it is writing as, and there is exactly
/// one such caller so the qualification cannot drift.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ComposePlan {
    /// `(peer-relative key, entity)`, in the order they should be applied.
    pub writes: Vec<(String, Entity)>,
    /// Peer-relative keys to unbind.
    pub removals: Vec<String>,
}

impl ComposePlan {
    /// Every key this plan touches, written or unbound. For a caller that needs
    /// to subscribe, invalidate or simply show its work.
    pub fn keys(&self) -> Vec<&str> {
        self.writes
            .iter()
            .map(|(k, _)| k.as_str())
            .chain(self.removals.iter().map(String::as_str))
            .collect()
    }
}

/// Why a verb refused. **Each names a different mistake**, because the four are
/// made by different people: the first by the code calling this, the second by
/// whoever typed an id, the third and fourth by a UI that lost track of what it
/// was showing.
#[derive(Debug, Clone, PartialEq)]
pub enum ComposeError {
    /// **`FEED-R1` at the authoring end.** The entry claims an author that is
    /// not the peer whose tree it is being written into — which is the exact
    /// condition [`FeedEntry::from_entity`] refuses on the way back out, so
    /// writing it would mean minting something our own reader rejects.
    AuthorIsNotThisPeer { author: String, peer: String },
    /// The entry or collection would not encode. Ours, not the author's.
    Encoding(String),
    /// A member index that is not in `members`. Carried with the length so the
    /// caller can tell *"the list moved under me"* from *"off by one"*.
    NoSuchMember { at: usize, len: usize },
}

impl std::fmt::Display for ComposeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComposeError::AuthorIsNotThisPeer { author, peer } => write!(
                f,
                "this entry claims author {author} and would be written into {peer}'s tree"
            ),
            ComposeError::Encoding(e) => write!(f, "could not encode: {e}"),
            ComposeError::NoSuchMember { at, len } => {
                write!(f, "no member at position {at}; the collection holds {len}")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Verb 1 — add an entry
// ---------------------------------------------------------------------------

/// An entry, minted: its hash, and the plan that binds it and its signature.
#[derive(Debug, Clone, PartialEq)]
pub struct MintedEntry {
    /// The entry's content hash — its identity, and what every reference to it
    /// pins (§2.2.1).
    pub hash: Hash,
    pub plan: ComposePlan,
}

/// **Verb 1.** Mint an `app/feed/entry`, sign it, and bind both.
///
/// `signer` is the authoring peer's keypair. It is taken by reference and used
/// twice — once for the signature over the entry hash, once for the identity
/// hash that names *whose* signature it is — and both are exactly what
/// [`crate::feed_read::attribute`] recomputes at the far end. That is the
/// closing of the loop: this function's output is verified by the function that
/// reads it, in the same crate, and the round trip is gated rather than argued.
///
/// ## Why the author check is here and not at the call site
///
/// `FEED-R1` makes an entry whose `author` differs from the namespace it is read
/// under **invalid**, and our own decoder enforces it. A composer that let the
/// two diverge would write entities that are refused by the next read — a defect
/// that looks like data corruption and is actually an authoring bug. Refusing at
/// the mint is the only point where the person who can fix it is standing.
pub fn plan_add_entry(
    signer: &entity_crypto::IdentityKeypair,
    entry: &FeedEntry,
) -> Result<MintedEntry, ComposeError> {
    let peer = signer.peer_id().to_string();
    if entry.author != peer {
        return Err(ComposeError::AuthorIsNotThisPeer {
            author: entry.author.clone(),
            peer,
        });
    }

    let entity = entry.to_entity().map_err(ComposeError::Encoding)?;
    let hash = entity.content_hash;

    let signature = entity_types::SignatureData {
        target: hash,
        signer: signer.peer_identity_hash(),
        algorithm: signer.key_type().label().to_string(),
        signature: signer.sign(&hash.to_bytes()),
    }
    .to_entity()
    .map_err(|e| ComposeError::Encoding(format!("{e:?}")))?;

    Ok(MintedEntry {
        hash,
        plan: ComposePlan {
            // The entry first: a signature bound to bytes that are not there
            // yet is the transient form of the dangling record §7.3 forbids,
            // and on a store that applies these in order it is avoidable for
            // free.
            writes: vec![
                (entry_key(&hash), entity),
                (signature_key(&peer, &hash), signature),
            ],
            removals: Vec::new(),
        },
    })
}

// ---------------------------------------------------------------------------
// Verb 2 — remove an entry
// ---------------------------------------------------------------------------

/// **What removal means, and the sentence a UI owes the person pressing it.**
///
/// `FEED-R21` is a **MUST NOT**: *a conformant application MUST NOT present
/// removal as deletion.* §7.5 goes further and says where the sentence belongs —
/// *"it belongs at the moment of the action rather than in a help page"* — and
/// gives the wording: *"Removed from your site — people who already have it
/// still have it."*
///
/// This type exists so that obligation has somewhere to live that is not a
/// renderer's memory. [`plan_remove_entry`] returns it, so a caller that wants
/// to put a button on screen has already been handed the thing it must say, and
/// a caller that ignores it is visibly ignoring something.
///
/// ⚠ **It is not an error and not a warning.** It is what the verb *is*. There
/// is no global takedown, no protocol operation reaches into another peer's
/// store, and none will; anyone holding an older signed root can still show what
/// you served then (§7.4). Presenting that as deletion would be a promise the
/// architecture cannot keep — *worse than the honest statement, because a person
/// would believe it.*
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemovalMeaning;

impl RemovalMeaning {
    /// The i18n key for §7.5's sentence. A key rather than a string, because
    /// this is user-facing copy and the catalog is where translators can see it
    /// — the same reason `doctor.rs`'s outcomes are keys.
    pub const COPY_KEY: &'static str = "feed.compose.removal_is_unpublication";
}

/// **Verb 2.** Unbind an entry **and its detached signature**.
///
/// Returns the plan and [`RemovalMeaning`], which the caller owes the user.
///
/// ## The signature is the half that gets forgotten
///
/// §7.3's property — *a tree from which an entry was removed is byte-identical
/// to a tree that never contained it* — is a property of the trie, and it holds
/// only over the bindings you actually removed. `system/signature/{hex}` is
/// outside `app/feed/`, so an implementation that thinks of removal as *"drop
/// the entry"* leaves a `system/signature` entity bound to a target that no
/// longer resolves. Not a leak of the entry's content — the signature carries
/// only a hash — but a durable record of something the tree claims not to have,
/// which is precisely the trace §7.3 says is absent.
///
/// It takes the author because [`signature_key`] does: the invariant pointer is
/// built from the signer's peer id, and a removal computed against the wrong one
/// silently unbinds nothing.
pub fn plan_remove_entry(author: &str, entry_hash: &Hash) -> (ComposePlan, RemovalMeaning) {
    (
        ComposePlan {
            writes: Vec::new(),
            removals: vec![
                entry_key(entry_hash),
                signature_key(author, entry_hash),
            ],
        },
        RemovalMeaning,
    )
}

// ---------------------------------------------------------------------------
// Verbs 3 and 4 — collection membership
// ---------------------------------------------------------------------------

/// What an edit did to a collection, beyond the bytes.
///
/// One field, and it exists because of a consequence that is easy to ship
/// without noticing: see [`plan_remove_from_collection`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionEdit {
    /// **The `cover` is no longer among `members`.**
    ///
    /// Not an error — a cover outside `members` is accepted, because §5's *"one
    /// member, for presentation"* is a CDDL comment with no requirement id
    /// behind it and refusing would be inventing a MUST on somebody else's
    /// convention. But it is a state the author did not ask for and would want
    /// to know about, so it is reported rather than silently fixed. **Clearing
    /// the cover here would be worse**: it would discard an authored choice as a
    /// side effect of an unrelated edit.
    pub cover_is_no_longer_a_member: bool,
}

/// **Verb 3.** Append a member and rebind the collection.
///
/// ## Appended, never inserted-and-sorted
///
/// `FEED-R15` makes the order content. There is no correct place to put a new
/// member except where the author put it, and the only position this verb can
/// know is *last* — so that is what it does, and a caller that wants a different
/// position reorders explicitly. A verb that sorted, deduped or inserted "in the
/// right place" would be authoring on the author's behalf.
///
/// ## ⭐ Duplicates are allowed, and that is not laxness
///
/// §5 says *membership is not exclusive* — a photo is in three albums by being
/// referenced three times. The same is true **within** one collection: a
/// playlist that plays a track twice is an ordinary playlist, and the type's own
/// framing (*album, playlist, portfolio, pinned set, reading list*) includes the
/// case. So this does not dedupe.
///
/// ⚠ **The consequence lands on verb 4**, and it is the reason that one takes a
/// position rather than a reference: if a member can appear twice, *"remove this
/// member"* does not name one binding. A remove-by-value that takes the first
/// match will silently take the wrong one, and the author sees the right number
/// of items with the wrong one gone.
pub fn plan_add_to_collection(
    id: &CollectionId,
    collection: &FeedCollection,
    member: crate::entity_ref::EntityRef,
    now: u64,
) -> Result<(ComposePlan, FeedCollection), ComposeError> {
    let mut next = collection.clone();
    next.members.push(member);
    next.updated_at = now;
    let entity = next.to_entity().map_err(ComposeError::Encoding)?;
    Ok((
        ComposePlan {
            writes: vec![(collection_key(id), entity)],
            removals: Vec::new(),
        },
        next,
    ))
}

/// **Verb 4.** Remove the member **at `at`** and rebind the collection.
///
/// ## Why a position and not a reference
///
/// [`plan_add_to_collection`] permits duplicates, on §5's own terms, so a
/// reference does not identify a single membership. Taking a position makes the
/// ambiguity impossible to express rather than resolving it silently — the
/// argument-list guard [`crate::feed::MirrorSubject`] uses for `FEED-R26`, one
/// type over.
///
/// It is also the only form a list UI can supply honestly: what the person
/// clicked is a row, and a row *is* a position.
///
/// Refuses an out-of-range position rather than clamping, and carries the length
/// so the caller can tell *"the list moved under me"* from *"off by one"*.
pub fn plan_remove_from_collection(
    id: &CollectionId,
    collection: &FeedCollection,
    at: usize,
    now: u64,
) -> Result<(ComposePlan, FeedCollection, CollectionEdit), ComposeError> {
    if at >= collection.members.len() {
        return Err(ComposeError::NoSuchMember {
            at,
            len: collection.members.len(),
        });
    }
    let mut next = collection.clone();
    next.members.remove(at);
    next.updated_at = now;

    let edit = CollectionEdit {
        cover_is_no_longer_a_member: match &next.cover {
            None => false,
            Some(cover) => {
                // It only counts as a change if the cover WAS a member before.
                // A collection whose cover was already outside `members` has not
                // been changed by this edit, and reporting it would send the
                // author to look at something they chose.
                collection.members.contains(cover) && !next.members.contains(cover)
            }
        },
    };

    let entity = next.to_entity().map_err(ComposeError::Encoding)?;
    Ok((
        ComposePlan {
            writes: vec![(collection_key(id), entity)],
            removals: Vec::new(),
        },
        next,
        edit,
    ))
}

// ---------------------------------------------------------------------------
// Applying a plan
// ---------------------------------------------------------------------------

/// Apply a [`ComposePlan`] to `peer`'s tree through the arm-aware writer.
///
/// The **only** place a composer key is qualified with a peer, which is what
/// keeps `/{peer}/` out of the four verbs and out of [`crate::feed`]'s key
/// builders. One call site, so the qualification cannot drift — the rule
/// [`crate::feed::signature_key`] is already written to.
///
/// **Writes land before removals**, which costs nothing here (no verb produces
/// both) and is the safe order if one ever does: a plan that removed first would
/// have a window where neither the old nor the new binding exists.
///
/// Fire-and-forget, like every other app-tier write —
/// [`WriterHandle::put`](crate::writer_handle::WriterHandle::put) logs its own
/// failures. ⚠ **So this returns nothing and a caller must not read success into
/// it**: on the Direct/IDB arm a `put` is write-behind on a 250 ms debounce, so
/// *the call returned* is not *the bytes are durable*. A surface that needs to
/// know waits on the store; see AGENTS.md's `durable_state_hash` entry.
pub fn apply(writer: &crate::writer_handle::WriterHandle, peer: &str, plan: &ComposePlan) {
    for (key, entity) in &plan.writes {
        writer.put(format!("/{peer}/{key}"), entity.clone());
    }
    for key in &plan.removals {
        writer.remove(format!("/{peer}/{key}"));
    }
}

/// One of your own posts, as the authoring surface needs it: **the address it
/// is bound at**, and the entry.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnPost {
    /// The binding's address — taken from the **key**, never recomputed.
    pub hash: Hash,
    pub entry: FeedEntry,
}

/// Your own posts, for the authoring surface.
///
/// ## ⛔ Why this is not [`crate::feed_tree::read_owned_feed`]
///
/// That function is the **publish projection's** reader: it resolves every
/// pointer body's blob closure, sorts into publish order, and — the part that
/// decides it — returns `Vec<FeedEntry>`, **dropping the hash**. An authoring
/// surface needs the hash, because §7.3's unbinding is by address.
///
/// ## ⭐⭐ And the hash cannot be recovered by re-encoding
///
/// The obvious repair is `entry.to_entity().content_hash`, and it is wrong in a
/// way that is silent and total. V7 §2.6 makes unknown fields **MUST-ignore**,
/// so an entry carrying a term this build does not model — one authored by a
/// later version of us, by another implementation, or by a convention revision —
/// decodes fine, drops the term, and **re-encodes to a different hash**. Remove
/// would then build a key that names no binding, unbind nothing, and report the
/// §7.5 sentence. The post stays up and the person is told it came down.
///
/// So the address is read from the **key**, which is where it actually lives:
/// [`entry_key`] binds an entry at its own content hash precisely so that *"a
/// key that is the identity cannot drift from it"*. Decoding is for display;
/// addressing is from the key.
///
/// Ordering is the caller's — this returns tree order, which is by hash and
/// therefore arbitrary.
pub fn own_posts(peers: &crate::peers::Peers, peer: &str) -> Vec<OwnPost> {
    let prefix = format!("/{peer}/{}", crate::feed::entry_prefix());
    let mut out = Vec::new();
    for listed in peers.tree_listing(peer, &prefix) {
        let Some(hex) = listed.path.strip_prefix(&prefix).filter(|r| !r.is_empty()) else {
            continue;
        };
        let Some(hash) = crate::entity_ref::hash_from_hex(hex) else {
            // A key under our entry prefix that is not a hash was not written by
            // `entry_key`. Skipped rather than guessed at: offering a Remove
            // button for it would build an address from something that is not
            // one.
            tracing::warn!(key = %listed.path, "feed compose: entry key is not a content hash");
            continue;
        };
        let Some(entity) = peers.get_entity(peer, &listed.path) else { continue };
        match FeedEntry::from_entity(&entity, peer) {
            Ok(entry) => out.push(OwnPost { hash, entry }),
            Err(why) => tracing::warn!(
                key = %listed.path,
                ?why,
                "feed compose: one of our own entries did not decode"
            ),
        }
    }
    out
}

/// The authoring keypair for one of **this profile's own** peers.
///
/// `None` when the id names a peer this profile does not hold the key for —
/// which is every foreign peer, and is the honest answer: you cannot author into
/// somebody else's namespace, and [`plan_add_entry`] would refuse the entry
/// anyway under `FEED-R1`.
///
/// Derives the id from the key rather than trusting the stored `peer_id` field,
/// which is `roster::spawn_list_derived`'s rule and exists because the two can
/// drift.
pub fn authoring_keypair(peer: &str) -> Option<entity_crypto::IdentityKeypair> {
    crate::persistence::load_all_peer_entries()
        .into_iter()
        .map(|e| entity_crypto::IdentityKeypair::Ed25519(e.persisted.keypair))
        .find(|kp| kp.peer_id().to_string() == peer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::{EmbedData, EmbedNode, EmbedPayload};
    use crate::entity_ref::EntityRef;

    const NOW: u64 = 1_757_000_000_000;

    fn keypair() -> entity_crypto::IdentityKeypair {
        entity_crypto::IdentityKeypair::Ed25519(entity_crypto::Keypair::generate())
    }

    fn body(s: &str) -> EmbedNode {
        EmbedNode::new(
            "text/plain",
            EmbedData::new(EmbedPayload::Inline(s.as_bytes().to_vec()), s),
        )
    }

    fn h(seed: &str) -> Hash {
        Hash::compute("test/note", seed.as_bytes())
    }

    // -- verb 1 ------------------------------------------------------------

    /// ⭐ **THE LOOP, AND IT IS THE GATE THIS MODULE EXISTS FOR.** What the
    /// composer mints is verified by the reader in this same crate — not by an
    /// assertion about the fields, which would only prove the composer agrees
    /// with itself.
    ///
    /// `attribute` is the function a foreign reader runs on somebody else's
    /// entry. Feeding it ours closes `FEED-R2` end to end: the signature targets
    /// the entry's own hash, the signer is the author's identity hash derived
    /// from their peer id with no second fetch, and the bytes verify against the
    /// key the peer id embeds.
    #[test]
    fn an_entry_this_composer_mints_is_attributed_by_our_own_reader() {
        let kp = keypair();
        let author = kp.peer_id().to_string();
        let entry = FeedEntry::new(author.clone(), NOW, body("hello"));

        let minted = plan_add_entry(&kp, &entry).unwrap();
        let signature = &minted.plan.writes[1].1;

        let verdict = crate::feed_read::attribute(&author, &minted.hash, Some(signature));
        assert_eq!(
            verdict,
            crate::feed_read::Attribution::Signed,
            "the composer's own signature must satisfy the reader"
        );
    }

    /// The two bindings, at the two keys, in that order — and the second one is
    /// outside `app/feed/`.
    #[test]
    fn adding_an_entry_binds_the_entry_and_its_signature() {
        let kp = keypair();
        let author = kp.peer_id().to_string();
        let minted =
            plan_add_entry(&kp, &FeedEntry::new(author.clone(), NOW, body("x"))).unwrap();

        assert_eq!(minted.plan.writes.len(), 2);
        assert!(minted.plan.removals.is_empty());
        assert_eq!(minted.plan.writes[0].0, entry_key(&minted.hash));
        assert_eq!(minted.plan.writes[1].0, signature_key(&author, &minted.hash));
        assert!(
            minted.plan.writes[1].0.starts_with("system/signature/"),
            "FEED-R2's pointer is kernel-owned and NOT under app/feed/ — which is \
             the whole reason it gets forgotten"
        );
        // The entry lands before the thing that points at it.
        assert!(minted.plan.writes[0].0.starts_with("app/feed/entries/"));
    }

    /// `FEED-R1` at the authoring end: minting an entry that claims somebody
    /// else's authorship is refused here, because our own decoder refuses it on
    /// the way back out.
    #[test]
    fn an_entry_claiming_another_author_is_refused_at_the_mint() {
        let kp = keypair();
        let entry = FeedEntry::new("QmSomebodyElse", NOW, body("not mine"));
        assert!(matches!(
            plan_add_entry(&kp, &entry),
            Err(ComposeError::AuthorIsNotThisPeer { .. })
        ));
    }

    // -- verb 2 ------------------------------------------------------------

    /// ⛔ **§7.3 — the removal takes the signature with it.** Falsify by
    /// dropping the second removal and this is what goes red: the tree would
    /// keep a `system/signature` entity naming a target it no longer holds,
    /// which is exactly the trace §7.3 says does not exist.
    #[test]
    fn removing_an_entry_unbinds_its_signature_too() {
        let author = keypair().peer_id().to_string();
        let hash = h("an entry");
        let (plan, _meaning) = plan_remove_entry(&author, &hash);

        assert!(plan.writes.is_empty(), "a removal writes nothing — no tombstone (§7.3)");
        assert_eq!(
            plan.removals,
            vec![entry_key(&hash), signature_key(&author, &hash)],
            "both halves, and the signature is the one outside app/feed/"
        );
    }

    /// `FEED-R21`'s sentence has a home. A caller cannot take the plan without
    /// also being handed what it owes the person pressing the button.
    #[test]
    fn a_removal_hands_the_caller_the_sentence_it_owes() {
        let (_plan, meaning) = plan_remove_entry("QmAuthor", &h("e"));
        assert_eq!(meaning, RemovalMeaning);
        // `t` returns the KEY when the catalog has no entry, so this is a real
        // presence check and not a tautology.
        let rendered = crate::i18n::t(RemovalMeaning::COPY_KEY, &[]);
        assert_ne!(
            rendered,
            RemovalMeaning::COPY_KEY,
            "FEED-R21's wording is a catalog string, not a literal in a renderer"
        );
        // §7.5's obligation is that it does not read as deletion.
        let lowered = rendered.to_lowercase();
        assert!(
            !lowered.contains("delet"),
            "FEED-R21 is a MUST NOT: {rendered:?} presents removal as deletion"
        );
    }

    // -- applying a plan ---------------------------------------------------

    /// ⭐⭐ **THE WHOLE ACT, END TO END, AND IT IS THE CLAIM THIS MODULE MAKES:
    /// a tree write IS the publish.** Compose an entry, apply the plan to a real
    /// peer's tree, read both bindings back out at the keys a *foreign* reader
    /// would ask for, and attribute the result.
    ///
    /// Nothing here mints a root, projects a directory or touches a network, and
    /// at the end the entry is present, addressed by its own hash, and
    /// attributable to its author. That is the live road complete.
    ///
    /// **The keys are read back QUALIFIED**, rebuilt from the same peer-relative
    /// builders `feed_read` uses — so this also gates [`apply`]'s qualification,
    /// which is the one place `/{peer}/` is prepended.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn composing_an_entry_puts_it_in_the_tree_where_a_reader_looks_for_it() {
        // ⚠ The profile authors as ITSELF, and the test is built that way on
        // purpose. An earlier cut signed with a freshly generated key and wrote
        // to `/{that key's peer}/…`, which failed at the read: `get_entity`
        // selects the owning SDK by peer id, and a profile has no SDK for a peer
        // it does not hold. That is not a rig detail — it is the same fact
        // `plan_add_entry` refuses on under `FEED-R1`, arriving one layer down.
        // One key, two handles, from a fixed seed — the profile is constructed
        // with it and the composer signs with it.
        //
        // ⚠ **Not `direct_peer_shared`**, which would have been the obvious way
        // to reach the peer's own identity: `tests/escape_hatch_budget.rs`
        // refuses a new caller of it, correctly — it is an L0 hatch that answers
        // `None` on the Worker arm, and a composer must not be built on
        // something that is silently wrong on one arm. The product's cross-arm
        // answer is [`authoring_keypair`], which reads persistence; a seed is
        // how a test gets the same key without a persisted profile.
        const SEED: [u8; 32] = [7u8; 32];
        let signing =
            entity_crypto::IdentityKeypair::Ed25519(entity_crypto::Keypair::from_seed(SEED));
        let peers =
            crate::peers::Peers::new_direct_with_keypair(entity_crypto::Keypair::from_seed(SEED));
        let author = peers.primary_peer_id().to_string();
        assert_eq!(author, signing.peer_id().to_string(), "authoring as ourselves");

        let entry = FeedEntry::new(author.clone(), NOW, body("the whole act"));
        let minted = plan_add_entry(&signing, &entry).unwrap();
        let writer = peers.writer_handle().expect("a Direct profile has a writer");
        apply(&writer, &author, &minted.plan);

        let entry_path = format!("/{author}/{}", entry_key(&minted.hash));
        let sig_path = format!("/{author}/{}", signature_key(&author, &minted.hash));

        let stored_entry = peers
            .get_entity(&author, &entry_path)
            .expect("the entry is in the tree at the key a reader asks for");
        let stored_sig = peers
            .get_entity(&author, &sig_path)
            .expect("and so is its FEED-R2 signature, outside app/feed/");

        // It decodes as what it claims to be, under the namespace it was written
        // into — FEED-R1's own check, run against our own write.
        let decoded = FeedEntry::from_entity(&stored_entry, &author).unwrap();
        assert_eq!(decoded, entry);
        assert_eq!(
            stored_entry.content_hash, minted.hash,
            "the key IS the identity: what came back hashes to the address it was filed at"
        );
        assert_eq!(
            crate::feed_read::attribute(&author, &minted.hash, Some(&stored_sig)),
            crate::feed_read::Attribution::Signed,
            "and a reader can name its author, from the tree alone"
        );
    }

    /// `authoring_keypair` answers only for peers this profile holds a key for.
    /// A foreign id is `None` — the honest answer, since you cannot author into
    /// somebody else's namespace.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn there_is_no_authoring_key_for_a_peer_this_profile_does_not_hold() {
        assert!(authoring_keypair("QmSomebodyElseEntirely").is_none());
    }

    // -- verbs 3 and 4 -----------------------------------------------------

    fn collection_with(members: &[&str]) -> FeedCollection {
        let mut c = FeedCollection::new("Album", NOW);
        c.members = members.iter().map(|m| EntityRef::pin("QmAuthor", h(m))).collect();
        c
    }

    /// Appended at the end, and `updated_at` moves. `FEED-R15` makes the order
    /// content, and the end is the only position this verb can know.
    #[test]
    fn adding_to_a_collection_appends_and_stamps() {
        let id = CollectionId::parse("album").unwrap();
        let before = collection_with(&["a", "b"]);
        let added = EntityRef::pin("QmAuthor", h("c"));

        let (plan, next) =
            plan_add_to_collection(&id, &before, added.clone(), NOW + 5).unwrap();

        assert_eq!(next.members.len(), 3);
        assert_eq!(next.members[2], added, "appended, not inserted");
        assert_eq!(&next.members[..2], &before.members[..], "and nothing moved");
        assert_eq!(next.updated_at, NOW + 5);
        assert_eq!(next.created_at, before.created_at, "creation does not move");
        assert_eq!(plan.writes.len(), 1);
        assert_eq!(plan.writes[0].0, collection_key(&id));
    }

    /// ⭐ **Duplicates are allowed — §5's *membership is not exclusive*, applied
    /// within one collection.** A playlist that plays a track twice is an
    /// ordinary playlist. This is the precondition for the next test, and
    /// without it that one measures nothing.
    #[test]
    fn a_collection_may_hold_the_same_member_twice() {
        let id = CollectionId::parse("album").unwrap();
        let again = EntityRef::pin("QmAuthor", h("a"));
        let (_, next) =
            plan_add_to_collection(&id, &collection_with(&["a", "b"]), again.clone(), NOW)
                .unwrap();
        assert_eq!(next.members, vec![
            EntityRef::pin("QmAuthor", h("a")),
            EntityRef::pin("QmAuthor", h("b")),
            again,
        ]);
    }

    /// ⭐⭐ **THE REASON VERB 4 TAKES A POSITION.** With a duplicate present, a
    /// remove-by-value takes the first match — so the author sees the right
    /// number of items and the wrong one gone, which is the least detectable
    /// kind of wrong.
    ///
    /// Removing position 2 must leave `[a, b]`; a by-value implementation would
    /// leave `[b, a]`. The two differ only in ORDER, and order is content
    /// (`FEED-R15`) — so this is a data-loss bug wearing a no-op's clothes, and
    /// a test asserting only the length would pass through it.
    #[test]
    fn removing_a_duplicated_member_takes_the_one_that_was_clicked() {
        let id = CollectionId::parse("album").unwrap();
        let mut c = collection_with(&["a", "b"]);
        c.members.push(EntityRef::pin("QmAuthor", h("a"))); // [a, b, a]

        let (_, next, _) = plan_remove_from_collection(&id, &c, 2, NOW + 1).unwrap();

        assert_eq!(
            next.members,
            vec![
                EntityRef::pin("QmAuthor", h("a")),
                EntityRef::pin("QmAuthor", h("b")),
            ],
            "position 2 went; a by-value remove would have taken position 0 and \
             left [b, a] — same length, different content"
        );
    }

    /// Out of range refuses and says how long the list was, so a caller can tell
    /// *"it moved under me"* from *"off by one"*. Clamping would remove
    /// something nobody asked to remove.
    #[test]
    fn a_position_that_is_not_there_is_refused_with_the_length() {
        let id = CollectionId::parse("album").unwrap();
        let c = collection_with(&["a", "b"]);
        assert_eq!(
            plan_remove_from_collection(&id, &c, 2, NOW),
            Err(ComposeError::NoSuchMember { at: 2, len: 2 })
        );
        assert_eq!(
            plan_remove_from_collection(&id, &FeedCollection::new("Empty", NOW), 0, NOW),
            Err(ComposeError::NoSuchMember { at: 0, len: 0 })
        );
    }

    /// Removing the cover's only membership REPORTS, and does not clear the
    /// cover — clearing it would discard an authored choice as a side effect of
    /// an unrelated edit.
    #[test]
    fn removing_the_cover_reports_rather_than_silently_repairing() {
        let id = CollectionId::parse("album").unwrap();
        let mut c = collection_with(&["a", "b"]);
        c.cover = Some(EntityRef::pin("QmAuthor", h("a")));

        let (_, next, edit) = plan_remove_from_collection(&id, &c, 0, NOW).unwrap();
        assert!(edit.cover_is_no_longer_a_member);
        assert_eq!(next.cover, c.cover, "the authored choice survives the report");
    }

    /// The other side of the same coin: a cover that was ALREADY outside
    /// `members` has not been changed by this edit, and saying so would send the
    /// author to look at something they chose deliberately.
    #[test]
    fn a_cover_that_was_never_a_member_is_not_reported_as_a_change() {
        let id = CollectionId::parse("album").unwrap();
        let mut c = collection_with(&["a", "b"]);
        c.cover = Some(EntityRef::pin("QmAuthor", h("elsewhere")));

        let (_, _, edit) = plan_remove_from_collection(&id, &c, 0, NOW).unwrap();
        assert!(!edit.cover_is_no_longer_a_member);
    }

    /// And a duplicated cover is not lost when one of its memberships goes —
    /// the check is *is it still there*, not *did I just remove one like it*.
    #[test]
    fn a_cover_with_two_memberships_survives_one_of_them_being_removed() {
        let id = CollectionId::parse("album").unwrap();
        let mut c = collection_with(&["a", "b"]);
        c.members.push(EntityRef::pin("QmAuthor", h("a")));
        c.cover = Some(EntityRef::pin("QmAuthor", h("a")));

        let (_, _, edit) = plan_remove_from_collection(&id, &c, 0, NOW).unwrap();
        assert!(
            !edit.cover_is_no_longer_a_member,
            "one of two memberships went; the cover is still a member"
        );
    }
}
