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
        let mut trie_nodes = 0usize;
        for h in &closure {
            let Some(e) = shared.content_store.get(h) else { continue };
            write_blob(base, h, &entity_ecf::ecf_for_hash(&e.entity_type, &e.data))?;
            if !self.bindings.values().any(|b| b == h) {
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
pub struct DirFetcher {
    base: std::path::PathBuf,
    peer_id: String,
}

impl DirFetcher {
    pub fn new(base: impl Into<std::path::PathBuf>, peer_id: impl Into<String>) -> Self {
        Self { base: base.into(), peer_id: peer_id.into() }
    }

    fn blob_path(&self, hash: &Hash) -> std::path::PathBuf {
        let hex = hash.to_hex();
        self.base.join("content").join(&hex[0..2]).join(&hex[2..4]).join(&hex)
    }
}

impl entity_peer::published_root::ContentFetcher for DirFetcher {
    fn manifest(&self) -> Result<Vec<u8>, String> {
        let p = self.base.join(&self.peer_id).join(PUBLISHED_ROOT_REL);
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
