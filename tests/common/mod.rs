//! Shared harness for the static-publish tests: a peer that authors entities,
//! signs a root over them, and **projects** the result as the files a bucket
//! would serve.
//!
//! Used by `published_root_walk.rs` (does the trust chain compose?) and
//! `registry_static_resolve.rs` (does it compose across a registry and two
//! domains?). Native only; `entity-tree` / `entity-wire` are test-only
//! dev-deps — see `Cargo.toml`.

#![cfg(not(target_arch = "wasm32"))]
#![allow(dead_code)] // each test crate uses a different subset

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use entity_crypto::{KeyType, Keypair};
use entity_entity::Entity;
use entity_hash::{invariant_signature_path, Hash};
use entity_peer::published_root::{ContentFetcher, PublishedRootClient};
use entity_peer::PeerBuilder;
use entity_store::ContentStore;

// ---------------------------------------------------------------------------
// What a static publish put on the bucket
// ---------------------------------------------------------------------------

/// The files a static origin serves. Deliberately split so a test can hand a
/// consumer one half and not the other:
///
/// - `blobs` — the `hash → bytes` content store. **Self-verifying**: a
///   consumer re-hashes, so the host cannot lie here.
/// - `pointers` — the `path → hash` index (our `.bin` pointer mirror). **Host
///   asserted**: nothing about it is signed, and a consumer that trusts it is
///   trusting the origin.
/// - `manifest` / `signature` — the signed root and its signature.
#[derive(Default, Clone)]
pub struct Origin {
    pub manifest: Vec<u8>,
    pub signature: Option<Vec<u8>>,
    /// Hex of the published-root hash this origin's signature targets.
    pub head_hex: String,
    pub blobs: BTreeMap<Hash, Vec<u8>>,
    pub pointers: BTreeMap<String, Hash>,
}

impl Origin {
    /// Everything except the trie nodes — today's projection
    /// (`content_site/publish_fixture.rs`): authored bodies plus the pointer
    /// mirror, and nothing a signed root can be walked through.
    pub fn without_trie_nodes(&self, leaves: &BTreeSet<Hash>) -> Origin {
        let mut o = self.clone();
        o.blobs.retain(|h, _| leaves.contains(h));
        o
    }
}

// ---------------------------------------------------------------------------
// The publisher
// ---------------------------------------------------------------------------

pub struct Published {
    pub peer_id: String,
    pub pubkey: Vec<u8>,
    pub key_type: KeyType,
    /// The correct projection: authored leaves **and** the trie closure.
    pub full: Origin,
    /// Every authored leaf (not trie nodes) in this publish.
    pub leaf_hashes: BTreeSet<Hash>,
    pub seq: u64,
}

impl Published {
    pub fn client<F: ContentFetcher>(&self, fetcher: F) -> PublishedRootClient<F> {
        PublishedRootClient::new(
            fetcher,
            self.pubkey.clone(),
            self.key_type,
            Some(self.peer_id.clone()),
        )
    }
}

/// A live publisher. Kept alive across publishes so `seq` can advance — the
/// state a one-shot helper cannot express.
pub struct Publisher {
    peer: entity_peer::Peer,
    keypair: Keypair,
    peer_id: String,
    /// Peer-relative key → hash. The trie is built over exactly this, which is
    /// the `tracking-config`-over-a-prefix shape rather than the whole store.
    bindings: RefCell<BTreeMap<String, Hash>>,
}

impl Publisher {
    pub fn new(seed: u8) -> Self {
        // `Keypair` is not `Clone`, and the peer consumes one — so derive the
        // same identity twice from the seed. Holding our own copy is what lets
        // a test sign registry bindings with the publishing peer's key.
        let keypair = Keypair::from_seed([seed; 32]);
        let peer = PeerBuilder::new()
            .keypair(Keypair::from_seed([seed; 32]))
            .build()
            .expect("publisher peer builds");
        let peer_id = peer.peer_id().to_string();
        Self { peer, keypair, peer_id, bindings: RefCell::new(BTreeMap::new()) }
    }

    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }

    pub fn keypair(&self) -> &Keypair {
        &self.keypair
    }

    /// Store an entity without binding it to a path — content reachable by
    /// hash only (identity entities, which are resolved from a signature's
    /// `signer` field rather than from a path).
    pub fn put(&self, entity: Entity) -> Hash {
        let h = entity.content_hash;
        self.peer.shared().content_store.put(entity).expect("content put");
        h
    }

    /// Author an entity at a peer-relative key. The key is what a consumer
    /// passes to `PublishedRootClient::resolve`.
    pub fn bind(&self, rel_key: &str, entity: Entity) -> Hash {
        let hash = self.put(entity);
        self.bind_hash(rel_key, hash);
        hash
    }

    /// Bind an already-stored hash at a peer-relative key.
    pub fn bind_hash(&self, rel_key: &str, hash: Hash) {
        self.peer
            .shared()
            .location_index
            .set(&format!("/{}/{}", self.peer_id, rel_key), hash);
        self.bindings.borrow_mut().insert(rel_key.to_string(), hash);
    }

    /// Build a HAMT over everything bound so far, sign the root, and project
    /// the result as static files. Mirrors what `entity-browser publish` does,
    /// minus the disk write.
    pub fn publish(&self) -> Published {
        let shared = self.peer.shared();
        let bindings = self.bindings.borrow().clone();

        let root = entity_tree::trie::build_trie(shared.content_store.as_ref(), &bindings)
            .expect("trie builds");

        // Default prefix is the peer-qualified `/{peer_id}/`, which is what the
        // keys above are relative to.
        let head = self.peer.publish_root(root).expect("publishes a signed root");
        assert_eq!(
            self.peer.published_root_head(),
            Some(head),
            "the head pointer must serve what we just published"
        );

        // `manifest` is the 3-key WIRE entity — `verify_signed_root` reads its
        // `content_hash` — while content blobs are the 2-key bare hashable
        // form, which is what our `content/` files already carry.
        let manifest_entity = shared.content_store.get(&head).expect("head is stored");
        let manifest = entity_wire::encode_entity(&manifest_entity);
        let seq = entity_types::PublishedRootData::from_entity(&manifest_entity)
            .expect("head decodes as a published-root")
            .seq;

        let sig_path = invariant_signature_path(&self.peer_id, &head);
        let signature = shared.location_index.get(&sig_path).map(|h| {
            let e = shared.content_store.get(&h).expect("signature entity is stored");
            entity_ecf::ecf_for_hash(&e.entity_type, &e.data)
        });
        assert!(
            signature.is_some(),
            "publisher must bind a signature at the invariant path {sig_path}"
        );

        let bare = |h: &Hash| -> Option<Vec<u8>> {
            let e = shared.content_store.get(h)?;
            Some(entity_ecf::ecf_for_hash(&e.entity_type, &e.data))
        };

        let mut full = Origin {
            manifest,
            signature,
            head_hex: head.to_hex(),
            ..Default::default()
        };

        // The authored leaves, and the pointer mirror over them.
        let mut leaf_hashes = BTreeSet::new();
        for (key, h) in &bindings {
            if let Some(bytes) = bare(h) {
                leaf_hashes.insert(*h);
                full.blobs.insert(*h, bytes);
            }
            full.pointers
                .insert(format!("/{}/{}", self.peer_id, key), *h);
        }

        // The trie closure. These appear in NO location-index listing — they
        // are reachable only by hash from the signed root, which is why a
        // projector that enumerates bindings emits an unwalkable tree.
        for h in trie_closure(shared.content_store.as_ref(), root) {
            if let Some(bytes) = bare(&h) {
                full.blobs.insert(h, bytes);
            }
        }

        Published {
            peer_id: self.peer_id.clone(),
            pubkey: shared.keypair.public_key_bytes(),
            key_type: shared.keypair.key_type(),
            full,
            leaf_hashes,
            seq,
        }
    }
}

/// Transitive hash closure of a trie root, by walking node bodies for embedded
/// 33-byte hashes. Deliberately structure-agnostic: a publisher must not need
/// to know the HAMT's internal encoding to project it, and if that assumption
/// is wrong this is where it shows.
pub fn trie_closure(store: &dyn ContentStore, root: Hash) -> BTreeSet<Hash> {
    let mut seen = BTreeSet::new();
    let mut queue = vec![root];
    while let Some(h) = queue.pop() {
        if !seen.insert(h) {
            continue;
        }
        let Some(entity) = store.get(&h) else { continue };
        for w in entity.data.windows(33) {
            if let Ok(candidate) = Hash::from_bytes(w) {
                if !seen.contains(&candidate) && store.get(&candidate).is_some() {
                    queue.push(candidate);
                }
            }
        }
    }
    seen
}

// ---------------------------------------------------------------------------
// Consumers
// ---------------------------------------------------------------------------

/// The fetch log, shared with the test after the fetcher is moved into the
/// client. `PublishedRootClient` owns its fetcher and exposes no accessor, so
/// the handle has to be taken before construction.
#[derive(Clone, Default)]
pub struct AskLog(pub Arc<Mutex<Vec<String>>>);

impl AskLog {
    /// Every distinct hash the consumer fetched — the closure a static publish
    /// must project.
    pub fn closure(&self) -> BTreeSet<String> {
        self.0.lock().unwrap().iter().cloned().collect()
    }
}

/// A [`ContentFetcher`] over an [`Origin`] that records every hash requested.
pub struct RecordingFetcher {
    pub origin: Origin,
    asked: AskLog,
    /// `(requested hash, bytes_to_serve_instead)` — the substitution arm.
    substitute: Option<(Hash, Vec<u8>)>,
}

impl RecordingFetcher {
    pub fn new(origin: Origin) -> Self {
        Self { origin, asked: AskLog::default(), substitute: None }
    }

    /// A hostile origin that answers a request for `victim` with somebody
    /// else's well-formed bytes. Sharper than corrupting a byte: the served
    /// body decodes cleanly, so **only the hash recompute** can catch it.
    pub fn substituting(origin: Origin, victim: Hash, instead: Vec<u8>) -> Self {
        Self { origin, asked: AskLog::default(), substitute: Some((victim, instead)) }
    }

    pub fn log(&self) -> AskLog {
        self.asked.clone()
    }
}

impl ContentFetcher for RecordingFetcher {
    fn manifest(&self) -> Result<Vec<u8>, String> {
        Ok(self.origin.manifest.clone())
    }

    fn content(&self, hash: &Hash) -> Result<Vec<u8>, String> {
        self.asked.0.lock().unwrap().push(hash.to_hex());
        if let Some((victim, instead)) = &self.substitute {
            if victim == hash {
                return Ok(instead.clone());
            }
        }
        self.origin
            .blobs
            .get(hash)
            .cloned()
            .ok_or_else(|| format!("404 {}", hash.to_hex()))
    }

    fn signature_for(&self, _target: &Hash) -> Result<Option<Vec<u8>>, String> {
        Ok(self.origin.signature.clone())
    }
}

/// One authored body, encoded the way a page is.
pub fn text_body(text: &str) -> Vec<u8> {
    entity_ecf::to_ecf(&entity_ecf::Value::Map(vec![(
        entity_ecf::text("body"),
        entity_ecf::text(text),
    )]))
}
