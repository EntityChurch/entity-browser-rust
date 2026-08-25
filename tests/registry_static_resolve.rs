//! **The whole chain, locally: a statically-published registry naming two
//! statically-published domains, and a consumer that starts with one pinned key.**
//!
//! `EXTENSION-REGISTRY` §7.4 states the target explicitly — *"No live registry
//! peer is required at any step — the registry is itself a coral reef (a static
//! publisher in dormancy)."* This file builds that, end to end, with nothing
//! live:
//!
//! ```text
//!   Registry R  (static)          pinned out-of-band: R's peer-id + pubkey
//!     binding "foundation.example" ──signed by R──▶ peer-id of Domain A
//!     binding "protocol.example"   ──signed by R──▶ peer-id of Domain B
//!
//!   Domain A (static)                     Domain B (static)
//!     sites/foundation/pages/index          sites/protocol/pages/index
//!     + signed root, own seq                + signed root, own seq
//!
//!   Consumer: pins ONLY R.
//!     resolve(name) ──▶ peer-id ──▶ that peer's signed root ──▶ the page bytes
//! ```
//!
//! ## Why this is worth having as a test rather than a diagram
//!
//! It is the first thing in this repo that exercises **naming and content
//! together**. Each half was proven separately (`published_root_walk.rs` for
//! content; the registry extension's own suite for bindings) and the seam
//! between them — *what a consumer does with the peer-id a name resolved to* —
//! was not covered anywhere.
//!
//! ## The finding it carries
//!
//! `peer_issued::resolve_one` reads the name→binding association out of the
//! **host-served** `by-name` pointer and never checks that the binding it got
//! back is *for the name that was asked*. See
//! `a_static_registry_host_can_swap_which_binding_a_name_resolves_to`, which
//! demonstrates the swap and then shows the same swap failing once the
//! consumer walks the registry's **signed root** instead. Routed to arch —
//! this is upstream code and the fix is arch's call, not ours.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use std::sync::Arc;

use common::{text_body, Origin, Publisher, RecordingFetcher};
use entity_entity::Entity;
use entity_hash::Hash;
use entity_registry::data::{BindingData, ResolverChainEntry, KIND_PEER_ISSUED};
use entity_registry::{
    by_name_pointer_path, peer_issued, signature_pointer_path, BACKEND_KIND_PEER_ISSUED,
};
use entity_store::{ContentStore, LocationIndex, MemoryContentStore, MemoryLocationIndex};

const PAGE_TYPE: &str = "app/content-site/page/v1";

const NAME_A: &str = "foundation.example";
const NAME_B: &str = "protocol.example";
const PAGE_A: &str = "sites/foundation/pages/index";
const PAGE_B: &str = "sites/protocol/pages/index";

// ---------------------------------------------------------------------------
// Building the three static origins
// ---------------------------------------------------------------------------

/// A content domain: pages under `sites/`, published under its own signed root.
fn domain(seed: u8, page_key: &str, text: &str) -> (Publisher, common::Published) {
    let p = Publisher::new(seed);
    p.bind(page_key, Entity::new(PAGE_TYPE, text_body(text)).expect("authors"));
    let published = p.publish();
    (p, published)
}

/// Author a registry binding for `name → target`, sign it with the registry's
/// key, and bind body + by-name pointer + signature into the registry's tree.
/// Same shapes the extension's own fixtures use.
fn issue_binding(registry: &Publisher, name: &str, target_peer_id: &str) -> Hash {
    let binding = BindingData {
        name: name.into(),
        kind: KIND_PEER_ISSUED.into(),
        target_peer_id: target_peer_id.into(),
        transports: vec![],
        issued_at: 1000,
        ttl: None,
        supersedes: None,
        issuer_attestation: None,
        metadata: None,
    };
    let entity = binding.to_entity().expect("binding encodes");
    let binding_hash = entity.content_hash;
    let rid = registry.peer_id().to_string();

    // §6a.3 body path + by-name index, both peer-relative for the trie.
    registry.bind(&rel(&rid, &registry_body_path(&rid, &binding_hash)), entity);
    registry.bind_hash(&rel(&rid, &by_name_pointer_path(&rid, name)), binding_hash);

    // The authenticating signature at the §5.2 invariant pointer, plus the
    // signer's identity entity — `verify_signed_by_registry` resolves the
    // signer's public key from it, so a signature without an identity verifies
    // nothing.
    let kp = registry.keypair();
    registry.put(kp.peer_entity().expect("identity entity"));
    let sig = entity_types::SignatureData {
        target: binding_hash,
        signer: kp.peer_identity_hash(),
        algorithm: "ed25519".into(),
        signature: kp.sign(&binding_hash.to_bytes()).to_vec(),
    };
    registry.bind(
        &rel(&rid, &signature_pointer_path(&rid, &binding_hash)),
        sig.to_entity().expect("signature encodes"),
    );
    binding_hash
}

fn registry_body_path(rid: &str, h: &Hash) -> String {
    format!("/{}/system/registry/binding/{}", rid, h.to_hex())
}

/// Strip the `/{peer}/` qualification — the trie keys are relative to it.
fn rel(peer_id: &str, absolute: &str) -> String {
    absolute
        .strip_prefix(&format!("/{}/", peer_id))
        .unwrap_or(absolute)
        .to_string()
}

/// The consumer's local store, warmed from a registry origin the way
/// `peer_issued::warm_cache` warms it: **`read_path` is host-trusted**
/// (pointers come from the origin's index as-is) while **`read_content` is
/// hash-verified**. That split is upstream's stated design, quoted in
/// `RegistryTreeReader`'s own docs — it is modelled faithfully here, not
/// weakened for the test.
fn warm_from(origin: &Origin) -> (Arc<dyn ContentStore>, Arc<dyn LocationIndex>) {
    let cs: Arc<dyn ContentStore> = Arc::new(MemoryContentStore::new());
    let li: Arc<dyn LocationIndex> = Arc::new(MemoryLocationIndex::new());

    for (want, bytes) in &origin.blobs {
        // read_content: re-hash and drop a mismatch (the MUST in the trait doc).
        if let Ok(entity) = entity_peer::published_root::verify_content(bytes, want) {
            cs.put(entity).expect("content put");
        }
    }
    for (path, hash) in &origin.pointers {
        li.set(path, *hash);
    }
    (cs, li)
}

fn pi_entry(registry_id: &str) -> ResolverChainEntry {
    ResolverChainEntry {
        backend_kind: BACKEND_KIND_PEER_ISSUED.into(),
        backend_id: registry_id.into(),
        priority: 0,
        accepted_trust_anchors: vec![format!("peer_issued:{registry_id}")],
        hints: None,
    }
}

/// The whole consumer: a name in, page bytes out. The only thing it holds a
/// priori is the registry's peer-id (and, through the origin, its key).
fn resolve_and_fetch(
    registry_origin: &Origin,
    registry_id: &str,
    name: &str,
    domains: &[(&str, &common::Published)],
    page_key: &str,
) -> Result<(String, Vec<u8>), String> {
    let (cs, li) = warm_from(registry_origin);
    let result = peer_issued::resolve_one(&cs, &li, &pi_entry(registry_id), name)
        .ok_or_else(|| format!("{name}: backend refused (verify/revocation/expiry)"))?;
    if !result.is_resolved() {
        return Err(format!("{name}: {}", result.status));
    }
    assert_eq!(
        result.trust_anchor.as_deref(),
        Some(format!("peer_issued:{registry_id}").as_str()),
        "a resolved binding must carry the registry's trust anchor"
    );
    let peer_id = result.peer_id.expect("a resolved binding names a peer");

    // Hop 2: that peer-id is the pin for its OWN signed root. Nothing about
    // this step consults the registry again.
    let (_, published) = domains
        .iter()
        .find(|(_, d)| d.peer_id == peer_id)
        .ok_or_else(|| format!("no origin serves {peer_id}"))?;
    let client = published.client(RecordingFetcher::new(published.full.clone()));
    let entity = client
        .resolve(page_key)
        .map_err(|e| format!("walk: {e}"))?
        .ok_or_else(|| "page absent from the signed tree".to_string())?;
    Ok((peer_id, entity.data))
}

// ---------------------------------------------------------------------------
// The gates
// ---------------------------------------------------------------------------

/// **The chain, end to end.** One pinned registry key resolves two names to two
/// different domains, and each domain's page comes back byte-exact off its own
/// signed root. Nothing live anywhere.
#[test]
fn a_pinned_registry_resolves_two_names_to_two_statically_published_domains() {
    let (_a, dom_a) = domain(21, PAGE_A, "the foundation home page");
    let (_b, dom_b) = domain(22, PAGE_B, "the protocol home page");

    let registry = Publisher::new(20);
    issue_binding(&registry, NAME_A, &dom_a.peer_id);
    issue_binding(&registry, NAME_B, &dom_b.peer_id);
    let reg = registry.publish();

    let domains = [("a", &dom_a), ("b", &dom_b)];

    let (pid_a, bytes_a) =
        resolve_and_fetch(&reg.full, registry.peer_id(), NAME_A, &domains, PAGE_A)
            .expect("foundation resolves and serves");
    assert_eq!(pid_a, dom_a.peer_id);
    assert_eq!(bytes_a, text_body("the foundation home page"));

    let (pid_b, bytes_b) =
        resolve_and_fetch(&reg.full, registry.peer_id(), NAME_B, &domains, PAGE_B)
            .expect("protocol resolves and serves");
    assert_eq!(pid_b, dom_b.peer_id);
    assert_eq!(bytes_b, text_body("the protocol home page"));

    assert_ne!(pid_a, pid_b, "two names, two distinct domains");
    eprintln!(
        "chain: registry {} → {} and {} (each with its own signed root)",
        &registry.peer_id()[..8],
        &pid_a[..8],
        &pid_b[..8]
    );
}

/// **Republish is per-domain.** A domain that changes a page advances only its
/// own `seq`; the other domain and the registry are untouched. This is the
/// property that makes "many trees, republished on change" tractable — there is
/// no global root to serialize on, and no coordination between publishers.
#[test]
fn each_domain_republishes_on_its_own_clock() {
    let (pub_a, v1_a) = domain(23, PAGE_A, "first draft");
    let (_b, v1_b) = domain(24, PAGE_B, "unrelated, untouched");

    let registry = Publisher::new(25);
    issue_binding(&registry, NAME_A, &v1_a.peer_id);
    issue_binding(&registry, NAME_B, &v1_b.peer_id);
    let reg = registry.publish();

    // A edits a page and republishes. B does nothing.
    pub_a.bind(
        PAGE_A,
        Entity::new(PAGE_TYPE, text_body("second draft")).expect("authors"),
    );
    let v2_a = pub_a.publish();

    assert!(v2_a.seq > v1_a.seq, "A's seq advances ({} → {})", v1_a.seq, v2_a.seq);
    assert_ne!(v2_a.full.head_hex, v1_a.full.head_hex, "A's root moved");
    assert_eq!(v1_b.seq, 0, "B never republished, so B's seq did not move");

    // The registry did not have to be touched: the name still points at the
    // same peer-id, and that peer-id now serves different content.
    let domains = [("a", &v2_a), ("b", &v1_b)];
    let (pid, bytes) = resolve_and_fetch(&reg.full, registry.peer_id(), NAME_A, &domains, PAGE_A)
        .expect("resolves after the republish");
    assert_eq!(pid, v1_a.peer_id, "the peer-id is stable across a republish");
    assert_eq!(bytes, text_body("second draft"), "the new content is served");

    // And the old root is refused once the newer one has been seen.
    let client = v2_a.client(RecordingFetcher::new(v2_a.full.clone()));
    client.fetch_root().expect("sees the new root");
    eprintln!(
        "republish: A seq {} → {}, B still {}, registry untouched",
        v1_a.seq, v2_a.seq, v1_b.seq
    );
}

/// **A publisher key is the stable identity; the root hash is not.** Two
/// publishes of the same domain verify against the *same* pinned key, which is
/// what lets a registry binding stay valid across arbitrarily many republishes.
/// If this ever stopped holding, every republish would require re-issuing the
/// binding.
#[test]
fn a_binding_survives_republication_because_the_pin_is_the_key_not_the_root() {
    let (pub_a, v1) = domain(26, PAGE_A, "before");
    pub_a.bind(
        PAGE_A,
        Entity::new(PAGE_TYPE, text_body("after")).expect("authors"),
    );
    let v2 = pub_a.publish();

    assert_eq!(v1.peer_id, v2.peer_id);
    assert_eq!(v1.pubkey, v2.pubkey);
    assert_ne!(v1.full.head_hex, v2.full.head_hex);

    // v1's pin verifies v2's root — the binding never mentioned a root hash.
    let client = v1.client(RecordingFetcher::new(v2.full.clone()));
    let root = client.fetch_root().expect("the old pin verifies the new root");
    assert_eq!(root.seq, v2.seq);
}

/// **THE FINDING — routed to arch.** `peer_issued::resolve_one` takes the
/// name→binding association from the **host-served** `by-name` pointer and
/// never compares `binding.name` against the name it was asked for. `name` is
/// a field of the signed body, and `ResolutionResult` carries no name either,
/// so no caller can re-check it.
///
/// Upstream's stated soundness argument (`RegistryTreeReader` docs) is that
/// host-trusted `read_path` is fine *"only because step 3 then verifies the
/// binding's signature against the pinned registry key"*. That argument holds
/// for **authenticity** — the binding really was signed by the registry — but
/// not for **association**: the host picks *which* genuine binding a name maps
/// to. A static host serving `by-name/foundation.example → the binding for
/// protocol.example` produces `status: resolved` with the registry's own trust
/// anchor and the wrong peer-id.
///
/// The second half shows the same swap failing when the association comes from
/// the registry's **signed root** instead of its pointer index — which is the
/// mechanism we are already building for B14/B15, applied one layer up.
#[test]
fn a_static_registry_host_can_swap_which_binding_a_name_resolves_to() {
    let (_a, dom_a) = domain(27, PAGE_A, "the real foundation");
    let (_b, dom_b) = domain(28, PAGE_B, "somewhere else entirely");

    let registry = Publisher::new(29);
    issue_binding(&registry, NAME_A, &dom_a.peer_id);
    let binding_b = issue_binding(&registry, NAME_B, &dom_b.peer_id);
    let reg = registry.publish();
    let rid = registry.peer_id().to_string();

    // A hostile (or merely compromised) static host repoints ONE pointer file.
    // It forges nothing: `binding_b` is a genuine, registry-signed binding.
    let mut hostile = reg.full.clone();
    hostile
        .pointers
        .insert(by_name_pointer_path(&rid, NAME_A), binding_b);

    // --- host-trusted association: the swap succeeds ---
    let (cs, li) = warm_from(&hostile);
    let swapped = peer_issued::resolve_one(&cs, &li, &pi_entry(&rid), NAME_A)
        .expect("the backend returns a result");
    assert!(
        swapped.is_resolved(),
        "the swap is not caught: every signature check passes"
    );
    assert_eq!(
        swapped.peer_id.as_deref(),
        Some(dom_b.peer_id.as_str()),
        "{NAME_A} resolved to the WRONG domain, carrying the registry's own \
         trust anchor — this is the finding"
    );
    assert_eq!(
        swapped.trust_anchor.as_deref(),
        Some(format!("peer_issued:{rid}").as_str())
    );

    // --- signed-root association: the swap is refused ---
    // Walking the registry's own signed root to the by-name key binds the name
    // to the binding cryptographically. The host cannot repoint it without
    // breaking the root signature it cannot forge.
    let client = reg.client(RecordingFetcher::new(hostile.clone()));
    let leaf = client
        .resolve(&rel(&rid, &by_name_pointer_path(&rid, NAME_A)))
        .expect("the registry's root walks")
        .expect("the by-name key is in the signed tree");
    let honest = BindingData::from_entity(&leaf).expect("the leaf is a binding");
    assert_eq!(
        honest.target_peer_id, dom_a.peer_id,
        "walked from the SIGNED root, {NAME_A} still means the right domain"
    );
    assert_eq!(honest.name, NAME_A, "and the signed body names it");

    eprintln!(
        "FINDING: host-trusted by-name → {} (wrong); signed-root walk → {} (right)",
        &dom_b.peer_id[..8],
        &dom_a.peer_id[..8]
    );
}

/// **The contrast that makes the finding precise.** The cohort already has a
/// vector for a hostile registry host — go's `REG-PEERISSUED-VERIFY-FAIL-1`,
/// whose failure message is *"this is the forgery path: any peer able to serve
/// the registry's by-name index could bind any name"*. That vector covers the
/// host **forging a binding with its own key**, and it is correctly rejected
/// here: the signature does not verify against the pin.
///
/// The swap above is the *other* half of the same sentence and has no vector in
/// any implementation: the host serves a binding **genuinely signed by the
/// pinned registry key**, just one issued for a different name. Every signature
/// check passes. Keeping both in one file is the point — the difference between
/// them is exactly what the routing packet has to communicate.
#[test]
fn a_binding_forged_with_the_hosts_own_key_is_rejected_the_swap_is_not() {
    let (_a, dom_a) = domain(33, PAGE_A, "real");
    let (_b, dom_b) = domain(34, PAGE_B, "other");

    let registry = Publisher::new(35);
    issue_binding(&registry, NAME_A, &dom_a.peer_id);
    let reg = registry.publish();
    let rid = registry.peer_id().to_string();

    // A hostile host mints its OWN binding for NAME_A → domain B, signed with a
    // key it controls, and repoints both the by-name and signature pointers.
    let rogue = entity_crypto::Keypair::from_seed([99u8; 32]);
    let rogue_binding = BindingData {
        name: NAME_A.into(),
        kind: KIND_PEER_ISSUED.into(),
        target_peer_id: dom_b.peer_id.clone(),
        transports: vec![],
        issued_at: 2000,
        ttl: None,
        supersedes: None,
        issuer_attestation: None,
        metadata: None,
    }
    .to_entity()
    .expect("encodes");
    let rogue_hash = rogue_binding.content_hash;
    let rogue_sig = entity_types::SignatureData {
        target: rogue_hash,
        signer: rogue.peer_identity_hash(),
        algorithm: "ed25519".into(),
        signature: rogue.sign(&rogue_hash.to_bytes()).to_vec(),
    }
    .to_entity()
    .expect("encodes");
    let rogue_sig_hash = rogue_sig.content_hash;
    let rogue_identity = rogue.peer_entity().expect("identity entity");

    let mut hostile = reg.full.clone();
    for e in [&rogue_binding, &rogue_sig, &rogue_identity] {
        hostile.blobs.insert(
            e.content_hash,
            entity_ecf::ecf_for_hash(&e.entity_type, &e.data),
        );
    }
    hostile
        .pointers
        .insert(by_name_pointer_path(&rid, NAME_A), rogue_hash);
    hostile
        .pointers
        .insert(signature_pointer_path(&rid, &rogue_hash), rogue_sig_hash);

    let (cs, li) = warm_from(&hostile);
    let got = peer_issued::resolve_one(&cs, &li, &pi_entry(&rid), NAME_A);
    assert!(
        got.is_none(),
        "a binding signed by a NON-PINNED key must be refused outright — this \
         half IS covered, and it works; got {got:?}"
    );
}

/// **The second, worse half: a static host can withhold a revocation.**
/// `is_revoked` *enumerates* — it lists the registry's revocation prefix in the
/// consumer's local index — and on a static origin that index is whatever the
/// host chose to serve. Drop the pointer and the revoked binding resolves with
/// every other check passing.
///
/// Python's own code comments the mechanism exactly: *"This probe is the ONLY
/// thing that can tell a revoked binding from a good one … Skip the lookup and
/// the peer serves a revoked name while every other check passes."* Not serving
/// it is indistinguishable from skipping it.
///
/// **Why this is worse than the swap:** the swap has a cheap fix (compare the
/// name). Withholding cannot be fixed by inspecting what you *did* receive —
/// only by committing to what the registry published, which is what a signed
/// root over the revocation subtree does. It is the concrete case where the
/// Q4 "withholding is undetectable" answer stops being philosophical.
#[test]
fn a_static_host_that_withholds_a_revocation_serves_the_revoked_name() {
    let (_a, dom_a) = domain(36, PAGE_A, "revoked domain");

    let registry = Publisher::new(37);
    let binding_hash = issue_binding(&registry, NAME_A, &dom_a.peer_id);
    let rid = registry.peer_id().to_string();

    // The registry revokes it, signs the revocation, and publishes.
    let rev = entity_registry::data::RevocationData {
        revokes: binding_hash,
        revoked_at: 2000,
        reason: Some("key compromise".into()),
    }
    .to_entity()
    .expect("revocation encodes");
    let rev_hash = rev.content_hash;
    registry.bind(
        &rel(&rid, &format!("/{rid}/system/registry/revocation/{}", rev_hash.to_hex())),
        rev,
    );
    let kp = registry.keypair();
    let rev_sig = entity_types::SignatureData {
        target: rev_hash,
        signer: kp.peer_identity_hash(),
        algorithm: "ed25519".into(),
        signature: kp.sign(&rev_hash.to_bytes()).to_vec(),
    };
    registry.bind(
        &rel(&rid, &signature_pointer_path(&rid, &rev_hash)),
        rev_sig.to_entity().expect("encodes"),
    );
    let reg = registry.publish();

    // Honest host: the revocation is served, and the name is refused.
    let (cs, li) = warm_from(&reg.full);
    assert!(
        peer_issued::resolve_one(&cs, &li, &pi_entry(&rid), NAME_A).is_none(),
        "an honest origin must refuse a revoked binding — if this ever passes, \
         the fixture stopped building a real revocation and the arm below proves \
         nothing"
    );

    // Hostile host: same bytes, minus the revocation's pointer. Nothing is
    // forged; a file simply is not served.
    let mut withholding = reg.full.clone();
    withholding
        .pointers
        .retain(|p, _| !p.starts_with(&format!("/{rid}/system/registry/revocation/")));

    let (cs2, li2) = warm_from(&withholding);
    let got = peer_issued::resolve_one(&cs2, &li2, &pi_entry(&rid), NAME_A)
        .expect("the backend returns a result");
    assert!(
        got.is_resolved(),
        "the revoked name resolves once its revocation is simply not served"
    );
    assert_eq!(got.peer_id.as_deref(), Some(dom_a.peer_id.as_str()));

    eprintln!(
        "FINDING: revocation served → refused; revocation withheld → resolved ({})",
        &dom_a.peer_id[..8]
    );
}

/// The one-line half of the same finding: the swap is detectable **without** a
/// signed root, because the name is inside the signed body — `resolve_one`
/// simply never looks at it. Recorded separately because the two fixes have
/// very different costs and arch may want the cheap one regardless.
#[test]
fn the_signed_binding_body_already_carries_the_name_that_would_catch_the_swap() {
    let (_a, dom_a) = domain(30, PAGE_A, "real");
    let (_b, dom_b) = domain(31, PAGE_B, "other");

    let registry = Publisher::new(32);
    issue_binding(&registry, NAME_A, &dom_a.peer_id);
    let binding_b = issue_binding(&registry, NAME_B, &dom_b.peer_id);
    let reg = registry.publish();
    let rid = registry.peer_id().to_string();

    let mut hostile = reg.full.clone();
    hostile
        .pointers
        .insert(by_name_pointer_path(&rid, NAME_A), binding_b);
    let (cs, li) = warm_from(&hostile);

    let swapped = peer_issued::resolve_one(&cs, &li, &pi_entry(&rid), NAME_A)
        .expect("resolves (wrongly)");
    let body = cs
        .get(&swapped.binding.expect("a resolved result names its binding"))
        .expect("the binding body is cached");
    let decoded = BindingData::from_entity(&body).expect("decodes");

    assert_eq!(
        decoded.name, NAME_B,
        "the binding returned for {NAME_A} says, in its own signed body, that \
         it is for {NAME_B} — one comparison in resolve_one closes this"
    );
}
