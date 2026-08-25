//! **The full path** — a name in, a verified page out.
//!
//! ```text
//! pin: the registry's peer-id + key
//!   │
//!   ├─ walk the REGISTRY's signed root → system/registry/binding/by-name/{name}
//!   │     → the binding entity  →  a domain peer-id
//!   │
//!   └─ walk THAT domain's signed root → sites/{site}/pages/{slug}
//!         → the authored bytes
//! ```
//!
//! Two hops, **one pin**, nothing live. `EXTENSION-REGISTRY` §7.4's coral reef,
//! composed out of the pieces B14/B15/B16a already built — a registry tree is
//! just a tree, so hop 1 is [`SignedSession`] pointed at a different key space.
//!
//! ## Why this resolves over the signed root instead of `peer_issued::resolve_one`
//!
//! Not to avoid upstream — to close **our own routed finding** on our own
//! consumer. `resolve_one` reads the binding hash out of the **host-served**
//! `by-name` pointer, so a static host can repoint one pointer file, forge
//! nothing, and answer `foundation.example` with the binding the registry
//! legitimately issued for `protocol.example`
//! (`ROUTING-2026-08-17-e` F1; confirmed 3-of-3 by arch). Walking the registry's
//! **signed root** removes the host's choice entirely: it cannot repoint a trie
//! leaf without breaking a signature it cannot forge.
//!
//! We do the name comparison **as well** ([`NameError::NameMismatch`]), because
//! that is arch's actual ruling (D1) and it is one comparison: the signature
//! covers a body carrying `name`, so `sig(R, {name, target_peer_id})` **is** the
//! commitment and a resolver that ignores it is discarding one it already holds.
//! Belt and braces — and the check is what keeps this correct if the walk is
//! ever swapped for a host-trusted read.
//!
//! ## What this does NOT close, stated exactly
//!
//! **Revocation withholding is narrowed, not closed** (F2). We check the §6a.6
//! by-target key *inside the signed tree*, so a host cannot drop a revocation
//! out of a root it is serving. What it can still do is serve an **older signed
//! root** — one published before the revocation — which is a rollback, and a
//! rollback is caught only by [`SignedSession`]'s `seq` floor *within a session*.
//! Across a cold start there is no floor, and the bound falls back to the
//! binding's **TTL**. That is precisely why arch's D3 makes a non-null `ttl` a
//! MUST and why [`super::registry_publish`] cannot express a null one.
//!
//! ## Arch's §4 invariant — we already satisfy it, and it was worth checking
//!
//! `ROUTING-2026-08-18-j` §4: *"the absence of a node is never an answer"* — a
//! node that resolved without the key is `not_found`; a node that did not
//! resolve is a failed walk. Arch dispositioned our consumer side as *"closes
//! when the engines land it"*, on the assumption we inherit
//! `collect_bindings_into`'s tolerant walk. **We do not**, because resolution
//! here is `trie_get` behind [`SignedSession`]'s pump: a miss inside the walk is
//! *recorded*, the pump then **fetches** it, and a withheld blob turns that into
//! [`SignedFetchError::Transport`]. `Absent` is reachable only when the walk
//! completed with nothing outstanding. Verified rather than argued —
//! `an_unwalkable_tree_terminates_rather_than_spinning` (mutation-checked) and
//! `a_withheld_node_can_never_produce_a_resolved_name` (6 blobs on the walk,
//! each withheld in turn, none became an answer).
//!
//! That is what makes the revocation probe **fail closed**: a withheld node on
//! that path lands in `NameError::Registry`, not in the `Absent` arm that means
//! "not revoked". The remaining gap is a *fixture* gap, named at
//! `the_revocation_probe_adds_no_fetches_of_its_own` — we cannot emit a
//! published-then-withheld revocation, so that exact shape is unproven.

use entity_hash::Hash;
use entity_registry::data::{normalize_name, BindingData, KIND_PEER_ISSUED};
use entity_registry::{by_name_pointer_path, revocation_by_target_path};

use super::http_poll::BinSource;
use super::publish_layout::PublishLayout;
use super::signed_fetch::{SignedFetchError, SignedSession};

/// Why a name did not resolve to a usable target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameError {
    /// The registry's own tree could not be read or did not verify.
    Registry(SignedFetchError),
    /// The registry's signed tree carries no binding for this name. **Not** a
    /// host's answer — an absence committed to by the root.
    NotBound,
    /// The binding the registry served is for a *different* name. This is the
    /// substitution vector; on the signed-root path it should be unreachable,
    /// and it is checked anyway.
    NameMismatch { asked: String, got: String },
    /// A `peer-issued` binding with no TTL. Refused (arch D3): a null-TTL
    /// binding whose revocation an origin withholds is permanently unrevokable.
    NoTtl,
    Expired { expired_at_ms: u64 },
    /// The registry published a revocation naming this binding.
    Revoked,
    /// A kind this resolver does not implement.
    UnsupportedKind(String),
    Malformed(String),
}

impl std::fmt::Display for NameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Registry(e) => write!(f, "registry: {e}"),
            Self::NotBound => write!(f, "no binding for this name in the signed registry"),
            Self::NameMismatch { asked, got } => {
                write!(f, "the registry served a binding for {got:?}, not {asked:?}")
            }
            Self::NoTtl => write!(f, "binding carries no ttl (refused)"),
            Self::Expired { expired_at_ms } => write!(f, "binding expired at {expired_at_ms}"),
            Self::Revoked => write!(f, "binding is revoked"),
            Self::UnsupportedKind(k) => write!(f, "unsupported binding kind {k:?}"),
            Self::Malformed(e) => write!(f, "malformed binding: {e}"),
        }
    }
}

/// **B17 — say what was verified.** A resolved name and an unchecked one must
/// not look alike, so every success carries the evidence that produced it.
///
/// Nothing here is a boolean the caller can set: each field records a check that
/// **A resolver's own bound on how long it will believe a binding** —
/// `EXTENSION-REGISTRY` §6a's resolver-side ceiling (1.11, arch `d1584a1`).
///
/// A resolver that declares a local maximum MUST treat a binding's effective
/// lifetime as **`min(binding.ttl, local_max)`**, computed at resolution and
/// **never written back**: the binding's content hash is unchanged, because this
/// is a *use* bound and not a re-issue.
///
/// **This is the half that protects the consumer, and it is the only half we
/// can hold.** §6a.3's whole argument is about the party bearing the risk — a
/// hostile origin withholds a revocation and `ttl` bounds the exposure — so a
/// ceiling enforced by the *registry* cannot defend anyone against that
/// registry, which simply issues itself a long one. The split is DNS's: the
/// authority sets the record's TTL, the **resolver** caps what it will honor,
/// because the resolver is the one holding stale data.
///
/// It bites us specifically. Our `SignedSession` `seq` floor lives for the life
/// of the session, so across a **cold start** the only bound on a withheld
/// revocation is the TTL the registry chose (this is the narrowed-not-closed
/// half of F2). A ceiling here is what shortens that window.
///
/// **`None` is the default and is conformant** — §6a makes the ceiling a MAY,
/// and a MUST only once declared. We ship no number for the reason arch writes
/// none: there is no defensible constant, and picking one makes every
/// unconfigured deployment *look* configured. A deployment that wants the
/// protection states its own.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResolverPolicy {
    /// The resolver's ceiling in **milliseconds**, matching `binding.ttl`'s
    /// unit (`issued_at`/`ttl` are ms — emitting seconds yields a permanently
    /// expired binding, which reads as a broken registry).
    pub max_ttl_ms: Option<u64>,
}

impl ResolverPolicy {
    /// No ceiling declared. Conformant, and the honest default.
    pub const fn undeclared() -> Self {
        Self { max_ttl_ms: None }
    }

    /// Declare a ceiling. From here on the MUST binds: every resolution clamps.
    pub const fn with_max_ttl_ms(ms: u64) -> Self {
        Self { max_ttl_ms: Some(ms) }
    }

    /// `min(ttl, local_max)`. The whole rule.
    pub fn effective_ttl_ms(&self, binding_ttl_ms: u64) -> u64 {
        match self.max_ttl_ms {
            Some(cap) => binding_ttl_ms.min(cap),
            None => binding_ttl_ms,
        }
    }
}

/// actually ran on this resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameEvidence {
    /// The registry we pinned. The *only* thing a consumer holds a priori.
    pub registry_peer_id: String,
    /// The binding's own content address.
    pub binding_hash: Hash,
    /// True when the name→binding association came from the registry's **signed
    /// root** rather than a host-served pointer. Always true on this path; it is
    /// a field rather than an assumption so a future host-trusted fallback
    /// cannot silently claim the same standing.
    pub association_committed: bool,
    /// `binding.name` was compared to the queried name (arch D1).
    pub name_checked: bool,
    /// The §6a.6 by-target revocation key was probed **inside the signed tree**.
    pub revocation_checked: bool,
    /// When this binding stops being believable — the bound on a withheld
    /// revocation, and the reason a null TTL is refused. **Already clamped** by
    /// [`ResolverPolicy`], so a surface that displays this is showing what this
    /// resolver will actually honor.
    pub expires_at_ms: u64,
    /// The TTL the registry **issued**, before any local ceiling. Kept beside
    /// the clamped answer so a surface can say *why* an expiry is sooner than
    /// the registry said, and so "we clamped" is never inferred from a
    /// subtraction.
    pub issued_ttl_ms: u64,
    /// The lifetime this resolver will actually honor: `min(issued, local_max)`.
    pub effective_ttl_ms: u64,
}

impl NameEvidence {
    /// Whether this resolver's own ceiling shortened the registry's TTL.
    pub fn ttl_was_clamped(&self) -> bool {
        self.effective_ttl_ms < self.issued_ttl_ms
    }
}

/// A name, resolved.
#[derive(Debug, Clone)]
pub struct NamedTarget {
    pub name: String,
    pub peer_id: String,
    /// Where that peer's static publish is served, read out of the binding's
    /// `transports` (`EXTENSION-NETWORK` §6.5.3 `http-poll`).
    ///
    /// **`None` is the pre-configured case, not a failure.** A binding with no
    /// transports resolves fine; the consumer just knows *who* and not *where*,
    /// and the origin has to come from `entity-deployment.json` or a
    /// registration it already holds. With `Some`, one pinned registry is enough
    /// to reach a domain nothing configured.
    ///
    /// `None` means **no `http-poll` profile in the binding, or one that does
    /// not advertise what a fetch needs** — never "a layout we cannot consume",
    /// which was the case audit F6 left open and
    /// [`layout`](Self::layout) closed.
    pub origin: Option<String>,
    /// **Where that peer's artifacts are, as the publisher advertised them.**
    ///
    /// This is what the signed walk uses. [`origin`](Self::origin) is derived
    /// from it for the transport-trusted `.list` menus only — a convenience
    /// surface where nothing is verified — and the two must never be re-fused:
    /// deriving a fetch URL from the origin is exactly the hop-0 defect
    /// §6.5.3 v1.8 forbids.
    pub layout: Option<PublishLayout>,
    pub evidence: NameEvidence,
}

/// Resolve `name` through a pinned registry's signed tree.
///
/// `registry` is a [`SignedSession`] over the **registry's** publish; hold it
/// across resolutions for the `seq` floor and the cache (see its docs).
/// `now_ms` is passed in rather than read so a caller can be deterministic and
/// so this compiles identically on wasm, where the clock is a different API.
/// `policy` is this resolver's own ceiling ([`ResolverPolicy`]). It is a
/// required argument rather than an optional one deliberately: it is the half
/// of §6a's TTL bound that protects the *consumer*, and an overload that
/// silently skips it is how one half of a gate ends up unreachable from the
/// surface that needed it.
pub async fn resolve_name<S: BinSource + ?Sized>(
    src: &S,
    registry: &SignedSession,
    name: &str,
    now_ms: u64,
    policy: &ResolverPolicy,
) -> Result<NamedTarget, NameError> {
    let registry_id = registry.pin().peer_id.clone();
    // Same normalization the resolver uses, from the same function — see
    // `registry_publish::normalized` for why re-deriving it is how the two ends
    // drift apart.
    let norm = normalize_name(name.trim(), "none");

    let key = rel(&registry_id, &by_name_pointer_path(&registry_id, &norm));
    let entity = match registry.resolve(src, &key).await {
        Ok(e) => e,
        Err(SignedFetchError::Absent) => return Err(NameError::NotBound),
        Err(e) => return Err(NameError::Registry(e)),
    };
    let binding_hash = entity.content_hash;
    let binding =
        BindingData::from_entity(&entity).map_err(|e| NameError::Malformed(format!("{e:?}")))?;

    // D1 — the association the registry signed. On this path the root already
    // committed to it; the comparison costs one string and holds the invariant
    // if the read ever becomes host-trusted again.
    if binding.name != norm {
        return Err(NameError::NameMismatch { asked: norm, got: binding.name });
    }
    if binding.kind != KIND_PEER_ISSUED {
        return Err(NameError::UnsupportedKind(binding.kind));
    }

    // D3 — a null TTL is refused rather than treated as "never expires".
    let ttl = binding.ttl.ok_or(NameError::NoTtl)?;
    // §6a resolver-side ceiling: min(binding.ttl, local_max), computed HERE and
    // never written back — `binding_hash` below is the hash of the binding as
    // published, because this is a use bound, not a re-issue.
    let effective_ttl = policy.effective_ttl_ms(ttl);
    let expires_at_ms = binding.issued_at.saturating_add(effective_ttl);
    if expires_at_ms <= now_ms {
        return Err(NameError::Expired { expired_at_ms: expires_at_ms });
    }

    // §6a.6's O(1) by-target index, probed inside the SIGNED tree — so the
    // answer "not revoked" is an absence the root committed to, not an absence
    // the host chose. See the module docs for what this still does not close.
    let rev_key = rel(&registry_id, &revocation_by_target_path(&registry_id, &binding_hash));
    match registry.resolve(src, &rev_key).await {
        Ok(_) => return Err(NameError::Revoked),
        Err(SignedFetchError::Absent) => {}
        Err(e) => return Err(NameError::Registry(e)),
    }

    let layout = http_poll_layout(&binding.transports);
    Ok(NamedTarget {
        name: norm,
        origin: layout.as_ref().map(|l: &PublishLayout| l.origin_for(&binding.target_peer_id)),
        layout,
        peer_id: binding.target_peer_id,
        evidence: NameEvidence {
            registry_peer_id: registry_id,
            binding_hash,
            association_committed: true,
            name_checked: true,
            revocation_checked: true,
            issued_ttl_ms: ttl,
            effective_ttl_ms: effective_ttl,
            expires_at_ms,
        },
    })
}

/// **Read the publisher's `http-poll` endpoint out of a binding's `transports`.**
///
/// Returns the layout **as advertised** — `manifest_url_prefix`,
/// `content_url_prefix`, `content_layout`, `tree_url_prefix`,
/// `tree_leaf_suffix`. Nothing is re-derived, which is the §6.5.3 v1.8 MUST:
/// *the manifest's location is DISCOVERED from the profile, never derived by
/// convention from the tree path.*
///
/// ## What this replaced, and why the old shape could not be patched
///
/// Until now this function returned an *origin string* — the `tree_url_prefix`
/// with a trailing `/{peer_id}` conditionally stripped — and
/// [`PinnedPublisher`] rebuilt every URL from it by our own convention. The
/// function's own doc comment recorded the consequence: *"a publisher who is not
/// us is unconsumable even when their profile told us everything we needed."*
/// The cross-implementation run turned that recorded limit into a measured
/// **hop-0 failure** against workbench-go's origin, so the "trust-chain refactor,
/// deliberately not this cleanup" is now the work
/// (`ROUTING-2026-08-19-c`).
///
/// The conditional strip survives — it is [`PublishLayout::origin_for`], the
/// audit-F6 rule intact (act only when the last segment is **exactly** the
/// peer-id, because an unconditional split turns a legal
/// `tree_url_prefix: "https://x.example"` into `"https:/"`). What changed is
/// that a failure to recognise our own shape no longer means *unconsumable*:
/// it means the prefix is origin-rooted, which is the other conformant form.
///
/// `None` now means something narrow and true: **no `http-poll` profile in this
/// binding, or one that does not advertise what a fetch needs** — not "a layout
/// we cannot consume".
///
/// [`PinnedPublisher`]: super::signed_fetch::PinnedPublisher
/// [`PublishLayout::origin_for`]: super::publish_layout::PublishLayout::origin_for
fn http_poll_layout(transports: &[entity_ecf::Value]) -> Option<PublishLayout> {
    transports.iter().find_map(PublishLayout::from_http_poll_profile)
}

/// Strip the `/{peer}/` qualification — the upstream path helpers return
/// absolute paths and a trie key is relative to the publisher's prefix.
fn rel(peer_id: &str, absolute: &str) -> String {
    absolute
        .strip_prefix(&format!("/{peer_id}/"))
        .unwrap_or(absolute)
        .to_string()
}

/// **List what a registry carries** — the registry-browser floor.
///
/// Reads the transport-trusted `by-name.list` enumeration artifact
/// ([`super::paths::NAMES_LIST_PATH`]), the sibling of a peer's
/// `sites.list`. Returns the names as offered; **every one of them still has to
/// be resolved** through [`resolve_name`] before it means anything.
///
/// **This list is not authoritative — but not for the reason this doc used to
/// give.** It claimed a signed root cannot answer *"what keys exist"*. That is
/// **false and was refuted by arch** (`1782d6d`): `EXTENSION-TREE` §3.1's leaf is
/// `[key, value_hash]`, so the keys are *in* the nodes, and upstream
/// `entity_tree::trie::collect_all_bindings` recovers the whole reachable key set
/// — see [`super::registry_publish`]'s tests, which do exactly that. Publish at
/// `system/registry/binding/by-name/` and the trie's key set **is** the name set.
///
/// Two real reasons it is still a menu rather than an inventory:
///
/// 1. **This reader has no trie decoder.** `collect_all_bindings` is a native
///    `ContentStore` walk; the browser side ([`SignedSession`]) resolves *keys*
///    and never decodes node structure. Enumerating from the signed root here is
///    buildable, not built.
/// 2. **Even the signed walk is silently short.** `collect_bindings_into` skips a
///    missing `Entry::Link` with a bare `if let Some(..)` and returns a
///    `BTreeMap`, not a `Result` — so an origin withholding one interior node
///    hides names with no error and a root hash that still verifies (measured:
///    1 of 24, `withholding_a_trie_node_shortens_the_walk_silently`). Arch's D9
///    says a walk MUST NOT be shortened without failing; the detection half is
///    upstream and not landed.
///
/// So: a hostile origin can hide a name (invisible on **either** path today) or
/// invent one (it fails to resolve). **Every name here must still go through
/// [`resolve_name`] before it means anything**, and any surface built on this
/// must treat an unresolvable entry as a failure, never as the end of a branch.
///
/// [`SignedSession`]: super::signed_fetch::SignedSession
pub async fn list_names<S: BinSource + ?Sized>(
    src: &S,
    registry: &super::signed_fetch::PinnedPublisher,
) -> Result<Vec<String>, String> {
    let url = format!(
        "{}/{}/{}.list",
        registry.origin.trim_end_matches('/'),
        registry.peer_id,
        super::paths::NAMES_LIST_PATH
    );
    let bytes = src
        .get(url, super::http_poll::Freshness::Mutable)
        .await
        .map_err(|e| format!("by-name listing: {e}"))?;
    Ok(String::from_utf8_lossy(&bytes)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::content_site::format::{NavItem, SiteManifest, SitePage};
    use crate::content_site::http_poll::{Freshness, PollError};
    use crate::content_site::publish_fixture::emit_owned_sites;
    use crate::content_site::read::OwnedSite;
    use crate::content_site::registry_publish::{
        emit_registry, BindingSpec, DEFAULT_TTL_MS,
    };
    use crate::content_site::signed_fetch::PinnedPublisher;
    use crate::content_site::signed_root::RootProjector;
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::path::{Path, PathBuf};
    use std::pin::Pin;

    /// **The deployment we are modelling**, and the thing worth reading off this
    /// test: what a real seeding looks like. One registry, four domains, each
    /// its own publish under its own key, each named by one entry.
    const DOMAINS: &[(&str, &str, u8)] = &[
        ("entitychurch.org", "foundation", 0xA1),
        ("protocol.entitychurch.org", "protocol", 0xA2),
        ("docs.entitychurch.org", "docs", 0xA3),
        ("lab.entitychurch.org", "lab", 0xA4),
    ];
    const REGISTRY_SEED: u8 = 0xB0;

    /// Every origin in one tree, keyed by its URL prefix — which is exactly what
    /// several HTTP origins look like once you take the network out.
    struct LocalWeb {
        root: PathBuf,
        fetched: RefCell<Vec<String>>,
    }

    impl LocalWeb {
        fn new(root: &Path) -> Self {
            Self { root: root.to_path_buf(), fetched: RefCell::new(Vec::new()) }
        }
        fn count(&self) -> usize {
            self.fetched.borrow().len()
        }
    }

    impl BinSource for LocalWeb {
        fn get(
            &self,
            url: String,
            _freshness: Freshness,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, PollError>>>> {
            self.fetched.borrow_mut().push(url.clone());
            let rel = url.trim_start_matches('/');
            let r = std::fs::read(self.root.join(rel))
                .map_err(|e| crate::content_site::http_poll::poll_error_for_io(rel, &e));
            Box::pin(std::future::ready(r))
        }
    }

    fn block_on<F: Future>(future: F) -> F::Output {
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

    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// Publish one domain into `{root}/{slug}/` and return its peer-id. The page
    /// body names the domain, so a mis-resolution cannot pass unnoticed.
    fn publish_domain(root: &Path, slug: &str, seed: u8, name: &str) -> String {
        let dir = root.join(slug);
        std::fs::create_dir_all(&dir).unwrap();
        let mut projector =
            RootProjector::new(entity_crypto::Keypair::from_seed([seed; 32])).unwrap();
        let peer_id = projector.peer_id().to_string();
        let site = OwnedSite {
            peer_id: peer_id.clone(),
            site_id: slug.to_string(),
            manifest: SiteManifest::new(slug, name, "index", vec![NavItem::new("Home", "/index")]),
            pages: vec![(
                "index".into(),
                SitePage::markdown(name, &format!("# {name}\n\nserved by {slug}")),
            )],
            assets: vec![],
        };
        emit_owned_sites(&dir, std::slice::from_ref(&site), "", Some(&mut projector)).unwrap();
        projector.finish(&dir).unwrap();
        peer_id
    }

    /// Stand up the whole local deployment: four domains, then one registry
    /// naming all four. Returns `(registry peer-id, name → peer-id)`.
    fn stand_up(root: &Path) -> (String, BTreeMap<String, String>) {
        let mut map = BTreeMap::new();
        let mut specs = Vec::new();
        for (name, slug, seed) in DOMAINS {
            let peer_id = publish_domain(root, slug, *seed, name);
            specs.push(BindingSpec {
                name: (*name).to_string(),
                target_peer_id: peer_id.clone(),
                // The origin each domain is served from. In this local web that
                // is a path prefix; in a real deployment it is a hostname. Same
                // field either way — it is what turns a peer-id into a fetch.
                origin: Some((*slug).to_string()),
            });
            map.insert((*name).to_string(), peer_id);
        }
        let reg_dir = root.join("registry");
        std::fs::create_dir_all(&reg_dir).unwrap();
        let report = emit_registry(
            &reg_dir,
            entity_crypto::Keypair::from_seed([REGISTRY_SEED; 32]),
            &specs,
            DEFAULT_TTL_MS,
            now_ms(),
        )
        .expect("registry emits");
        (report.registry_peer_id, map)
    }

    /// The consumer, entire. It starts holding **one string** — the registry's
    /// peer-id — and ends holding verified page bytes.
    fn visit(
        web: &LocalWeb,
        registry_id: &str,
        name: &str,
        slug: &str,
    ) -> Result<(NamedTarget, String), NameError> {
        // The pin is derived from the peer-id itself: for Ed25519 canonical form
        // the peer-id EMBEDS the public key, so there is no key to distribute.
        let reg_pin = PinnedPublisher::from_peer_id("registry", registry_id)
            .expect("a canonical peer-id yields its own pin");
        let registry = SignedSession::new(reg_pin);

        let target = block_on(resolve_name(web, &registry, name, now_ms(), &ResolverPolicy::undeclared()))?;

        // Hop 2 pins the DOMAIN by the peer-id the registry named, at the origin
        // the registry ALSO named. Nothing here consults the registry again, and
        // nothing here was configured with this domain: both halves came out of
        // the binding. `slug` is only the fallback for a transport-less binding.
        let origin = target.origin.clone().unwrap_or_else(|| slug.to_string());
        let site_pin = PinnedPublisher::from_peer_id(&origin, &target.peer_id)
            .expect("the named peer-id yields its own pin");
        let site = SignedSession::new(site_pin);
        let page = block_on(site.resolve(web, &format!("sites/{slug}/pages/index")))
            .map_err(NameError::Registry)?;
        Ok((target, String::from_utf8_lossy(&page.data).to_string()))
    }

    /// **The full path, four domains, nothing live.** One pinned registry
    /// peer-id resolves four names to four independently published domains, and
    /// each one's page comes back off its OWN signed root.
    #[test]
    fn one_pinned_registry_resolves_four_names_to_four_verified_domains() {
        let root = tempfile::tempdir().unwrap();
        let (registry_id, expected) = stand_up(root.path());
        let web = LocalWeb::new(root.path());

        eprintln!("registry: {registry_id}");
        for (name, slug, _) in DOMAINS {
            let (target, body) = visit(&web, &registry_id, name, slug)
                .unwrap_or_else(|e| panic!("{name}: {e}"));

            assert_eq!(
                &target.peer_id,
                expected.get(*name).unwrap(),
                "{name} must resolve to the domain the registry bound"
            );
            assert!(
                body.contains(&format!("served by {slug}")),
                "{name} served the wrong domain's page: {body:?}"
            );
            // B17 — the evidence, not a bare success.
            assert!(target.evidence.association_committed);
            assert!(target.evidence.name_checked);
            assert!(target.evidence.revocation_checked);
            assert_eq!(target.evidence.registry_peer_id, registry_id);
            assert!(target.evidence.expires_at_ms > now_ms());

            eprintln!("  {name} → {} ({slug})", target.peer_id);
        }
        eprintln!("full path: {} fetch(es) for 4 domains", web.count());
    }

    /// **The substitution vector, on the real chain.** A hostile static host
    /// repoints one by-name pointer file at a binding the registry legitimately
    /// issued for a *different* name — forging nothing. On the host-trusted read
    /// this resolves with the registry's own trust anchor and the wrong peer-id
    /// (our routed F1). Walking the signed root, the pointer file is not even
    /// consulted, so the swap has no effect at all.
    #[test]
    fn repointing_a_by_name_pointer_does_not_change_what_a_name_resolves_to() {
        let root = tempfile::tempdir().unwrap();
        let (registry_id, expected) = stand_up(root.path());

        // The attack: serve entitychurch.org's by-name key the bytes of
        // lab.entitychurch.org's pointer. Both are genuine registry artifacts.
        let by_name = |n: &str| {
            root.path()
                .join("registry")
                .join(&registry_id)
                .join(format!("system/registry/binding/by-name/{n}.bin"))
        };
        let lab = std::fs::read(by_name("lab.entitychurch.org")).unwrap();
        std::fs::write(by_name("entitychurch.org"), lab).unwrap();

        let web = LocalWeb::new(root.path());
        let (target, body) = visit(&web, &registry_id, "entitychurch.org", "foundation")
            .expect("the signed-root path is unaffected by the pointer file");
        assert_eq!(
            &target.peer_id,
            expected.get("entitychurch.org").unwrap(),
            "a repointed pointer must not change the resolution"
        );
        assert!(body.contains("served by foundation"));
    }

    /// A name the registry never bound is [`NameError::NotBound`] — an absence
    /// the signed root **committed to**, not one the host chose to report.
    #[test]
    fn a_name_the_registry_never_bound_is_not_bound() {
        let root = tempfile::tempdir().unwrap();
        let (registry_id, _) = stand_up(root.path());
        let web = LocalWeb::new(root.path());
        let got = visit(&web, &registry_id, "nobody.example", "foundation");
        assert!(matches!(got, Err(NameError::NotBound)), "got {got:?}");
    }

    /// **The domain is pinned by the peer-id the registry named, and that pin is
    /// load-bearing.** Serve the *wrong domain's* tree at the origin and the
    /// walk refuses it: the root verifies against its own publisher, but not
    /// against the peer-id the binding named.
    #[test]
    fn a_domain_serving_another_domains_tree_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let (registry_id, _) = stand_up(root.path());

        // `foundation/` now serves `lab/`'s published tree, wholesale.
        std::fs::remove_dir_all(root.path().join("foundation")).unwrap();
        copy_dir(&root.path().join("lab"), &root.path().join("foundation"));

        let web = LocalWeb::new(root.path());
        let got = visit(&web, &registry_id, "entitychurch.org", "foundation");
        assert!(got.is_err(), "the wrong publisher's tree must be refused: {got:?}");
    }


    /// **The question this whole layer answers: a name carries its origin.**
    /// The consumer is configured with NOTHING about the domains — no origins
    /// map, no pre-registration. It pins one registry, and every domain's
    /// location comes out of the binding's `http-poll` transport profile.
    #[test]
    fn a_resolved_name_carries_the_origin_its_content_is_served_from() {
        let root = tempfile::tempdir().unwrap();
        let (registry_id, _) = stand_up(root.path());
        let web = LocalWeb::new(root.path());

        let reg_pin = PinnedPublisher::from_peer_id("registry", &registry_id).unwrap();
        let registry = SignedSession::new(reg_pin);
        for (name, slug, _) in DOMAINS {
            let t = block_on(resolve_name(&web, &registry, name, now_ms(), &ResolverPolicy::undeclared())).unwrap();
            assert_eq!(
                t.origin.as_deref(),
                Some(*slug),
                "{name} must carry where it is served"
            );
        }
    }

    /// **arch D10, at our emitter.** A `peer-issued` binding MUST carry non-empty
    /// `transports` — our own F6, confirmed. We used to emit one anyway and print
    /// a note; a note is not a bound. Refused before anything is written, and the
    /// message names every offending binding rather than the first.
    #[test]
    fn a_binding_with_no_transport_is_refused_before_anything_is_written() {
        let root = tempfile::tempdir().unwrap();
        let peer = publish_domain(root.path(), "solo", 0xC1, "Solo");
        let reg_dir = root.path().join("registry");
        std::fs::create_dir_all(&reg_dir).unwrap();
        let err = emit_registry(
            &reg_dir,
            entity_crypto::Keypair::from_seed([0xC2; 32]),
            &[
                BindingSpec { name: "solo.example".into(), target_peer_id: peer.clone(), origin: None },
                BindingSpec {
                    name: "ok.example".into(),
                    target_peer_id: peer.clone(),
                    origin: Some("https://ok.example".into()),
                },
                BindingSpec { name: "also.example".into(), target_peer_id: peer, origin: None },
            ],
            DEFAULT_TTL_MS,
            now_ms(),
        )
        .expect_err("a binding with no transports must not be emittable");
        assert!(err.contains("solo.example"), "{err}");
        assert!(err.contains("also.example"), "both offenders reported, not just the first: {err}");
        assert!(!err.contains("ok.example"), "the conforming binding is not named: {err}");

        // …and nothing was written. A refusal that leaves a half-registry behind
        // is worse than the emit it prevented.
        assert!(
            std::fs::read_dir(&reg_dir).unwrap().next().is_none(),
            "the output directory must be untouched"
        );
    }

    /// The **resolver** stays tolerant of what the emitter refuses: a consumer
    /// meets registries it did not emit, and D10 binds issuers, not readers. No
    /// transports ⇒ `None`, recorded rather than guessed, so a caller can tell
    /// "no transport published" from "the origin is empty".
    #[test]
    fn a_binding_with_no_transports_yields_no_origin_rather_than_a_guess() {
        assert!(super::http_poll_layout(&[]).is_none());
    }

    /// **F6's rule survives; F6's verdict does not.** Both halves matter, and
    /// this test carries both.
    ///
    /// The rule: the trailing `/{peer_id}` is dropped **only** when the last
    /// segment is exactly the peer we are about to pin. An unconditional
    /// `rsplit_once('/')` turns a legal `tree_url_prefix: "https://x.example"`
    /// into **`"https:/"`**, and the fetch then fails as a transport error that
    /// reads *"the origin is down"*. That guard is now
    /// [`PublishLayout::origin_for`] and is asserted below.
    ///
    /// The verdict F6 reached — that a prefix without our peer segment is a
    /// *layout we cannot consume*, reported as `None` — was **wrong, and the
    /// cross-implementation run is what proved it**. It is the other conformant
    /// form of §6.5.3's tree join (origin-rooted, peer-id appended by the
    /// consumer): workbench-go publishes exactly that. Refusing it meant
    /// refusing every publisher who is not our emitter, which is the failure F6
    /// was trying to *avoid*, arrived at from the other side.
    ///
    /// Mutation check: restore the unconditional split and case 1's second
    /// assertion yields `"https:/"`; drop the conditional entirely and case 2's
    /// URL doubles the peer-id.
    #[test]
    fn a_prefix_is_read_as_peer_rooted_or_origin_rooted_never_truncated() {
        use crate::content_site::registry_publish::http_poll_profile;
        let peer = "2PEERTARGET";
        let layout = |p: entity_ecf::Value| super::http_poll_layout(&[p]).expect("decodes");

        // 1. **Peer-rooted** — our emitter's `{origin}/{peer_id}`. The segment
        //    is dropped to recover the origin, including when the origin itself
        //    carries path segments.
        let ours = layout(http_poll_profile(peer, "https://x.example"));
        assert_eq!(ours.origin_for(peer), "https://x.example");
        let nested = layout(http_poll_profile(peer, "https://x.example/base"));
        assert_eq!(
            nested.origin_for(peer),
            "https://x.example/base",
            "an unconditional split would say `https:/` here — that was F6"
        );
        assert_eq!(
            ours.tree_leaf_url(peer, "system/peer/published-root"),
            "https://x.example/2PEERTARGET/system/peer/published-root.bin",
            "peer-rooted: the prefix already carries the peer, so it is not appended twice"
        );

        // 2. **Origin-rooted** — the form F6 refused and §6.5.3's normative
        //    sentence specifies. Consumable: the consumer appends the peer-id.
        let theirs = layout(profile_with(peer, "https://y.example", "https://y.example/manifest"));
        assert_eq!(theirs.origin_for(peer), "https://y.example");
        assert_eq!(
            theirs.tree_leaf_url(peer, "system/peer/published-root"),
            "https://y.example/2PEERTARGET/system/peer/published-root.bin"
        );

        // 3. A prefix ending in some OTHER peer's id is origin-rooted too — the
        //    segment dropped has to be the peer being pinned, or nothing is
        //    dropped.
        let other = layout(profile_with(
            peer,
            "https://y.example/2SOMEONEELSE",
            "https://y.example/2SOMEONEELSE/manifest",
        ));
        assert_eq!(other.origin_for(peer), "https://y.example/2SOMEONEELSE");
    }

    /// **The manifest comes from the profile, not from our convention** — the
    /// §6.5.3 v1.8 MUST, at the one call site a name resolution goes through.
    #[test]
    fn a_binding_carries_the_publishers_own_manifest_url_not_ours() {
        use crate::content_site::registry_publish::http_poll_profile;
        let peer = "2PEERTARGET";

        let ours = super::http_poll_layout(&[http_poll_profile(peer, "https://x.example")])
            .expect("decodes");
        assert_eq!(
            ours.manifest_url,
            "https://x.example/2PEERTARGET/system/peer/published-root"
        );

        let theirs = super::http_poll_layout(&[profile_with(
            peer,
            "https://y.example",
            "https://y.example/manifest",
        )])
        .expect("decodes");
        assert_eq!(
            theirs.manifest_url, "https://y.example/manifest",
            "a publisher who serves its root somewhere else is consumable — deriving \
             this from the origin is the hop-0 defect the cross-impl run found"
        );
    }

    /// **Arch's §4 invariant, on the path where it is a security property.**
    ///
    /// `ROUTING-2026-08-18-j` §4: *"the absence of a node is never an answer"* —
    /// a node that resolved and did not hold the key is `not_found`; a node that
    /// did not resolve is a failed walk. §6a.6's revocation lookup is that
    /// distinction **at leaf depth**, and it is the one place where getting it
    /// wrong is not a diagnostic complaint: [`resolve_name`] reads
    /// `SignedFetchError::Absent` on the revocation key as **"not revoked"**. If a
    /// withheld node could produce `Absent`, a hostile origin would suppress a
    /// revocation by *deleting a file*, and we would hand back a `NamedTarget`
    /// whose evidence says `revocation_checked: true`.
    ///
    /// It cannot, and this is why: a miss inside the walk is recorded by
    /// `PumpFetcher`, the pump then **fetches** it, and a withheld blob makes that
    /// fetch fail → `Transport` → [`NameError::Registry`]. `Absent` is reachable
    /// only when the walk completed with **nothing outstanding** — every node on
    /// the path resolved, and none held the key.
    ///
    /// So the answer is fail-closed, and the name does not resolve at all.
    ///
    /// Mutation check: make the pump's content fetch treat a 404 as `Absent`
    /// instead of `Transport` and this test fails with a resolved name whose
    /// evidence claims the revocation was checked.
    #[test]
    fn a_withheld_node_can_never_produce_a_resolved_name() {
        let dir = tempfile::tempdir().unwrap();
        let (registry_id, _map) = stand_up(dir.path());
        let web = LocalWeb::new(dir.path());

        // Control: the intact registry resolves, and reports the revocation as
        // checked. Without this the assertion below could pass on a fixture that
        // never resolved anything.
        let (ok, _) = visit(&web, &registry_id, "entitychurch.org", "foundation")
            .expect("the intact chain resolves");
        assert!(ok.evidence.revocation_checked, "precondition: the probe ran");

        // **Withhold exactly the files this walk READ, one at a time.** The
        // candidate set is the control run's own fetch log, which is the precise
        // definition of "on the path": a registry carries four bindings, so most
        // of its blobs belong to names we are not resolving and withholding one
        // of those correctly changes nothing. (Measured — the first version of
        // this test swept every blob under `registry/content` and failed on a
        // body for a different name, which is the fixture being wrong, not the
        // code.) Under hash-keyed routing there is no stable referent for "the
        // node covering this key", so the fetch log is the only honest selector.
        let read: Vec<String> = web
            .fetched
            .borrow()
            .iter()
            .filter(|u| u.contains("/content/"))
            .cloned()
            .collect::<std::collections::BTreeSet<String>>()
            .into_iter()
            .collect();
        assert!(read.len() > 2, "precondition: the walk reads several blobs, got {}", read.len());

        for url in &read {
            let path = dir.path().join(url.trim_start_matches('/'));
            let saved = std::fs::read(&path).expect("read blob");
            std::fs::remove_file(&path).expect("withhold it");

            let probe = LocalWeb::new(dir.path());
            let got = visit(&probe, &registry_id, "entitychurch.org", "foundation");
            std::fs::write(&path, &saved).expect("restore");

            match got {
                Err(_) => {}
                Ok((t, _)) => panic!(
                    "withholding {url} — a blob this walk READS — still produced a RESOLVED \
                     name (revocation_checked={}). A hostile origin can suppress a revocation \
                     by deleting one file.",
                    t.evidence.revocation_checked
                ),
            }
        }

        // The chain is intact again — so the loop measured withholding, not
        // accumulated damage.
        let after = LocalWeb::new(dir.path());
        visit(&after, &registry_id, "entitychurch.org", "foundation")
            .expect("restoring every blob restores the chain");
        println!(
            "§4 invariant: {} blobs on the walk, each withheld in turn; every one failed the \
             walk and none became an answer",
            read.len()
        );
    }

    /// **Why the revocation probe cannot be selectively starved — measured.**
    ///
    /// The composed test above survives a mutation that turns a withheld blob
    /// into `Absent`, which looked at first like a weak gate. It is not: it is a
    /// structural fact worth pinning, because the whole fail-closed argument for
    /// §6a.6 rests on it.
    ///
    /// Within one [`SignedSession`] the by-name walk and the revocation probe
    /// share a content cache, and the by-target key sits under trie nodes the
    /// by-name walk has already fetched. Measured fetch sequence for one full
    /// `resolve_name`: manifest, signature, **3 content blobs**, manifest again
    /// (it is mutable, so re-read) — and then **nothing**. The probe adds zero
    /// content fetches.
    ///
    /// So a hostile origin has no blob it can withhold that starves *only* the
    /// revocation probe: every blob the probe needs, hop 1 needed first, and hop
    /// 1 fails closed before any "not revoked" conclusion is reached.
    ///
    /// **The case this does NOT cover, stated plainly:** a registry that has
    /// *published* a revocation puts the by-target leaf in its own blob, fetched
    /// only by the probe — and withholding **that** is exactly F2. We cannot
    /// build the fixture, because [`super::registry_publish::emit_registry`]
    /// emits bindings and has no way to emit a revocation. That is a fixture
    /// gap on the security-critical path, not a proven property. Do not read
    /// **§6a's resolver-side ceiling: `min(binding.ttl, local_max)`, and it is
    /// a USE bound — never a re-issue.** `EXTENSION-REGISTRY` 1.11 (arch
    /// `d1584a1`) makes this the half that protects the consumer, because a
    /// ceiling the registry enforces cannot defend anyone against *that*
    /// registry — it simply issues itself a long one.
    ///
    /// Three properties, and the third is the one that would rot quietly:
    /// the clamp applies, an undeclared ceiling changes nothing, and the
    /// **binding's content hash is identical either way**. If a future refactor
    /// ever "helpfully" rewrote the binding to carry the clamped TTL, the
    /// content address would move and every signature over it would stop
    /// verifying — so this asserts the artifact was left alone, not merely that
    /// the arithmetic was right.
    #[test]
    fn a_local_ceiling_clamps_the_lifetime_without_rewriting_the_binding() {
        let root = tempfile::tempdir().unwrap();
        let (registry_id, _expected) = stand_up(root.path());
        let web = LocalWeb::new(root.path());
        let registry =
            SignedSession::new(PinnedPublisher::from_peer_id("registry", &registry_id).unwrap());
        let name = "docs.entitychurch.org";

        // No ceiling: we honor exactly what the registry issued.
        let open = block_on(resolve_name(
            &web,
            &registry,
            name,
            now_ms(),
            &ResolverPolicy::undeclared(),
        ))
        .expect("resolves with no ceiling");
        let issued = open.evidence.issued_ttl_ms;
        assert!(issued > 0, "precondition: the fixture issues a real ttl");
        assert_eq!(open.evidence.effective_ttl_ms, issued, "nothing to clamp");
        assert!(!open.evidence.ttl_was_clamped());

        // A ceiling BELOW the issued ttl: we honor ours, and the expiry moves in.
        let cap = issued / 2;
        assert!(cap > 0, "precondition: the fixture ttl is divisible enough to halve");
        let capped = block_on(resolve_name(
            &web,
            &registry,
            name,
            now_ms(),
            &ResolverPolicy::with_max_ttl_ms(cap),
        ))
        .expect("resolves with a ceiling");
        assert_eq!(capped.evidence.effective_ttl_ms, cap, "min(issued, local_max)");
        assert_eq!(capped.evidence.issued_ttl_ms, issued, "what the registry said is preserved");
        assert!(capped.evidence.ttl_was_clamped());
        assert!(
            capped.evidence.expires_at_ms < open.evidence.expires_at_ms,
            "a ceiling must shorten the window, not just be recorded"
        );

        // A ceiling ABOVE it does NOT extend: the rule is a minimum, and a
        // resolver cannot grant a binding more life than its issuer did.
        let generous = block_on(resolve_name(
            &web,
            &registry,
            name,
            now_ms(),
            &ResolverPolicy::with_max_ttl_ms(issued.saturating_mul(10)),
        ))
        .expect("resolves");
        assert_eq!(generous.evidence.effective_ttl_ms, issued, "a ceiling never extends");

        // THE USE-BOUND PROPERTY: same binding, same content address, whatever
        // this resolver decided to honor.
        assert_eq!(
            open.evidence.binding_hash, capped.evidence.binding_hash,
            "the clamp must not rewrite the binding — this is a use bound, not a re-issue"
        );
        assert_eq!(open.peer_id, capped.peer_id, "and it resolves to the same peer");
    }

    /// this test as covering it.
    #[test]
    fn the_revocation_probe_adds_no_fetches_of_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let (registry_id, _map) = stand_up(dir.path());
        let web = LocalWeb::new(dir.path());
        let pin = PinnedPublisher::from_peer_id("registry", &registry_id).unwrap();
        let session = SignedSession::new(pin);

        let target = block_on(resolve_name(&web, &session, "entitychurch.org", now_ms(), &ResolverPolicy::undeclared()))
            .expect("resolves");
        assert!(target.evidence.revocation_checked);
        let after_resolve = content_fetches(&web);

        // Probe the same revocation key again on the same session. If it needed
        // any blob of its own, this would fetch it.
        let rev_key = rel(
            &registry_id,
            &revocation_by_target_path(&registry_id, &target.evidence.binding_hash),
        );
        let got = block_on(session.resolve(&web, &rev_key));
        assert_eq!(got.err(), Some(SignedFetchError::Absent), "nothing is revoked here");
        assert_eq!(
            content_fetches(&web),
            after_resolve,
            "the revocation probe must add no content fetches — if it does, there is a blob a \
             hostile origin can withhold that starves ONLY the probe, and the fail-closed \
             argument for §6a.6 no longer follows from hop 1 failing first"
        );
    }

    /// Content-blob fetches so far (the manifest is mutable and re-read by
    /// design, so it is excluded).
    fn content_fetches(web: &LocalWeb) -> usize {
        web.fetched.borrow().iter().filter(|u| u.contains("/content/")).count()
    }

    /// An `http-poll` profile with a caller-chosen `tree_url_prefix` — what a
    /// publisher who is not our emitter may legitimately publish.
    fn hand_rolled_profile(peer_id: &str, tree_url_prefix: &str) -> entity_ecf::Value {
        entity_ecf::cbor_map! {
            "peer_id" => entity_ecf::Value::Text(peer_id.to_string()),
            "transport_type" => entity_ecf::Value::Text("http-poll".into()),
            "endpoint" => entity_ecf::cbor_map! {
                "tree_url_prefix" => entity_ecf::Value::Text(tree_url_prefix.to_string()),
            },
        }
    }

    /// A **complete** `http-poll` profile in the origin-rooted form — what a
    /// publisher who is not our emitter advertises (workbench-go's shape).
    fn profile_with(peer_id: &str, tree_url_prefix: &str, manifest_url: &str) -> entity_ecf::Value {
        entity_ecf::cbor_map! {
            "peer_id" => entity_ecf::Value::Text(peer_id.to_string()),
            "transport_type" => entity_ecf::Value::Text("http-poll".into()),
            "endpoint" => entity_ecf::cbor_map! {
                "tree_url_prefix" => entity_ecf::Value::Text(tree_url_prefix.to_string()),
                "content_url_prefix" => entity_ecf::Value::Text(format!("{tree_url_prefix}/content")),
                "content_layout" => entity_ecf::Value::Text("sharded-2-4".into()),
                "tree_leaf_suffix" => entity_ecf::Value::Text(".bin".into()),
                "manifest_url_prefix" => entity_ecf::Value::Text(manifest_url.to_string()),
            },
        }
    }

    /// **What the origin actually learns when you resolve a name through it.**
    ///
    /// `EXTENSION-REGISTRY` §4.1 step 2 calls `name_format_dispatch` *"the primary
    /// privacy mechanism — without it, the queried name leaks to broad-matching
    /// backends earlier in priority."* That is exactly right for a **query**
    /// backend (DNS-TXT sends the name; a well-known URL puts it in the path).
    ///
    /// It is **not** what a static `peer-issued` backend does, and the difference
    /// is worth a measurement rather than an argument. We resolve by walking a
    /// content-addressed trie from the signed root: every request is
    /// `content/{aa}/{bb}/{hex}`, and the key is matched **inside** a node we
    /// already hold. **The name never crosses the wire.**
    ///
    /// This does not make the catch-all glob free — see the sibling test for what
    /// *does* leak — but it means the cost of routing an unknown name at a static
    /// registry is a hash-prefix oracle, not disclosure of the name.
    #[test]
    fn resolving_a_name_never_puts_that_name_on_the_wire() {
        let root = tempfile::tempdir().unwrap();
        let (registry_id, expected) = stand_up(root.path());
        let web = LocalWeb::new(root.path());
        let registry =
            SignedSession::new(PinnedPublisher::from_peer_id("registry", &registry_id).unwrap());

        let name = "docs.entitychurch.org";
        let t = block_on(resolve_name(&web, &registry, name, now_ms(), &ResolverPolicy::undeclared())).unwrap();
        assert_eq!(t.peer_id, expected[name], "precondition: the resolve succeeded");

        let urls = web.fetched.borrow().clone();
        assert!(!urls.is_empty(), "precondition: something was fetched");
        for u in &urls {
            assert!(
                !u.contains(name),
                "the queried name appeared in a request URL: {u}"
            );
            // Nor any prefix of it that would give the name away — the local part
            // is the identifying half of a registry-scoped name.
            assert!(!u.contains("docs.entity"), "a name fragment appeared in: {u}");
        }
    }

    /// **The other half, and the honest one: a name the registry does NOT carry
    /// still costs you something.** The walk descends by the name's own hash, so
    /// the origin observes which trie path was taken before the lookup died. That
    /// is a hash-prefix oracle over the name — far weaker than the name itself,
    /// and not nothing.
    ///
    /// What this pins is the *shape* of the disclosure, because it is the input to
    /// the dispatch-glob argument: a miss is **cheap and quiet** (it terminates in
    /// the trie, fetching only interior nodes shared by many names) rather than a
    /// published query. A catch-all glob at a static registry is therefore a
    /// materially smaller privacy event than the same glob at a DNS-TXT backend —
    /// which is why the default globs should be pinned per *backend kind*, not as
    /// one rule for all backends.
    #[test]
    fn a_name_the_registry_does_not_carry_dies_in_the_trie_without_being_sent() {
        let root = tempfile::tempdir().unwrap();
        let (registry_id, _) = stand_up(root.path());
        let web = LocalWeb::new(root.path());
        let registry =
            SignedSession::new(PinnedPublisher::from_peer_id("registry", &registry_id).unwrap());

        let private = "payroll.internal.example";
        let got = block_on(resolve_name(&web, &registry, private, now_ms(), &ResolverPolicy::undeclared()));
        assert!(got.is_err(), "a name the registry does not carry must not resolve");

        let urls = web.fetched.borrow().clone();
        for u in &urls {
            assert!(!u.contains("payroll"), "the private name leaked into a URL: {u}");
            assert!(!u.contains("internal"), "the private name leaked into a URL: {u}");
        }
        println!(
            "privacy measured: resolving a name the registry does NOT carry sent {} request(s), \
             none containing the name",
            urls.len()
        );
    }

    /// **The registry browser's floor.** Without this a consumer can only
    /// confirm a name it already knew — a signed root answers "what is at this
    /// key", never "what keys exist".
    #[test]
    fn a_registry_lists_the_names_it_carries_and_each_one_resolves() {
        let root = tempfile::tempdir().unwrap();
        let (registry_id, expected) = stand_up(root.path());
        let web = LocalWeb::new(root.path());
        let reg_pin = PinnedPublisher::from_peer_id("registry", &registry_id).unwrap();

        let listed = block_on(list_names(&web, &reg_pin)).expect("the registry enumerates");
        let mut want: Vec<String> = expected.keys().cloned().collect();
        want.sort();
        assert_eq!(listed, want, "the listing must offer every issued name");

        // The listing is a MENU, not an inventory — every entry is then resolved
        // through the signed root, which is the only thing that means anything.
        let registry = SignedSession::new(reg_pin);
        for name in &listed {
            let t = block_on(resolve_name(&web, &registry, name, now_ms(), &ResolverPolicy::undeclared()))
                .unwrap_or_else(|e| panic!("{name} listed but does not resolve: {e}"));
            assert_eq!(&t.peer_id, expected.get(name).unwrap());
        }
    }

    /// **A name invented by a hostile listing fails to resolve**, which is why
    /// the transport-trusted listing is safe to read. Hiding a name is the
    /// undetectable half, and nothing anywhere claims completeness.
    #[test]
    fn a_name_added_to_the_listing_by_the_host_does_not_resolve() {
        let root = tempfile::tempdir().unwrap();
        let (registry_id, _) = stand_up(root.path());
        let listing = root
            .path()
            .join("registry")
            .join(&registry_id)
            .join("system/registry/binding/by-name.list");
        let mut body = std::fs::read_to_string(&listing).unwrap();
        body.push_str("evil.example\n");
        std::fs::write(&listing, body).unwrap();

        let web = LocalWeb::new(root.path());
        let reg_pin = PinnedPublisher::from_peer_id("registry", &registry_id).unwrap();
        let listed = block_on(list_names(&web, &reg_pin)).unwrap();
        assert!(listed.contains(&"evil.example".to_string()), "the host really did inject it");

        let registry = SignedSession::new(reg_pin);
        let got = block_on(resolve_name(&web, &registry, "evil.example", now_ms(), &ResolverPolicy::undeclared()));
        assert!(matches!(got, Err(NameError::NotBound)), "got {got:?}");
    }


    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap().flatten() {
            let (src, dst) = (e.path(), to.join(e.file_name()));
            if src.is_dir() {
                copy_dir(&src, &dst);
            } else {
                std::fs::copy(&src, &dst).unwrap();
            }
        }
    }

    // -----------------------------------------------------------------------
    // The same chain, over a real socket
    // -----------------------------------------------------------------------

    /// A minimal static origin **over TCP**, emitting the CORS + cache headers
    /// `RUNBOOK-CDN-BROWSER-DEPLOYMENT` §2.1 requires. Deliberately not
    /// `python3 -m http.server` (which sends no CORS at all) and not
    /// `tools/cors-serve.py` (which would make this test depend on a process we
    /// would then have to find, kill and port-allocate around).
    ///
    /// It exists because **every other test in this file reads from disk**, and a
    /// disk read proves the walk against a *filesystem layout*, not against a
    /// server's path mapping — index resolution, trailing slashes, percent-
    /// encoding and content-type all live only on the HTTP side. (Measured
    /// honestly: reintroducing the `published-root.bin` bug fails the disk tests
    /// too. What changes is the diagnosis — here it is `HTTP 404` naming the
    /// exact path the origin was asked for, which is the difference between
    /// "that key does not exist" and "you asked the wrong URL".)
    fn serve_dir(root: std::path::PathBuf) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                let mut buf = [0u8; 4096];
                let Ok(n) = s.read(&mut buf) else { continue };
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or("/")
                    .to_string();
                let rel = path.trim_start_matches('/');
                // No traversal: this is a test server, but a test server that
                // serves `../..` teaches the wrong shape.
                let body = if rel.contains("..") { None } else { std::fs::read(root.join(rel)).ok() };
                let resp = match body {
                    Some(b) => {
                        // §2.1: mutable endpoints must not be cached, or a stale
                        // signed root silently defeats the `seq` floor.
                        let cache = if rel.ends_with("published-root") || rel.ends_with(".list") {
                            "no-store"
                        } else {
                            "public, max-age=31536000, immutable"
                        };
                        let head = format!(
                            "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\n\
                             Access-Control-Allow-Methods: GET, HEAD\r\n\
                             Cache-Control: {cache}\r\nContent-Length: {}\r\n\r\n",
                            b.len()
                        );
                        let mut out = head.into_bytes();
                        out.extend_from_slice(&b);
                        out
                    }
                    None => b"HTTP/1.1 404 Not Found\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: 0\r\n\r\n".to_vec(),
                };
                let _ = s.write_all(&resp);
                let _ = s.flush();
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    /// A [`BinSource`] that speaks HTTP/1.1 to [`serve_dir`]. Blocking inside the
    /// future is fine here — `block_on` above is a bare poll loop, so there is no
    /// executor to starve, and it keeps the client dependency-free.
    struct HttpWeb {
        base: String,
        fetched: RefCell<Vec<String>>,
        statuses: RefCell<Vec<u16>>,
    }

    impl HttpWeb {
        fn new(base: &str) -> Self {
            Self {
                base: base.to_string(),
                fetched: RefCell::new(Vec::new()),
                statuses: RefCell::new(Vec::new()),
            }
        }
    }

    impl BinSource for HttpWeb {
        fn get(
            &self,
            url: String,
            _freshness: Freshness,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, PollError>>>> {
            use std::io::{Read, Write};
            self.fetched.borrow_mut().push(url.clone());
            // The consumer builds absolute URLs from the origin it was pinned
            // at; anything relative would mean the origin never reached it.
            let rest = url.strip_prefix(&self.base).unwrap_or(&url).to_string();
            let addr = self.base.trim_start_matches("http://").to_string();
            let result = (|| -> Result<(u16, Vec<u8>), String> {
                let mut s = std::net::TcpStream::connect(&addr).map_err(|e| e.to_string())?;
                let req = format!("GET {rest} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
                s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
                let mut raw = Vec::new();
                s.read_to_end(&mut raw).map_err(|e| e.to_string())?;
                let split = raw
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                    .ok_or_else(|| "no header terminator".to_string())?;
                let head = String::from_utf8_lossy(&raw[..split]).to_string();
                let status: u16 = head
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|c| c.parse().ok())
                    .ok_or_else(|| "no status".to_string())?;
                // The runbook's whole point: a browser drops a cross-origin
                // response with no ACAO, however good the bytes are.
                if status == 200 && !head.to_ascii_lowercase().contains("access-control-allow-origin")
                {
                    return Err(format!("200 with no CORS header — a browser would drop this: {rest}"));
                }
                Ok((status, raw[split + 4..].to_vec()))
            })();
            let out = match result {
                Ok((status, body)) => {
                    self.statuses.borrow_mut().push(status);
                    if status == 200 {
                        Ok(body)
                    } else if status == 404 || status == 410 {
                        // Same line the browser fetcher draws — the origin
                        // answered "absent", which over a real socket is what
                        // a withheld blob looks like.
                        Err(PollError::NotFound(status))
                    } else {
                        Err(PollError::Decode(format!("HTTP {status} for {rest}")))
                    }
                }
                Err(e) => Err(PollError::Decode(e)),
            };
            Box::pin(std::future::ready(out))
        }
    }

    /// Publish the four domains + the registry, with each binding carrying an
    /// **absolute cross-origin** `http-poll` origin rather than a path slug.
    fn stand_up_at(root: &Path, origin_base: &str) -> (String, BTreeMap<String, String>) {
        let mut map = BTreeMap::new();
        let mut specs = Vec::new();
        for (name, slug, seed) in DOMAINS {
            let peer_id = publish_domain(root, slug, *seed, name);
            specs.push(BindingSpec {
                name: (*name).to_string(),
                target_peer_id: peer_id.clone(),
                origin: Some(format!("{origin_base}/{slug}")),
            });
            map.insert((*name).to_string(), peer_id);
        }
        let reg_dir = root.join("registry");
        std::fs::create_dir_all(&reg_dir).unwrap();
        let report = emit_registry(
            &reg_dir,
            entity_crypto::Keypair::from_seed([REGISTRY_SEED; 32]),
            &specs,
            DEFAULT_TTL_MS,
            now_ms(),
        )
        .expect("registry emits");
        (report.registry_peer_id, map)
    }

    /// **The multi-domain demo, over real HTTP, and the first time any of this
    /// has left the filesystem.**
    ///
    /// A consumer holds ONE string — the registry's peer-id — and a URL. It
    /// resolves four names through the registry's signed root, and for each one
    /// pins the domain by the peer-id the registry named and walks *that* peer's
    /// signed root to the authored page. Every byte crosses a socket; every
    /// response is checked for the CORS header a browser would require.
    ///
    /// The CORS half is the part that is genuinely only provable here: the
    /// harness rejects a 200 that carries no `Access-Control-Allow-Origin`,
    /// because a browser would — and `curl` would not.
    #[test]
    fn four_domains_resolve_and_serve_over_real_http_with_cors() {
        let root = tempfile::tempdir().unwrap();
        let base = serve_dir(root.path().to_path_buf());
        let (registry_id, expected) = stand_up_at(root.path(), &base);

        let web = HttpWeb::new(&base);
        let registry = SignedSession::new(
            PinnedPublisher::from_peer_id(format!("{base}/registry"), &registry_id).unwrap(),
        );

        for (name, slug, _) in DOMAINS {
            // Hop 1 — the name, through the registry's signed root.
            let target = block_on(resolve_name(&web, &registry, name, now_ms(), &ResolverPolicy::undeclared()))
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(&target.peer_id, &expected[*name], "{name} resolved to the wrong peer");
            let origin = target.origin.clone().unwrap_or_else(|| panic!("{name} published no origin"));
            assert_eq!(origin, format!("{base}/{slug}"), "the binding must carry the real origin");

            // Hop 2 — pin the DOMAIN by the peer-id the registry just named.
            let site = SignedSession::new(
                PinnedPublisher::from_peer_id(origin, &target.peer_id).unwrap(),
            );
            let page = block_on(site.resolve(&web, &format!("sites/{slug}/pages/index")))
                .unwrap_or_else(|e| panic!("{name} page: {e:?}"));
            let body = String::from_utf8_lossy(&page.data);
            assert!(body.contains(name), "{name}'s page must be its own: {body:?}");
        }

        // Nothing 404'd: a wrong URL shape would show up here and nowhere else.
        let statuses = web.statuses.borrow().clone();
        assert!(!statuses.is_empty(), "precondition: requests were made");
        assert!(
            statuses.iter().all(|s| *s == 200),
            "every request must have been served: {:?}",
            statuses.iter().filter(|s| **s != 200).collect::<Vec<_>>()
        );
        println!(
            "federation over HTTP: {} names, {} requests, all 200, all CORS-clean",
            DOMAINS.len(),
            statuses.len()
        );
    }

    /// **The runbook's failure mode, asserted rather than described.** An origin
    /// that serves perfect bytes with no `Access-Control-Allow-Origin` is
    /// unusable from a browser — `curl` is happy and the app is broken. The
    /// harness treats a missing header as a transport failure, so this test is
    /// what proves the harness would notice.
    #[test]
    fn a_200_without_cors_is_treated_as_a_failure_not_a_success() {
        let root = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                // 200, real body, NO CORS header — the python3 -m http.server shape.
                let _ = s.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi",
                );
            }
        });
        drop(root);

        let base = format!("http://127.0.0.1:{port}");
        let web = HttpWeb::new(&base);
        let err = block_on(web.get(format!("{base}/anything"), Freshness::Mutable))
            .expect_err("a CORS-less 200 must not be accepted");
        assert!(format!("{err}").contains("no CORS"), "{err}");
    }
}
