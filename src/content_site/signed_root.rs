//! **B14** — the signed root over a static publish (trust chain §6).
//!
//! What this closes: a static origin serving our `.bin` pointer mirror is
//! *trusted* — nothing in the projection is signed, so the host chooses which
//! hash answers a path. A published root makes the origin **untrusted**: the
//! consumer pins the publisher key, verifies the root's signature, and walks
//! the HAMT from the *signed* hash, re-hashing every blob it is handed. A page
//! swapped for another authored page is refused on the hash recompute — the
//! property a `path → hash` index cannot provide.
//!
//! **The finding this module exists for** (`published_root_walk.rs`, §8a of the
//! re-release buildout): the emitter enumerated *bindings* and so wrote no HAMT
//! interior nodes at all. Trie nodes appear in **no location-index listing** —
//! they are reachable only by hash from the signed root — so a signed root over
//! that projection is unwalkable, and it fails as `Ok(None)`, indistinguishable
//! from "that page does not exist". The closure is projected here, explicitly.
//!
//! **The root commits to the bytes we projected, not to the tree we read
//! from.** Every entity is recorded through [`RootProjector::record`] at the
//! moment [`super::publish_fixture::write_entity`] writes it, into this
//! projector's own peer — so the trie is built over exactly the emitted set
//! (the `sites/` + `apps/` subgraph), never over the source peer's whole tree,
//! which holds keys and app state a publish must not commit to.
//!
//! Native-only; the projector builds a real [`entity_peer::Peer`] (the shape
//! `tests/common/mod.rs` proves) rather than hand-assembling a
//! `PublishRootEngine`, so identity, store and index cannot disagree.

#![cfg(not(target_arch = "wasm32"))]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use entity_entity::Entity;
use entity_hash::{invariant_signature_path, Hash};
use entity_store::ContentStore;

pub use super::paths::PUBLISHED_ROOT_REL;

/// What a publish committed to.
#[derive(Debug)]
pub struct SignedRootReport {
    /// Hex of the published-root head — what a consumer pins a `seq` against.
    pub head_hex: String,
    /// §3.3a monotonic sequence. **0 on a first publish**, not 1.
    pub seq: u64,
    /// Keys committed to by the trie (the emitted bindings).
    pub keys: usize,
    /// HAMT interior nodes projected. Measured at ~+4.7% of the leaves at 1000
    /// keys — a rounding error against the content, not a second copy of it.
    pub trie_nodes: usize,
}

/// Accumulates what a projection emitted, then signs a root over exactly that.
pub struct RootProjector {
    peer: entity_peer::Peer,
    peer_id: String,
    /// Peer-relative key → hash. The trie operand, and the thing a consumer's
    /// `resolve` key is matched against.
    bindings: BTreeMap<String, Hash>,
    /// Bodies that must be projected but are **not** in the trie — reachable by
    /// hash only. The publisher's identity entity is the one that matters: a
    /// signature names its signer by identity hash, and a verifier that cannot
    /// fetch that entity has no public key to check against, so a signature
    /// without it verifies nothing.
    extra_blobs: BTreeSet<Hash>,
}

impl RootProjector {
    /// Build a projector under the **publisher identity** — the durable
    /// keypair (`persistence::publisher_keypair`) by default, so a deployment
    /// publishes under one stable peer-id across runs and a consumer's pin
    /// survives a republish.
    pub fn new(keypair: entity_crypto::Keypair) -> Result<Self, String> {
        let peer = entity_peer::PeerBuilder::new()
            .keypair(keypair)
            .build()
            .map_err(|e| format!("publisher peer builds: {e}"))?;
        let peer_id = peer.peer_id().to_string();
        Ok(Self { peer, peer_id, bindings: BTreeMap::new(), extra_blobs: BTreeSet::new() })
    }

    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }

    /// Record an entity the projection just wrote at `/{peer_id}/{tree_subpath}`.
    ///
    /// **Foreign peers are skipped, deliberately.** A publish can project more
    /// than one peer's subgraph (the e2e remote fixture does); a root signed by
    /// *this* key must commit only to keys under this peer, or the declared
    /// §3.3a prefix is a lie the consumer reconstructs confidently.
    pub fn record(&mut self, peer_id: &str, tree_subpath: &str, ent: &Entity) {
        if peer_id != self.peer_id {
            return;
        }
        let shared = self.peer.shared();
        let hash = ent.content_hash;
        // `put` is idempotent on content-addressed storage — a body emitted
        // twice (an asset shared across sites) lands once.
        let _ = shared.content_store.put(ent.clone());
        shared
            .location_index
            .set(&format!("/{}/{}", self.peer_id, tree_subpath), hash);
        self.bindings.insert(tree_subpath.to_string(), hash);
    }

    /// Bind an **already-stored** hash at a key, with no new body.
    ///
    /// The registry's `by-name` index is exactly this shape: the key
    /// `system/registry/binding/by-name/{norm}` names the *binding's* hash, and
    /// the binding body lives at its own content-addressed key. One entity, two
    /// trie keys — so recording it as a body twice would publish it twice.
    pub fn record_hash(&mut self, peer_id: &str, tree_subpath: &str, hash: Hash) {
        if peer_id != self.peer_id {
            return;
        }
        self.peer
            .shared()
            .location_index
            .set(&format!("/{}/{}", self.peer_id, tree_subpath), hash);
        self.bindings.insert(tree_subpath.to_string(), hash);
    }

    /// Store a body that is reachable **by hash only** — no trie key, but still
    /// projected. See [`Self::extra_blobs`] for why the identity entity is not
    /// optional.
    pub fn put_only(&mut self, ent: &Entity) -> Hash {
        let hash = ent.content_hash;
        let _ = self.peer.shared().content_store.put(ent.clone());
        self.extra_blobs.insert(hash);
        hash
    }

    /// The publisher's own identity entity — the thing a signature's `signer`
    /// field resolves to. Recorded as a hash-only body.
    pub fn publish_identity(&mut self) -> Result<Hash, String> {
        let ent = self
            .peer
            .shared()
            .keypair
            .peer_entity()
            .map_err(|e| format!("publisher identity entity: {e:?}"))?;
        Ok(self.put_only(&ent))
    }

    /// Sign `target` with the publisher's key, returning `(signature, signer
    /// identity hash)` — the two fields a §5.2 `system/signature` entity needs.
    ///
    /// Handed back as bytes rather than as a keypair reference on purpose: the
    /// signing key never leaves this type, and a caller cannot accidentally sign
    /// something else with it.
    pub fn sign_detached(&self, target: &Hash) -> (Vec<u8>, Hash) {
        let shared = self.peer.shared();
        let sig = shared.keypair.sign(&target.to_bytes()).to_vec();
        (sig, shared.keypair.peer_identity_hash())
    }

    /// The signing algorithm name for a §5.2 signature entity.
    pub fn algorithm(&self) -> &'static str {
        match self.peer.shared().keypair.key_type() {
            entity_crypto::KeyType::Ed448 => "ed448",
            _ => "ed25519",
        }
    }

    /// How many keys have been recorded so far.
    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    /// The recorded key → hash map, for **measurement gates only**.
    ///
    /// `#[cfg(test)]` deliberately: *which keys moved between two publishes* is
    /// the question §4.3 rule 1's `[MUST]` is about, and there is no way to ask
    /// it from outside without this. Nothing in the product may branch on the
    /// binding set — `finish` is the only thing entitled to read it.
    #[cfg(test)]
    pub(crate) fn bindings_for_measurement(&self) -> BTreeMap<String, Hash> {
        self.bindings.clone()
    }

    /// Load the published root already sitting in `base` into this projector's
    /// peer, so the next publish **chains off it** instead of restarting the
    /// §3.3a sequence at zero.
    ///
    /// **Why this is load-bearing, and how it was missed for the whole arc.**
    /// `PublishRootEngine::publish` derives `seq` from `current_head()` — the
    /// publisher peer's *own* location index. Every CLI invocation builds a
    /// fresh in-memory peer ([`Self::new`]), so that index was always empty and
    /// **every emit published `seq 0`**, no matter how many times the directory
    /// had been published before. Measured: two different registry contents,
    /// same key, both `seq 0`.
    ///
    /// The consequence is not cosmetic. `SignedSession`'s rollback floor refuses
    /// a root whose `seq` went *backwards*; two trees both at zero never do. So
    /// a host holding yesterday's bytes could serve them forever — signature
    /// valid, every hash valid, seq not lower — and the one defence against a
    /// republish being rolled back was inert **against our own publisher**.
    /// A withdrawn binding, a rotated target, a revocation: all re-servable.
    /// The durable identity persisted the *key* across runs and nothing
    /// persisted the *head*, which is exactly the sort of half that looks
    /// present until someone measures it.
    ///
    /// A missing prior head is a first publish (`seq 0`, no predecessor). A
    /// prior head that is *present and unreadable* is an **error**, never a
    /// silent restart: restarting the sequence is indistinguishable from the
    /// rollback this exists to prevent, and "absent" and "unreadable" arriving
    /// as the same value is the seam this repo has now met five times.
    pub fn adopt_prior_head(&self, base: &Path) -> Result<(), String> {
        match read_prior_head(base, &self.peer_id)? {
            Some(prior) => self.adopt_prior_head_bytes(&prior),
            None => Ok(()),
        }
    }

    /// [`Self::adopt_prior_head`] from bytes already read.
    ///
    /// The projection emitter needs this split because it **cleans
    /// `{base}/{peer_id}/` before writing** — by the time `finish` runs, the
    /// prior head has been deleted, and reading it there would find nothing and
    /// silently restart the sequence. So the bytes are carried across the clean.
    pub fn adopt_prior_head_bytes(&self, prior: &PriorHead) -> Result<(), String> {
        let entity = entity_wire::decode_entity(&prior.manifest)
            .map_err(|e| format!("prior published root does not decode: {e}"))?;
        let data = entity_types::PublishedRootData::from_entity(&entity)
            .map_err(|e| format!("prior published root is not a published-root: {e}"))?;
        // A head published by someone else is not our sequence to continue —
        // and adopting it would let a foreign tree in the output directory
        // dictate our `seq`.
        if data.peer_id != self.peer_id {
            return Err(format!(
                "the prior published root was published by {} — this identity is {}. \
                 Publishing a different peer into the same directory would either restart \
                 the sequence or continue someone else's",
                data.peer_id, self.peer_id
            ));
        }
        let shared = self.peer.shared();
        let hash = entity.content_hash;
        shared.content_store.put(entity).map_err(|e| format!("store prior head: {e}"))?;
        shared
            .location_index
            .set(&entity_peer::published_root::published_root_head_path(&self.peer_id), hash);
        // The signature over that head, at the invariant path `finish` reads it
        // from — so an UNCHANGED republish, which takes `publish`'s idempotent
        // early return and signs nothing, can still project one.
        if let Some((sig_hash, body)) = &prior.signature {
            // Verified against the hash the pointer named, with the same
            // function a consumer uses — adopting a body that does not hash to
            // its own address would re-project a signature nothing can verify.
            let sig_entity = super::http_poll::verify_and_decode(body, sig_hash)
                .map_err(|e| format!("prior signature does not verify: {e:?}"))?;
            shared
                .content_store
                .put(sig_entity)
                .map_err(|e| format!("store prior signature: {e}"))?;
            shared
                .location_index
                .set(&invariant_signature_path(&self.peer_id, &hash), *sig_hash);
        }
        Ok(())
    }

    /// Build the HAMT, sign a root over it, and project the closure + manifest
    /// + signature into `base` (an already-prefixed projection root).
    ///
    /// Takes `&self`, not `self`, so a projector can **republish**: record more
    /// (or changed) keys and call again, and `seq` advances because the peer's
    /// `publish_root` chains `predecessor` off the prior head. `publish_root` is
    /// idempotent when the root hash is unchanged, so calling twice on identical
    /// bindings does not inflate `seq`.
    pub fn finish(&self, base: &Path) -> Result<SignedRootReport, String> {
        if self.bindings.is_empty() {
            return Err("nothing to sign: the projection recorded no bindings".into());
        }
        let shared = self.peer.shared();

        // **Continue the sequence this directory is already at.** See
        // [`Self::adopt_prior_head`] — without this every emit is `seq 0` and
        // the consumer's rollback floor cannot see a rollback at all.
        self.adopt_prior_head(base)?;

        let root = entity_tree::trie::build_trie(shared.content_store.as_ref(), &self.bindings)
            .map_err(|e| format!("trie builds: {e}"))?;
        let head = self
            .peer
            .publish_root(root)
            .map_err(|e| format!("publishes a signed root: {e}"))?;

        // The manifest is the 3-key WIRE entity — `verify_signed_root` reads
        // its `content_hash`, which the bare hashable form does not carry.
        let manifest_entity = shared
            .content_store
            .get(&head)
            .ok_or_else(|| "head is not in the publisher's store".to_string())?;
        let seq = entity_types::PublishedRootData::from_entity(&manifest_entity)
            .map_err(|e| format!("head decodes as a published-root: {e}"))?
            .seq;
        write_file(
            &base.join(&self.peer_id).join(PUBLISHED_ROOT_REL),
            &entity_wire::encode_entity(&manifest_entity),
        )?;

        // The trie closure. Reachable only by hash from the signed root, so
        // nothing that enumerates the location index will emit these.
        let closure = trie_closure(shared.content_store.as_ref(), root);
        // **A SET, not a linear scan per closure member.** This counter is
        // report-only — `trie_nodes` is a line of output — and it was spelled
        // `self.bindings.values().any(…)` *inside* the closure loop, i.e.
        // `O(|closure| × |bindings|)`. Measured on a feed: `finish` went
        // quadratic and a 16,000-post archive took **20.4 s**, of which the
        // counter was **17.7 s**; the same publish is 2.6 s with this set.
        // Nothing else in the emit path is superlinear, so for two months the
        // whole of publish's scaling ceiling was a diagnostic nobody reads.
        // ⇒ *a report-only computation is still on the critical path.*
        let bound: BTreeSet<&Hash> = self.bindings.values().collect();
        let mut trie_nodes = 0usize;
        for h in &closure {
            let Some(e) = shared.content_store.get(h) else { continue };
            write_blob(base, h, &entity_ecf::ecf_for_hash(&e.entity_type, &e.data))?;
            if !bound.contains(h) {
                trie_nodes += 1;
            }
        }

        // The signature, projected through the ordinary two-hop convention
        // (pointer + blob) so a consumer's fetcher reads it exactly like any
        // other entity. `publish_root` bound it at the invariant path.
        let sig_path = invariant_signature_path(&self.peer_id, &head);
        let sig_hash = shared
            .location_index
            .get(&sig_path)
            .ok_or_else(|| format!("publisher bound no signature at {sig_path}"))?;
        let sig_entity = shared
            .content_store
            .get(&sig_hash)
            .ok_or_else(|| "signature entity is not stored".to_string())?;
        write_blob(
            base,
            &sig_hash,
            &entity_ecf::ecf_for_hash(&sig_entity.entity_type, &sig_entity.data),
        )?;
        write_file(
            &base
                .join(&self.peer_id)
                .join(format!("system/signature/{}.bin", head.to_hex())),
            &entity_ecf::ecf_for_hash_value(
                "system/hash",
                &entity_ecf::Value::Bytes(sig_hash.to_bytes()),
            ),
        )?;

        // Bodies outside the trie (the identity entity). Not committed to by
        // the root — deliberately: an identity is verified by its own hash,
        // which the signature already names.
        for h in &self.extra_blobs {
            let Some(e) = shared.content_store.get(h) else { continue };
            write_blob(base, h, &entity_ecf::ecf_for_hash(&e.entity_type, &e.data))?;
        }

        // The head entity itself is also fetched by hash during the walk.
        write_blob(
            base,
            &head,
            &entity_ecf::ecf_for_hash(&manifest_entity.entity_type, &manifest_entity.data),
        )?;

        Ok(SignedRootReport {
            head_hex: head.to_hex(),
            seq,
            keys: self.bindings.len(),
            trie_nodes,
        })
    }
}

/// Transitive hash closure of a trie root, by walking node bodies for embedded
/// The published-root manifest already sitting in `base` for `peer_id`, as
/// bytes — the input to [`RootProjector::adopt_prior_head_bytes`].
///
/// Read it **before** any clean: an emitter that wipes `{base}/{peer_id}/`
/// destroys its own sequence marker, and a sequence that restarts at zero is
/// indistinguishable from the rollback the sequence exists to detect.
///
/// `None` means genuinely absent (a first publish). An unreadable file is an
/// error, not a `None`.
pub fn read_prior_head(base: &Path, peer_id: &str) -> Result<Option<PriorHead>, String> {
    let path = base.join(peer_id).join(PUBLISHED_ROOT_REL);
    let manifest = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("read prior published root {}: {e}", path.display())),
    };
    // The prior signature comes with it. See [`PriorHead::signature`] — without
    // it an unchanged republish takes `publish_root`'s idempotent early return
    // and then cannot re-project a signature it never made.
    let head_hex = entity_wire::decode_entity(&manifest)
        .map(|e| e.content_hash.to_hex())
        .map_err(|e| format!("prior published root at {} does not decode: {e}", path.display()))?;
    let ptr_path = base.join(peer_id).join(format!("system/signature/{head_hex}.bin"));
    let signature = match fs::read(&ptr_path) {
        Ok(ptr) => match super::http_poll::crack_pointer(&ptr) {
            Ok(sig_hash) => {
                let hex = sig_hash.to_hex();
                let blob = base.join("content").join(&hex[0..2]).join(&hex[2..4]).join(&hex);
                fs::read(&blob).ok().map(|body| (sig_hash, body))
            }
            Err(_) => None,
        },
        Err(_) => None,
    };
    Ok(Some(PriorHead { manifest, signature }))
}

/// A previously-published root, carried across an emitter's clean.
pub struct PriorHead {
    /// The `published-root` manifest, as the 3-key wire entity on disk.
    manifest: Vec<u8>,
    /// The §5.2 signature over that head — `(hash, bare hashable body)`.
    ///
    /// Carried because `PublishRootEngine::publish` **returns the prior head
    /// unchanged when the content did not change**, and then signs nothing. That
    /// is right for a live peer, whose store still holds the old signature; it is
    /// wrong for a re-projecting CLI, which has just deleted its own output and
    /// has to write every artifact again. Without it, republishing an *unchanged*
    /// site fails with "publisher bound no signature" — the idempotent path, i.e.
    /// the most ordinary republish there is.
    ///
    /// `None` if it could not be read: the publish then either makes a new one
    /// (content changed) or fails loudly on the missing signature, which is
    /// better than emitting a root nothing can verify.
    signature: Option<(Hash, Vec<u8>)>,
}

/// 33-byte hashes. Deliberately structure-agnostic: a publisher must not need
/// to know the HAMT's internal encoding to project it, and if that assumption
/// is ever wrong this is where it shows.
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

/// `{base}/content/{aa}/{bb}/{hex66}` — the same sharded content address the
/// rest of the projection writes, so a blob emitted here is indistinguishable
/// from one emitted by `write_entity`.
fn write_blob(base: &Path, hash: &Hash, bytes: &[u8]) -> Result<(), String> {
    let hex = hash.to_hex();
    let dir = base.join("content").join(&hex[0..2]).join(&hex[2..4]);
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    fs::write(dir.join(&hex), bytes).map_err(|e| format!("write blob {hex}: {e}"))
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    fs::write(path, bytes).map_err(|e| format!("write {}: {e}", path.display()))
}

/// The artifact name a static publisher ships its own endpoint under.
///
/// Deliberately **not** at `manifest_url_prefix` — §6.5.3 reserves that slot for
/// the signed root and calls serving a transport profile there non-conformant,
/// naming it as *"a reasonable mistake"*: a static origin has no live surface to
/// answer "what are your transports", that slot is the one singular terminal
/// thing it serves, and a profile is plausibly "the manifest". It is not, and a
/// consumer following `signed_pointer` there would find a profile and no signed
/// root. Ours sits beside the site, which is what the spec says to do instead.
pub const TRANSPORT_PROFILE_REL: &str = "transport-profile";

/// **Emit our own `http-poll` endpoint beside the publish**, so a consumer can
/// *discover* where our artifacts are instead of sharing our convention.
///
/// This is the publisher half of §6.5.3 v1.8. Until it existed, our reader
/// derived the manifest's location by convention — which the ruling forbids —
/// and had no alternative, because **we advertised the endpoint only inside a
/// registry binding's `transports`**. A consumer meeting a bare
/// `entity-browser` origin had no endpoint document at all, so the rule was
/// satisfiable against other publishers and not against us. Found by running the
/// cross-implementation check in the other direction (`ROUTING-2026-08-19-c` §4).
///
/// Emitted **after** the signed root, and only when one was written: the profile
/// advertises `signed_pointer`, which under Amendment 10 obliges the trie
/// closure to be present. Advertising it beside a tree that has no root is the
/// false claim §6.5.3 warns about.
pub fn write_transport_profile(base: &Path, peer_id: &str, origin: &str) -> Result<(), String> {
    // ONE builder shared with the registry's by-hash reference (D8) — a
    // consumer that meets the standalone artifact and the referenced entity
    // must not be able to tell them apart, which two constructions of "the same"
    // entity cannot guarantee.
    let entity =
        crate::content_site::registry_publish::http_poll_profile_entity(peer_id, origin)?;
    write_file(&base.join(TRANSPORT_PROFILE_REL), &entity_wire::encode_entity(&entity))
}

// ---------------------------------------------------------------------------
// A fetcher over a projected directory
// ---------------------------------------------------------------------------

/// A [`ContentFetcher`] reading a **projected publish directory** — what a
/// static origin serves, minus the HTTP.
///
/// This is B15's shape with the transport removed, and it is deliberately here
/// rather than in the test: the three layout decisions it encodes are the
/// publisher's, so the consumer that has to undo them belongs beside the code
/// that made them.
///
/// 1. **The manifest is the 3-key wire entity** at [`PUBLISHED_ROOT_REL`]; our
///    content blobs everywhere else are the 2-key bare hashable form.
/// 2. **Content is sharded `{aa}/{bb}/{hex}`**, not upstream's flat `content_url`.
/// 3. **The signature is a two-hop** — a `system/hash` pointer at
///    `system/signature/{head_hex}.bin` naming a blob — where upstream's
///    `signature_url` serves the leaf directly.
///
/// ## The manifest is DISCOVERED, and #1 above is why that had to change
///
/// Divergence #1 was a *choice* — and `EXTENSION-NETWORK` §6.5.3 v1.8 has since
/// ruled that a consumer **MUST NOT** derive the manifest's location by
/// convention: it is read from the profile's `manifest_url_prefix`, and
/// `{origin}/manifest` is as conformant as ours. So this fetcher reads
/// `{base}/transport-profile` when the publisher emitted one
/// ([`crate::content_site::publish_layout`]), and falls back to the convention
/// only when there is none.
///
/// The fallback is not a shortcut kept for comfort — it is the **absence of a
/// source**. Our own `make site` publishes ship no profile artifact yet, so
/// every existing gate in this tree enters that arm; a publisher we have never
/// met, who does emit one, enters the other. That asymmetry is the open item,
/// not the design.
pub struct DirFetcher {
    base: std::path::PathBuf,
    peer_id: String,
    /// The origin this directory stands for — needed to map an advertised
    /// absolute URL back onto a file under `base`. `None` uses the fixture-
    /// friendly default of "whatever origin the profile itself names".
    origin: Option<String>,
}

impl DirFetcher {
    /// A directory whose layout follows **our** convention, or which carries a
    /// `transport-profile` naming the origin it was published for.
    pub fn new(base: impl Into<std::path::PathBuf>, peer_id: impl Into<String>) -> Self {
        Self { base: base.into(), peer_id: peer_id.into(), origin: None }
    }

    /// A directory standing in for a specific origin — use this when the
    /// publisher's advertised URLs are rooted somewhere other than the profile's
    /// own `tree_url_prefix`.
    pub fn at_origin(
        base: impl Into<std::path::PathBuf>,
        peer_id: impl Into<String>,
        origin: impl Into<String>,
    ) -> Self {
        Self { base: base.into(), peer_id: peer_id.into(), origin: Some(origin.into()) }
    }

    fn blob_path(&self, hash: &Hash) -> std::path::PathBuf {
        let hex = hash.to_hex();
        self.base.join("content").join(&hex[0..2]).join(&hex[2..4]).join(&hex)
    }

    /// The publisher's emitted endpoint, if it shipped one beside the site
    /// **and it is about the peer we are reading**.
    ///
    /// `transport-profile` is ONE artifact per hosting scope and its contents are
    /// per-peer, so where several publishers share an origin the last publish
    /// wins. Following it blindly then locates *the other peer's* tree — measured
    /// 2026-09-03 as `publish --verify` reporting *"the signed root is not
    /// walkable — a pinned consumer resolves NOTHING from this tree"* against a
    /// tree that was completely intact.
    ///
    /// A **positive mismatch** disqualifies; a profile that names no peer is
    /// still trusted, because absence is not evidence of a different peer and
    /// rejecting on it would break reading conformant publishers that omit the
    /// field (see `profile_peer_id`).
    fn discovered_layout(&self) -> Option<super::publish_layout::PublishLayout> {
        let bytes = fs::read(self.base.join("transport-profile")).ok()?;
        if let Some(declared) = super::publish_layout::PublishLayout::profile_peer_id(&bytes) {
            if declared != self.peer_id {
                return None;
            }
        }
        super::publish_layout::PublishLayout::from_profile_artifact(&bytes)
    }

    /// Where the manifest actually is, in order of authority: what the publisher
    /// advertised, then our own convention.
    fn manifest_path(&self) -> std::path::PathBuf {
        if let Some(layout) = self.discovered_layout() {
            // `origin_for`, not `tree_url_prefix` — a peer-rooted prefix
            // (`/{peer}`, our own same-origin emission) is not the origin, and
            // stripping it as one drops the peer segment from every path.
            let origin =
                self.origin.clone().unwrap_or_else(|| layout.origin_for(&self.peer_id));
            if let Some(rel) =
                super::publish_layout::PublishLayout::relative_to_origin(&layout.manifest_url, &origin)
            {
                return self.base.join(rel);
            }
        }
        self.base.join(&self.peer_id).join(PUBLISHED_ROOT_REL)
    }
}

impl entity_peer::published_root::ContentFetcher for DirFetcher {
    fn manifest(&self) -> Result<Vec<u8>, String> {
        let p = self.manifest_path();
        fs::read(&p).map_err(|e| format!("manifest {}: {e}", p.display()))
    }

    fn content(&self, hash: &Hash) -> Result<Vec<u8>, String> {
        let p = self.blob_path(hash);
        fs::read(&p).map_err(|e| format!("content {}: {e}", p.display()))
    }

    fn signature_for(&self, target: &Hash) -> Result<Option<Vec<u8>>, String> {
        // Hop 1: the pointer. Absent is a legitimate answer (an origin that
        // serves no signature), so it is `Ok(None)`, not an error.
        let ptr = self
            .base
            .join(&self.peer_id)
            .join(format!("system/signature/{}.bin", target.to_hex()));
        let Ok(bytes) = fs::read(&ptr) else { return Ok(None) };
        // The same `system/hash` cracker the HTTP-poll consumer uses — the
        // pointer shape is one fact, and a second decoder here is how the two
        // drift.
        let hash = super::http_poll::crack_pointer(&bytes)
            .map_err(|e| format!("signature pointer at {}: {e}", ptr.display()))?;
        // Hop 2: the blob it names. Here an absence IS an error — the pointer
        // asserted it exists, so a miss is a broken projection, not a posture.
        Ok(Some(self.content(&hash)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_site::format::{NavItem, SiteManifest, SitePage};
    use crate::content_site::publish_fixture::emit_owned_sites;
    use crate::content_site::read::OwnedSite;
    use entity_peer::published_root::PublishedRootClient;

    const SEED: [u8; 32] = [0x5a; 32];

    fn site(peer: &str, id: &str) -> OwnedSite {
        OwnedSite {
            peer_id: peer.to_string(),
            site_id: id.to_string(),
            manifest: SiteManifest::new(id, "Signed", "index", vec![NavItem::new("Home", "/index")]),
            pages: vec![
                ("index".into(), SitePage::markdown("Home", "# Home\n\nauthored bytes")),
                ("deep/one".into(), SitePage::markdown("Deep", "# Deep\n\nnested")),
            ],
            assets: vec![],
            content: Vec::new(),
        }
    }

    /// Emit a projection with a signed root and hand back everything a
    /// consumer needs to pin it.
    fn publish_into(dir: &Path) -> (String, Vec<u8>, entity_crypto::KeyType, SignedRootReport) {
        let kp = entity_crypto::Keypair::from_seed(SEED);
        let pubkey = kp.public_key_bytes().to_vec();
        let key_type = kp.key_type();
        let mut root = RootProjector::new(kp).expect("projector builds");
        let peer_id = root.peer_id().to_string();
        let s = site(&peer_id, "signed");
        emit_owned_sites(dir, std::slice::from_ref(&s), "", Some(&mut root)).unwrap();
        let report = root.finish(dir).expect("signs a root");
        (peer_id, pubkey, key_type, report)
    }

    fn client(dir: &Path, peer_id: &str, pubkey: Vec<u8>, kt: entity_crypto::KeyType)
        -> PublishedRootClient<DirFetcher>
    {
        PublishedRootClient::new(
            DirFetcher::new(dir, peer_id),
            pubkey,
            kt,
            Some(peer_id.to_string()),
        )
    }

    /// **A REPUBLISH CONTINUES THE SEQUENCE — and for the whole naming arc it
    /// did not.**
    ///
    /// `PublishRootEngine::publish` derives `seq` from the publisher peer's own
    /// location index, and every CLI invocation builds a **fresh in-memory
    /// peer** — so every emit published `seq 0`, however many times the
    /// directory had been published. Measured on the shipped binary before the
    /// fix: two different registry contents, same key, both zero.
    ///
    /// That made `SignedSession`'s rollback floor inert against our own
    /// publisher. The floor refuses a root whose `seq` went *backwards*; two
    /// trees at zero never do, so a host holding yesterday's bytes could serve
    /// them forever with every signature and every hash checking out. A
    /// withdrawn binding, a rotated target, a revocation — all re-servable, with
    /// nothing in the chain able to say so. The durable identity persisted the
    /// *key* across runs; nothing persisted the *head*.
    ///
    /// **Two separate projectors, deliberately.** The existing republish test
    /// reuses one projector, which is a live peer's shape and the case that
    /// always worked. Two projectors over one directory is the CLI's shape, and
    /// the only one that could catch this.
    #[test]
    fn a_second_publish_into_the_same_directory_advances_the_sequence() {
        let dir = tempfile::tempdir().unwrap();
        let (peer_id, pubkey, kt, first) = publish_into(dir.path());
        assert_eq!(first.seq, 0, "a first publish is seq 0");

        // A second CLI run: new process, new peer, same durable key, same dir.
        let kp = entity_crypto::Keypair::from_seed(SEED);
        let mut root = RootProjector::new(kp).expect("projector builds");
        root.adopt_prior_head(dir.path()).expect("adopts the head already published here");
        let mut changed = site(&peer_id, "signed");
        changed.pages.push((
            "second".into(),
            SitePage::markdown("Second", "# Second\n\npublished later"),
        ));
        emit_owned_sites(dir.path(), std::slice::from_ref(&changed), "", Some(&mut root)).unwrap();
        let second = root.finish(dir.path()).expect("signs a root");

        assert_eq!(second.seq, 1, "a republish must continue the sequence, not restart it");
        assert_ne!(second.head_hex, first.head_hex, "different content, different head");

        // And the consumer can tell the two apart, which is the whole point: a
        // session that has seen the newer root refuses the older one.
        let newer = client(dir.path(), &peer_id, pubkey.clone(), kt);
        let head = newer.fetch_root().expect("head fetches");
        assert_eq!(head.seq, 1, "the consumer reads the advanced seq off disk");

        // The third publish is UNCHANGED content: the engine's idempotent path,
        // which returns the prior head and signs nothing. It must still project
        // a complete tree — this is the most ordinary republish there is, and it
        // failed with "publisher bound no signature" until the prior signature
        // was carried across the clean too.
        let kp = entity_crypto::Keypair::from_seed(SEED);
        let mut root = RootProjector::new(kp).expect("projector builds");
        root.adopt_prior_head(dir.path()).expect("adopts");
        emit_owned_sites(dir.path(), std::slice::from_ref(&changed), "", Some(&mut root)).unwrap();
        let third = root.finish(dir.path()).expect("an unchanged republish still emits");
        assert_eq!(third.seq, 1, "unchanged content must NOT inflate the sequence");
        assert_eq!(third.head_hex, second.head_hex);
    }

    /// **The B14 gate.** A publish emits a signed root, and a consumer holding
    /// nothing but the publisher's key walks it off disk to the authored bytes.
    ///
    /// The property that matters is not "a file appeared" — it is that the
    /// **HAMT closure** is on disk. Before this module the projection wrote
    /// leaves and pointers only, and a signed root over it resolved `Ok(None)`
    /// for every key: indistinguishable from "that page does not exist".
    #[test]
    fn a_published_projection_is_walkable_from_its_signed_root() {
        let dir = tempfile::tempdir().unwrap();
        let (peer_id, pubkey, kt, report) = publish_into(dir.path());
        assert_eq!(report.seq, 0, "a first publish is seq 0, not 1");
        assert!(report.trie_nodes > 0, "a projection with no trie nodes is unwalkable");

        let c = client(dir.path(), &peer_id, pubkey, kt);
        let root = c.fetch_root().expect("the signed root verifies against the pinned key");
        assert_eq!(root.seq, 0);

        for key in ["sites/signed/manifest", "sites/signed/pages/index", "sites/signed/pages/deep/one"] {
            let got = c
                .resolve(key)
                .unwrap_or_else(|e| panic!("resolve {key}: {e:?}"))
                .unwrap_or_else(|| panic!("{key} resolved to nothing — is the trie closure projected?"));
            assert!(!got.data.is_empty(), "{key} came back empty");
        }
    }

    /// The page bytes are the **authored** ones, byte-exact — the walk is not
    /// merely returning *an* entity that happens to be at that key.
    #[test]
    fn a_walked_page_is_byte_exact_with_what_was_authored() {
        let dir = tempfile::tempdir().unwrap();
        let (peer_id, pubkey, kt, _) = publish_into(dir.path());
        let authored = SitePage::markdown("Home", "# Home\n\nauthored bytes").to_entity();

        let c = client(dir.path(), &peer_id, pubkey, kt);
        let got = c.resolve("sites/signed/pages/index").unwrap().expect("index resolves");
        assert_eq!(got.content_hash, authored.content_hash);
        assert_eq!(got.data, authored.data);
    }

    /// **The property that makes the origin untrusted.** Swap a page's blob for
    /// another *validly authored* page — no forgery, no signature touched — and
    /// the walk refuses it on the hash recompute. A `path → hash` pointer mirror
    /// cannot do this: the host picks the hash.
    #[test]
    fn a_page_swapped_for_another_authored_page_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (peer_id, pubkey, kt, _) = publish_into(dir.path());

        // The bytes of `deep/one`, written over the address of `index`.
        let index = SitePage::markdown("Home", "# Home\n\nauthored bytes").to_entity();
        let other = SitePage::markdown("Deep", "# Deep\n\nnested").to_entity();
        let hex = index.content_hash.to_hex();
        let victim = dir.path().join("content").join(&hex[0..2]).join(&hex[2..4]).join(&hex);
        assert!(victim.exists(), "the leaf we are about to substitute must be projected");
        fs::write(&victim, entity_ecf::ecf_for_hash(&other.entity_type, &other.data)).unwrap();

        let c = client(dir.path(), &peer_id, pubkey, kt);
        let got = c.resolve("sites/signed/pages/index");
        assert!(
            !matches!(&got, Ok(Some(e)) if e.data == other.data),
            "a substituted page must never be served as the authored one: {got:?}"
        );
    }

    /// A root signed by a **different** key is refused against the pin — the
    /// whole point of pinning the publisher rather than the origin.
    #[test]
    fn a_root_signed_by_another_key_is_refused_against_the_pin() {
        let dir = tempfile::tempdir().unwrap();
        let (peer_id, _, _, _) = publish_into(dir.path());
        let impostor = entity_crypto::Keypair::from_seed([0x77; 32]);

        let c = client(dir.path(), &peer_id, impostor.public_key_bytes().to_vec(), impostor.key_type());
        assert!(
            c.fetch_root().is_err(),
            "a root signed by a key we did not pin must not verify"
        );
    }

    /// **The mutation check for this whole module.** Remove *exactly* the HAMT
    /// interior nodes — nothing else — and the walk stops resolving. That is
    /// today's pre-B14 emitter reproduced: leaves and pointers on disk, no
    /// closure, and every key answering `Ok(None)`.
    ///
    /// The removal set is computed precisely rather than by "everything that is
    /// not a leaf": deleting the head or the signature would break `fetch_root`
    /// and the test would pass for a reason that has nothing to do with the
    /// trie. `removed == report.trie_nodes` is the check on the check.
    #[test]
    fn without_the_trie_closure_the_same_projection_does_not_resolve() {
        let dir = tempfile::tempdir().unwrap();
        let (peer_id, pubkey, kt, report) = publish_into(dir.path());

        let c0 = client(dir.path(), &peer_id, pubkey.clone(), kt);
        assert!(c0.fetch_root().is_ok(), "the root must verify before we break the closure");

        // Keep: every authored leaf, the head, and the signature blob. Remove
        // the rest — which is precisely the interior nodes.
        let mut keep: BTreeSet<String> = BTreeSet::new();
        keep.insert(report.head_hex.clone());
        for key in ["sites/signed/manifest", "sites/signed/pages/index", "sites/signed/pages/deep/one"] {
            let e = c0.resolve(key).unwrap().expect("leaf resolves before the break");
            keep.insert(e.content_hash.to_hex());
        }
        let sig_ptr = dir
            .path()
            .join(&peer_id)
            .join(format!("system/signature/{}.bin", report.head_hex));
        let sig_hash = crate::content_site::http_poll::crack_pointer(&fs::read(&sig_ptr).unwrap())
            .expect("the signature pointer decodes");
        keep.insert(sig_hash.to_hex());

        let mut removed = 0usize;
        for entry in walk_blobs(&dir.path().join("content")) {
            let name = entry.file_name().unwrap().to_string_lossy().to_string();
            if !keep.contains(&name) {
                fs::remove_file(&entry).unwrap();
                removed += 1;
            }
        }
        assert_eq!(
            removed, report.trie_nodes,
            "the removal set must be exactly the trie nodes ({} reported)",
            report.trie_nodes
        );

        let c = client(dir.path(), &peer_id, pubkey, kt);
        assert!(
            c.fetch_root().is_ok(),
            "the root itself must still verify — otherwise this proves nothing about the trie"
        );
        let got = c.resolve("sites/signed/pages/index");
        assert!(
            !matches!(&got, Ok(Some(_))),
            "with the trie closure gone the walk must not resolve: {got:?}"
        );
    }

    fn walk_blobs(dir: &Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        let Ok(rd) = fs::read_dir(dir) else { return out };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk_blobs(&p));
            } else {
                out.push(p);
            }
        }
        out
    }
}
