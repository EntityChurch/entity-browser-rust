//! `feed_peer` — a [`FeedSource`] over a **live connection** to the author.
//!
//! The same read [`OriginFeedSource`](crate::feed_fetch::OriginFeedSource)
//! performs against a published origin, performed instead against the peer
//! whose feed it is, over whatever transport reaches them.
//!
//! ## "It is just the transport" is three-quarters true, and the rest is the point
//!
//! Measured 2026-09-11 **before** this module was written, because the claim is
//! the kind that is cheap to believe and expensive to be wrong about.
//!
//! **1. The key space is identical — and that is not a coincidence, it is what
//! publishing *is*.** [`index_head_path`](crate::feed::index_head_path) and
//! [`index_head_key`](crate::feed::index_head_key) are one expression
//! (`path = "/{peer}" + "/" + key`), and the same holds for pages, entries and
//! signatures. A publish is a **projection of the tree**
//! (`REFERENCE-PUBLISHING-PIPELINE` §0.1), so the key a signed-root consumer
//! resolves and the path a live reader dispatches at are the same string with a
//! peer segment on the front. Nothing had to be invented for this module to
//! address anything.
//!
//! **2. The reader was already abstracted at the right level.** [`FeedSource`]
//! is `key → Option<Entity>`, async, boxed — so a live source is an `impl`, not
//! a redesign, and [`read_feed`](crate::feed_read::read_feed) with its whole
//! `FEED-R4` attribution walk runs unchanged. It was *nearly* not: the trait
//! shipped **synchronous**, shaped by a test double that always had the answer
//! in hand, and no real source ever does. That was fixed the day before this
//! module needed it — `a_source_that_answers_later_is_still_a_source` is the
//! gate, and this is the implementation that proves it was the right shape.
//!
//! **3. The authority model is genuinely different, and both are sound.** A
//! published tree is **root-anchored**: the publisher's signature over a trie is
//! what makes a stranger's CDN safe to fetch from. A live read is
//! **connection-anchored**: the kernel authenticated this peer id at the
//! handshake, so the bytes came from the author by construction. Neither is
//! weaker; they answer *"why do I believe these are their bytes"* by different
//! routes. **A live source that also tried to root-anchor would be asking the
//! author to prove they are themselves** — and would fail, because a peer's live
//! tree carries no published root and need not. What is *unchanged* is the part
//! that does not depend on either: `FEED-R2`'s per-entry detached signature is
//! verified here exactly as over HTTP, by the same code, so attribution is a
//! property of the entry and not of how it arrived.
//!
//! **4. The real asymmetry is AUTHORIZATION, and it is not transport at all —
//! and TODAY IT IS SWITCHED OFF, which is not what this paragraph said until a
//! gate measured it.** An origin publish is public by construction. A live read
//! is, *by the kernel's default*, denied: `default_connection_grants`
//! (`core/capability`) grants `system/tree:get` at `system/type/*` and
//! `system/handler/*` and nothing else, so `/{author}/app/feed/index` would come
//! back **403** until the author granted it.
//!
//! ⚠ **That default is overridden one layer up and this app never sees it.**
//! `PeerManager::with_keypair_and_optional_connector`
//! (`entity-core-rust/bindings/sdk/src/peer_manager.rs`) builds every peer with
//! `debug_open_grants: true`, so `PeerConfig::default()`'s `false` is not the
//! posture any peer here runs. **Measured, not inferred**, and it corrected the
//! first draft of this paragraph: a probe dispatching at three paths — an
//! app-private one, a granted one and `system/peer/keys` — got **no 403 at
//! all**. Every path answers as though granted; the only thing that varies is
//! whether an entity is there. So **a peer you are connected to can read any
//! path in your tree**, today, and the live feed read needs no grant.
//!
//! `share.rs`'s module doc has said *"Enforcement is still off … do not read a
//! `Peer` audience as a control today"* since the SHARE work; what is new here
//! is the same fact stated as a **read** rather than as a write, which is the
//! direction a person asking *"can I read my friend's feed"* meets it from.
//!
//! **What this does NOT change is the reader**, and that is the useful part:
//! `ShareTarget::Prefix("/{author}/app/feed/")` already produces exactly the
//! grant this read needs (`system/tree` + `system/content`, `get` + `list`,
//! scoped to the subtree), so at cutover the mechanism is in place and the only
//! thing that changes is that an unshared feed starts being refused. **So
//! "publish my feed" and "share my feed with you" are the same act over two
//! transports** — one of them is simply not yet enforced.
//!
//! The consequence is one line of code and one sentence of copy: a 403 must
//! never render as *"this publisher has posted nothing"*. See
//! [`remote_read`](crate::remote_read)'s table. It is unreachable today and it
//! is the **common** case the day enforcement flips, which is precisely when
//! nobody will be looking at this module.
//!
//! ## ⚠ The finding that made this more than a transport swap
//!
//! **The feed index does not exist in the live tree. It is a publish artifact.**
//! Measured, not inferred: `index_head_path` and `index_page_path` — the
//! absolute tree paths — are referenced by **nothing in this crate but their own
//! unit test**. [`feed_ingest`](crate::feed_ingest) binds entries at
//! `app/feed/entries/{hex}` and writes no index;
//! [`plan_index`](crate::feed_publish::plan_index) builds the head and pages
//! **at publish time** and writes them into the out-dir only.
//!
//! So a live reader that only resolved keys would ask a peer with a perfectly
//! good feed for `app/feed/index`, get a 404, and report *"this author has no
//! feed"*. **The transport was never the missing piece.**
//!
//! This module takes §4.3 rule 6's own way out — *"a reader that cannot fetch
//! \[the index] falls back to **enumerating the prefix** — slower, same
//! answer"* — which is implementable here and was not on the HTTP path as
//! `read_feed` was written. [`entry_prefix`](crate::feed::entry_prefix)'s doc
//! says outright that it exists *"so §4.3 rule 6's fallback has something to
//! enumerate"*; this is the first consumer of that sentence.
//!
//! **Two bounds, stated rather than discovered later.** The prefix is **ours,
//! not the convention's** (§2: *"the cross-impl contract is the type tag, not
//! the path"*), so enumeration reads a publisher running this implementation and
//! is not a cross-impl mechanism — `A-38` is the routed question and this does
//! not answer it. And **the authored order is not recoverable from a prefix**:
//! §4.5 makes order authored and the index is what carries it, so the fallback
//! sorts by [`feed_tree::sort_key`](crate::feed_tree::sort_key), the same
//! `(created_at, content_hash)` total order the author's own publisher uses.
//! That is the best reconstruction available and it is **a reconstruction** —
//! which is why the walk reports which arm it took rather than presenting the
//! two as interchangeable.
//!
//! ## What is NOT here
//!
//! **No durable copy.** Nothing this reader fetches is written down, so D24 does
//! not engage — the same bound [`feed_fetch`](crate::feed_fetch) states. When it
//! does, the analysis is not one answer for the whole feed: the head and pages
//! are fixed-key and **mutable**, an entry is keyed by its own content hash and
//! cannot go stale, and a signature is keyed by the *target's* hash and **looks**
//! content-addressed while being mutable.
//!
//! **No pointer bodies over the wire.** A post whose body took EMBED §3's
//! pointer arm (over 16 KiB) names a blob in the author's content store. The
//! bytes are there — `feed_ingest` seeds them through `Peers::seed_content`, and
//! `system/content:get` resolves **by hash, not by namespace** (the namespace is
//! the capability scope; `handle_get` looks the hash up in a flat store) — so the
//! walk is [`file_offer::pull_offer`](crate::file_offer::pull_offer)'s, one
//! consumer over. It is not wired, so a pointer body renders as an entry whose
//! text did not resolve rather than as a failure of the read.

use std::future::Future;
use std::pin::Pin;

use entity_entity::Entity;

use crate::dispatch_handle::DispatchHandle;
use crate::feed_read::FeedSource;
use crate::remote_read::{list_keys_at, read_entity_at};

/// A [`FeedSource`] over a live connection to `author`.
///
/// **Owned, not borrowed**, for [`FeedSource`]'s own reason: a boxed future
/// cannot borrow `self`, so everything the read needs is cloned in. That
/// friction is the trait being honest — it is exactly what a `JsFuture`-backed
/// source must do too.
pub struct PeerFeedSource {
    dispatch: DispatchHandle,
    author: String,
}

impl PeerFeedSource {
    /// Read `author`'s feed over whatever connection reaches them.
    ///
    /// **No key check, deliberately, and this is the one place this source is
    /// laxer than the origin one — because the transport was stricter first.**
    /// `OriginFeedSource::new` refuses an author whose peer id does not embed
    /// its own key, since without it there is nothing to verify a *stranger's
    /// origin* against. Here the peer id is not a verification key we are about
    /// to use, it is the **dispatch target**: the kernel authenticated it at the
    /// handshake, and an id it could not authenticate has no connection for this
    /// source to read over at all. Refusing here would rule out legitimately
    /// connected peers on an Ed448 or legacy-SHA-256 id for a check the
    /// transport already performed.
    ///
    /// `FEED-R4` attribution is unaffected and still runs: it reports
    /// `Unattributed::KeyNotInPeerId` for exactly those ids — *we could not
    /// check* — which is kept apart from *this is not theirs*.
    pub fn new(dispatch: DispatchHandle, author: impl Into<String>) -> Self {
        Self { dispatch, author: author.into() }
    }

    /// `key` → the fully qualified tree path. **The whole translation between
    /// the two transports is this one line**, which is the measurement above
    /// made executable.
    fn path_for(&self, relative_key: &str) -> String {
        format!("/{}/{}", self.author, relative_key)
    }
}

impl FeedSource for PeerFeedSource {
    fn get(
        &self,
        relative_key: String,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Entity>, String>>>> {
        let dispatch = self.dispatch.clone();
        let author = self.author.clone();
        let path = self.path_for(&relative_key);
        Box::pin(async move {
            // **Stringified here and nowhere earlier.** `FeedSource`'s contract
            // is `Result<_, String>`, so the typed outcome ends at this
            // boundary — which is why every `RemoteReadError` variant's
            // `Display` is a distinct sentence rather than a shared "read
            // failed". A reader who is not granted the prefix reads *why*.
            read_entity_at(&dispatch, &author, &path).await.map_err(|e| e.to_string())
        })
    }

    fn list(
        &self,
        relative_prefix: String,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Vec<String>>, String>>>> {
        let dispatch = self.dispatch.clone();
        let author = self.author.clone();
        let path = self.path_for(&relative_prefix);
        Box::pin(async move {
            // `Some(...)`, always — **this source can always enumerate**, and an
            // empty listing means the peer has bound nothing under the prefix.
            // Returning `None` for an empty result would say *"I cannot
            // enumerate"* about a successful enumeration, which is the fallback
            // being switched off by an author who has simply posted nothing.
            list_keys_at(&dispatch, &author, &path)
                .await
                .map(Some)
                .map_err(|e| e.to_string())
        })
    }
}
// ---------------------------------------------------------------------------
// The live gates — two real peers, one real connection, no HTTP anywhere.
// ---------------------------------------------------------------------------
//
// These are the only gates in this crate that read a feed the way the operator
// asked about: *"if I connect to a peer, I should be able to do these same
// operations."* Everything else in the feed arc points a directory-backed
// `BinSource` at a temp dir, which is an origin publish with the web server
// left out — a population that cannot contain a permission, a handshake or a
// dispatch.
//
// The harness is `peers::memory_transport_tests::spawn_peer_on_registry`: two
// `Peers` over one in-process `MemoryTransportRegistry`, each running
// `entity_peer::server::run`, with `connect_peer` completing the real
// entity-protocol handshake. No networking, no ports, no browser — and the
// dispatch path under test is the same one a WebRTC data channel uses, because
// nothing below `DispatchHandle` knows which transport it got.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod live_tests {
    use super::*;
    use crate::feed::{
        entry_key, index_head_key, index_page_key, FeedEntry, IndexHead, IndexPage,
    };
    use crate::embed::{EmbedData, EmbedNode, EmbedPayload};
    use crate::entity_ref::EntityRef;
    use crate::feed_read::{read_feed, Attribution, FeedReadError};
    use crate::peers::memory_transport_tests::spawn_peer_on_registry;
    use crate::peers::Peers;
    use crate::share::{Audience, Share, ShareTarget};
    use entity_hash::Hash;
    use entity_peer::transport::MemoryTransportRegistry;
    use std::time::Duration;

    const NOW: u64 = 1_757_000_000_000;

    fn body(s: &str) -> EmbedNode {
        EmbedNode::new("text/plain", EmbedData::new(EmbedPayload::Inline(s.as_bytes().to_vec()), s))
    }

    /// Write one entry into `peer_id`'s tree at the key the publisher binds it
    /// at — `feed_tree`'s own `seed_entry`, awaited, because this one has to be
    /// durable before another peer dispatches at it.
    async fn seed_entry(peers: &Peers, peer_id: &str, entry: &FeedEntry) -> Hash {
        let e = entry.to_entity().unwrap();
        let hash = e.content_hash;
        peers
            .put_and_wait(peer_id, format!("/{peer_id}/{}", entry_key(&hash)), e, 2_000)
            .await
            .expect("the entry lands in the author's tree");
        hash
    }

    /// Three posts, oldest first, returning their hashes in authored order.
    async fn seed_three_posts(peers: &Peers, pid: &str) -> Vec<Hash> {
        let mut hashes = Vec::new();
        for i in 0..3u64 {
            let entry = FeedEntry::new(pid, NOW + i * 1000, body(&format!("post {i}")));
            hashes.push(seed_entry(peers, pid, &entry).await);
        }
        hashes
    }

    /// Write the §4.2 index the *publisher* would have projected — head plus one
    /// page, newest-first within the page.
    ///
    /// **This is the shape a live tree does NOT have today**, and writing it by
    /// hand here is what separates the two arms: this gate exercises key
    /// resolution, and [`a_peer_whose_tree_holds_only_entries_is_still_readable`]
    /// exercises the enumeration fallback against the tree a real author has.
    async fn seed_index(peers: &Peers, pid: &str, hashes: &[Hash]) {
        let mut refs: Vec<EntityRef> = hashes.iter().map(|h| EntityRef::pin(pid, *h)).collect();
        refs.reverse(); // §4.5 — newest first WITHIN the page.
        let page = IndexPage::new(0, refs, NOW + 2000).to_entity().unwrap();
        peers
            .put_and_wait(pid, format!("/{pid}/{}", index_page_key(0)), page, 2_000)
            .await
            .expect("page 0 lands");
        let head = IndexHead::new(0, NOW + 2000).to_entity().unwrap();
        peers
            .put_and_wait(pid, format!("/{pid}/{}", index_head_key()), head, 2_000)
            .await
            .expect("the head lands");
    }

    /// Author the policy entry that grants `reader` `owner`'s feed subtree —
    /// **through the product mechanism, not a hand-rolled `GrantEntry`.**
    ///
    /// A `ShareTarget::Prefix` share over `/{owner}/app/feed/` is exactly the
    /// authorization a live feed read will need. Hand-writing the grant here
    /// would prove the reader works against a grant nothing in the product
    /// produces.
    ///
    /// ⚠ **What this DOES and DOES NOT establish today.** It establishes that
    /// the policy write lands (`status < 300`). It does **not** authorize
    /// anything, and no gate in this module may claim it does: every peer is
    /// built with `debug_open_grants: true` (`bindings/sdk`'s `PeerManager`), so
    /// the read succeeds identically with this never called — measured, by
    /// deleting it. **A gate whose expected value does not depend on the thing
    /// it names is a gate satisfied by its fallback**, and that is why the
    /// positive gate below does not call this and says so, rather than calling
    /// it and reading as though authorization were covered.
    ///
    /// Kept, exercised once, because it is the half that becomes load-bearing at
    /// cutover — and because the ordering it documents is real:
    /// `assemble_inbound_grants` unions the matched policy entry at authenticate
    /// time, so a policy authored **after** the handshake would not reach an
    /// already-open connection.
    async fn share_feed_with(peers: &Peers, owner: &str, reader: &str) {
        let share = Share {
            title: "my feed".into(),
            target: ShareTarget::Prefix(format!("/{owner}/app/feed/")),
            // One named peer — an `app/share/record`, not a publication. The
            // reader is granted by name, so the policy key this authors is the
            // reader's peer id and nothing is written under `default`. That
            // keeps `SHARE-10` out of the gate: authoring a `default` entry
            // where a deployment had none converts *no request-time ceiling*
            // into *this request-time ceiling*, which is a side effect a test
            // has no business having.
            audience: Audience::peer(reader.to_string(), NOW),
            note: None,
            created_at: NOW,
            size: None,
            from: owner.to_string(),
        };
        for (key, grants) in crate::share::policy_entries(&[share], owner, &[]) {
            let params = crate::share::build_configure_params(&key, &grants)
                .expect("the configure params encode");
            let result = peers
                .execute(
                    owner,
                    "system/capability".to_string(),
                    "configure".to_string(),
                    params,
                    Default::default(),
                )
                .await
                .expect("configure dispatches");
            assert!(result.status < 300, "policy write refused: {}", result.status);
        }
    }

    /// Stand up author + reader, seed the author, optionally share, connect.
    async fn two_peers(
        index: bool,
    ) -> (Peers, String, Peers, String, Vec<Hash>, Vec<tokio::task::JoinHandle<()>>) {
        let registry = MemoryTransportRegistry::new();
        let (author, pid_author, h_a) = spawn_peer_on_registry(registry.clone());
        let (reader, pid_reader, h_b) = spawn_peer_on_registry(registry.clone());
        tokio::task::yield_now().await;

        let hashes = seed_three_posts(&author, &pid_author).await;
        if index {
            seed_index(&author, &pid_author, &hashes).await;
        }
        let connect = reader.connect_peer(&pid_reader, format!("memory://{pid_author}"));
        let remote = tokio::time::timeout(Duration::from_secs(2), connect)
            .await
            .expect("connect_peer timed out")
            .expect("connect_peer must succeed");
        assert_eq!(remote, pid_author);

        (author, pid_author, reader, pid_reader, hashes, vec![h_a, h_b])
    }

    /// ⭐ **THE GATE THE OPERATOR ASKED FOR.** A peer I am connected to serves
    /// me their feed, over the live transport, with no origin and no web server
    /// in the picture — and `read_feed` is the *same function* the published
    /// path calls, with only the source swapped.
    ///
    /// **It shares nothing, on purpose.** `share_feed_with` is not called here
    /// and calling it would be dishonest: with `debug_open_grants` the read is
    /// identical either way, so a gate that authored a share first would read as
    /// *"the grant is what made this work"* while measuring nothing of the kind.
    /// What this gate covers is the **transport and the walk**; authorization is
    /// not in its population, and [`the_live_read_needs_no_grant_today`] is the
    /// gate that says so out loud.
    #[tokio::test]
    async fn a_connected_peer_reads_a_feed_over_the_live_transport() {
        let (_author, pid_author, reader, pid_reader, hashes, handles) =
            two_peers(true).await;

        let dispatch = reader.dispatch_handle(&pid_reader).expect("the reader has a handle");
        let src = PeerFeedSource::new(dispatch, &pid_author);
        let read = read_feed(&src, &pid_author, 10).await.expect("the feed reads over the link");

        assert_eq!(read.len(), 3, "all three posts came back over the connection");
        // §4.5 — newest first. The authored order is `hashes` oldest-first.
        assert_eq!(
            read.iter().map(|r| r.hash).collect::<Vec<_>>(),
            hashes.iter().rev().copied().collect::<Vec<_>>(),
            "newest first, as published"
        );

        // **Attribution ran, and it told the truth.** These entries carry no
        // `FEED-R2` signature (nothing signed them — this fixture writes the
        // tree, not a publish), so the honest verdict is *we hold no
        // signature*, never *attributed*. A live read must not become an
        // attribution shortcut just because the bytes arrived over an
        // authenticated connection: the connection says who sent them, the
        // signature says who wrote them, and those are different claims.
        for row in &read {
            assert!(
                matches!(row.attribution, Attribution::Unattributed(_)),
                "an unsigned entry must not read as attributed just because the \
                 transport was authenticated: {:?}",
                row.attribution
            );
        }

        for h in handles {
            h.abort();
        }
    }

    /// Every `resources.include` pattern in an echoed `capability:configure`
    /// result, in order. Walks the map rather than matching on the wire bytes —
    /// see the caller for why a substring check is not good enough here.
    fn find_resource_patterns(v: &ciborium::Value) -> Vec<String> {
        fn key<'a>(m: &'a [(ciborium::Value, ciborium::Value)], k: &str) -> Option<&'a ciborium::Value> {
            m.iter().find(|(kk, _)| kk.as_text() == Some(k)).map(|(_, vv)| vv)
        }
        let Some(map) = v.as_map() else { return Vec::new() };
        let Some(grants) = key(map, "grants").and_then(|g| g.as_array()) else { return Vec::new() };
        let mut out = Vec::new();
        for g in grants {
            let Some(gm) = g.as_map() else { continue };
            let Some(res) = key(gm, "resources").and_then(|r| r.as_map()) else { continue };
            let Some(inc) = key(res, "include").and_then(|i| i.as_array()) else { continue };
            out.extend(inc.iter().filter_map(|p| p.as_text().map(str::to_string)));
        }
        out
    }

    /// ⭐ **Arch's §6 experiment, run — a grant over ANOTHER peer's namespace is
    /// accepted, and its peer-relative sibling is accepted too.**
    ///
    /// `ROUTING-2026-09-17-a` §1.3 settles `entity-workbench-go`'s report that
    /// B's grant to C naming a signature path *"canonicalizes peer-locally to
    /// B's own namespace"*: it does, correctly, because a bare pattern **is**
    /// peer-relative — and `ENTITY-CORE-PROTOCOL` §5.2's table says the
    /// universal form is the same pattern with a leading `/`. Arch asked both
    /// seats to confirm a conformant peer accepts it, naming it *"the one
    /// falsifier we could not run ourselves, because we do not run code"*.
    ///
    /// **It is accepted — five patterns, all 200, and each stored VERBATIM.**
    /// `/{A}/system/signature/*` (a third peer's segment, not the granter's),
    /// `/*/system/signature/*` (every author), the peer-relative sibling, a
    /// foreign prefix, and the granter's own. Nothing is rewritten on the way
    /// in, which is the half worth having: canonicalization happens at **check**
    /// time, exactly as §1.3 reads it, so a stored grant means what it says.
    ///
    /// ⛔ **What this does NOT establish, and the bound is the whole value of
    /// running it.** Acceptance is *the pattern is expressible and storable*,
    /// never *a read under it resolves*: enforcement here is
    /// `debug_open_grants: true` (see the posture gate below), so no read can
    /// distinguish a grant that covers it from one that does not. The peer-
    /// relative arm is carried beside it precisely so nobody reads the 2xx as
    /// discrimination — **both** spellings configure, and what separates them is
    /// resolution, which nothing here measures.
    ///
    /// ⚠⚠ **THE FIRST RUN CAME BACK 400 AND THE REFUSAL WAS THE RIG'S.**
    /// `peer_pattern` was a hand-typed `"2KSOMEREADER"`, and `configure`
    /// validates it — *"must be the literal `default`, a §3.5 invariant-pointer
    /// peer hash, or a Base58 PeerID"* — so every arm was refused for a reason
    /// that had nothing to do with the resource under test. One arm alone would
    /// have been reported to arch as **"§1.3 is wrong, a conformant peer
    /// refuses a foreign-namespace grant"**, into an open ruling, on a
    /// measurement of my own typo. What caught it was running the controls in
    /// the same loop: *the granter's own namespace* and *an ordinary feed
    /// prefix* are patterns the shipped `share_feed_with` authors every day, and
    /// they failed **identically**. ⇒ **when a probe refuses, put a pattern you
    /// already know is accepted through the same call before you believe the
    /// refusal is about your subject.** They are still in the loop.
    ///
    /// ⭐ **And it found an asymmetry nobody asked about: `peer_pattern` is
    /// validated and the resource pattern is not.** `/2KTHIRDPARTYAUTHOR/…` is
    /// not a peer id at all and is stored without complaint. That is defensible
    /// — a resource pattern is a pattern, and `/*/…` has to be legal — but it
    /// means a mistyped author in a grant is accepted and then matches nothing,
    /// which is the *correct, complete, empty answer* shape: no error anywhere,
    /// at either end. Routed rather than worked around.
    #[tokio::test]
    async fn a_grant_over_another_peers_namespace_is_accepted_by_configure() {
        use entity_capability::{GrantEntry, IdScope, PathScope};

        let registry = MemoryTransportRegistry::new();
        let (granter, pid_granter, h) = spawn_peer_on_registry(registry.clone());
        let (_reader, pid_reader, h2) = spawn_peer_on_registry(registry.clone());
        tokio::task::yield_now().await;

        // Some third peer, neither the granter nor the grantee. Deliberately a
        // literal: what is under test is the SHAPE of the pattern, and a real
        // peer id would invite the reading that acceptance depended on the peer
        // existing. It does not — a grant is a pattern, not a resolution.
        let author = "2KTHIRDPARTYAUTHOR";

        for pattern in [
            // THE SUBJECT — §5.2's universal form over a third peer's segment.
            format!("/{author}/system/signature/*"),
            // Its peer-relative sibling, which is what `entity-workbench-go`
            // reported canonicalizing locally. Also accepted; see the doc.
            "system/signature/*".to_string(),
            // Every author at once — the wider row in §1.3's table.
            "/*/system/signature/*".to_string(),
            // CONTROLS. Both are patterns the shipped `share_feed_with` writes,
            // so a run in which these are refused is a run measuring the rig.
            // Keeping them in the loop is what caught the `peer_pattern` typo.
            format!("/{pid_granter}/system/signature/*"),
            format!("/{author}/app/feed/"),
        ] {
            let grants = vec![GrantEntry {
                handlers: PathScope::new(vec!["system/tree".to_string()]),
                resources: PathScope::new(vec![pattern.clone()]),
                operations: IdScope::new(vec!["get".to_string()]),
                peers: None,
                constraints: None,
                allowances: None,
            }];
            let params = crate::share::build_configure_params(&pid_reader, &grants)
                .expect("the configure params encode");
            let result = granter
                .execute(
                    &pid_granter,
                    "system/capability".to_string(),
                    "configure".to_string(),
                    params,
                    Default::default(),
                )
                .await
                .expect("configure dispatches");
            assert!(
                result.status < 300,
                "a conformant peer refused the grant pattern {pattern:?} \
                 (status {}) — arch's §1.3 rests on this being accepted",
                result.status
            );

            // …and it is stored VERBATIM. The echoed entry is decoded rather
            // than substring-matched: the peer-relative arm is a suffix of the
            // universal one, so `contains` would pass for that arm even if the
            // pattern had been rewritten under the granter — which is the one
            // rewrite `entity-workbench-go` reported and the one this is here
            // to look for.
            let echoed: ciborium::Value = ciborium::from_reader(result.result.data.as_slice())
                .expect("the echoed policy entry decodes");
            let resources = find_resource_patterns(&echoed);
            assert_eq!(
                resources,
                vec![pattern.clone()],
                "the pattern was rewritten on the way in: {pattern:?} -> {resources:?}"
            );
        }

        h.abort();
        h2.abort();
    }

    /// The share half, asserted for **what it establishes and nothing more**:
    /// the policy entry an author would write to share their feed with one named
    /// reader is accepted by `system/capability:configure`.
    ///
    /// It is **not** evidence the read is authorized — nothing is, today. This
    /// is the piece that becomes load-bearing at cutover, and gating it now is
    /// what stops it rotting in the meantime: a `GrantEntry` shape that stopped
    /// encoding, or a `policy_entries` that stopped producing a key for a named
    /// audience, would be found here rather than on the day enforcement flips.
    #[tokio::test]
    async fn the_feed_share_policy_is_authored_even_though_nothing_enforces_it_yet() {
        let registry = MemoryTransportRegistry::new();
        let (author, pid_author, h_a) = spawn_peer_on_registry(registry.clone());
        let (_reader, pid_reader, h_b) = spawn_peer_on_registry(registry.clone());
        tokio::task::yield_now().await;

        // Panics inside on a non-2xx, which is the assertion.
        share_feed_with(&author, &pid_author, &pid_reader).await;

        h_a.abort();
        h_b.abort();
    }

    /// ⚠ **THE POSTURE, PINNED — this gate's job is to be the instrument for a
    /// concession, not its approval.**
    ///
    /// A connected peer can read a path in our tree that **nothing has shared
    /// with them, and that no grant covers**. That is `debug_open_grants: true`,
    /// set for every peer by `PeerManager::with_keypair_and_optional_connector`
    /// (`entity-core-rust/bindings/sdk`) — it overrides `PeerConfig::default()`'s
    /// `false`, so the kernel's `default_connection_grants` is never the posture
    /// anything here runs under. `share.rs` has said *"enforcement is still
    /// off"* since the SHARE work; this states the same fact as a **read**,
    /// which is the direction somebody asking *"can I read my friend's feed"*
    /// arrives from.
    ///
    /// **The path is deliberately not a feed path.** `app/secret/` is not
    /// something any share, any grant or any convention makes readable, so this
    /// cannot be satisfied by the feed subtree being legitimately open — it
    /// isolates the posture from the feature.
    ///
    /// **When enforcement flips, this test goes RED, and that is the design.**
    /// It is the tripwire that tells whoever flips it what changes: at that
    /// point `share_feed_with` stops being metadata and becomes the thing that
    /// makes [`a_connected_peer_reads_a_feed_over_the_live_transport`] pass, and
    /// `RemoteReadError::NotShared` stops being unreachable. Do not "fix" it by
    /// deleting it — invert it.
    #[tokio::test]
    async fn the_live_read_needs_no_grant_today() {
        let (author, pid_author, reader, pid_reader, _hashes, handles) =
            two_peers(false).await;

        let private = entity_entity::Entity::new(
            "app/probe/private",
            entity_ecf::to_ecf(&entity_ecf::Value::Null),
        )
        .unwrap();
        let path = format!("/{pid_author}/app/secret/not-shared-with-anybody");
        author
            .put_and_wait(&pid_author, path.clone(), private, 2_000)
            .await
            .expect("the private entity lands");

        let dispatch = reader.dispatch_handle(&pid_reader).expect("the reader has a handle");
        let seen = crate::remote_read::read_entity_at(&dispatch, &pid_author, &path).await;

        assert!(
            matches!(seen, Ok(Some(_))),
            "EITHER the posture changed and this gate is now the tripwire it was \
             built to be — invert it, and turn `share_feed_with` back on in the \
             read gate — OR something else broke. Got: {seen:?}"
        );

        for h in handles {
            h.abort();
        }
    }

    /// ⭐ **THE SHAPE A REAL AUTHOR'S TREE ACTUALLY HAS** — entries and no index,
    /// because the index is a publish artifact (`index_head_path` is referenced
    /// by nothing in this crate but its own unit test).
    ///
    /// This is §4.3 rule 6's fallback — *"a reader that cannot fetch \[the
    /// index] falls back to enumerating the prefix — slower, same answer"* — and
    /// it is the arm that makes this module usable against a peer rather than
    /// against a fixture. Without it the live reader asks a peer with three
    /// posts for `app/feed/index`, gets a 404, and reports that they have no
    /// feed.
    ///
    /// **Falsify by deleting `FeedSource::list`'s override on
    /// [`PeerFeedSource`]** — the default returns `Ok(None)` (*this source
    /// cannot enumerate*) and the read reds with `NoIndex`, which is exactly the
    /// production symptom.
    #[tokio::test]
    async fn a_peer_whose_tree_holds_only_entries_is_still_readable() {
        let (_author, pid_author, reader, pid_reader, hashes, handles) =
            two_peers(false).await;

        let dispatch = reader.dispatch_handle(&pid_reader).expect("the reader has a handle");
        let src = PeerFeedSource::new(dispatch, &pid_author);
        let read = read_feed(&src, &pid_author, 10)
            .await
            .expect("a tree with entries and no index is still a feed");

        assert_eq!(read.len(), 3, "every entry under the prefix came back");
        // **Reconstructed, not authored** — the index is what carries §4.5's
        // order and there is none, so this is `feed_tree::sort_key`'s
        // `(created_at, content_hash)` reversed. It agrees with the authored
        // order here because these posts were authored in clock order, which is
        // the ordinary case and not a guarantee; the module doc states the
        // bound.
        assert_eq!(
            read.iter().map(|r| r.hash).collect::<Vec<_>>(),
            hashes.iter().rev().copied().collect::<Vec<_>>(),
            "newest first by the reconstructed total order"
        );

        for h in handles {
            h.abort();
        }
    }
}

